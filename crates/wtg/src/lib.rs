use clap::Parser;

pub mod backend;
pub mod cli;
pub mod constants;
pub mod error;
pub mod git;
pub mod github;
pub mod help;
pub mod identifier;
pub mod output;
pub(crate) mod parse_input;
pub mod remote;
pub mod repo_manager;
mod resolution;

use cli::Cli;
use error::{WtgError, WtgResult};
use repo_manager::RepoManager;

/// Run the CLI using the process arguments.
pub fn run() -> WtgResult<()> {
    run_with_args(std::env::args())
}

/// Run the CLI using a custom iterator of arguments.
pub fn run_with_args<I, T>(args: I) -> WtgResult<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            // If the error is DisplayHelp, show our custom help
            if err.kind() == clap::error::ErrorKind::DisplayHelp {
                help::display_help();
                return Ok(());
            }
            // Otherwise, propagate the error
            return Err(WtgError::Cli {
                message: err.to_string(),
                code: err.exit_code(),
            });
        }
    };
    run_with_cli(cli)
}

fn run_with_cli(cli: Cli) -> WtgResult<()> {
    // If no input provided, show custom help
    if cli.input.is_none() {
        help::display_help();
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(run_async(cli))
}

async fn run_async(cli: Cli) -> WtgResult<()> {
    // Parse the input to determine if it's a remote repo or local
    let parsed_input = cli.parse_input()?;

    // Use new backend path if -t flag is set
    if cli.use_new_backend {
        return run_with_new_backend(parsed_input).await;
    }

    // Create the appropriate repo manager
    let repo_manager = if let Some(gh_repo_info) = parsed_input.gh_repo_info() {
        RepoManager::remote(gh_repo_info.clone())?
    } else {
        RepoManager::local()?
    };

    // Get the git repo instance
    let git_repo = repo_manager.git_repo()?;

    // Determine the remote info - either from the remote repo manager or from the local repo
    let remote_info = repo_manager
        .remote_info()
        .cloned()
        .map_or_else(|| git_repo.github_remote(), Some);

    // Print snarky messages if no GitHub remote (only for local repos)
    if remote_info.is_none() {
        remote::check_remote_and_snark(git_repo.path());
    }

    // Detect what type of input we have
    let result = Box::pin(identifier::identify(
        parsed_input.query_as_string().as_str(),
        git_repo,
    ))
    .await?;

    // Display the result
    output::display(result)?;

    Ok(())
}

/// Run using the new trait-based backend architecture.
async fn run_with_new_backend(parsed_input: parse_input::ParsedInput) -> WtgResult<()> {
    use backend::resolve_backend;
    use resolution::resolve;

    // Create the backend based on available resources
    let backend = resolve_backend(&parsed_input)?;

    // Print snarky messages if no GitHub remote (only for local repos with git-only backend)
    if backend.repo_info().is_none()
        && let Ok(git_repo) = git::GitRepo::open()
    {
        remote::check_remote_and_snark(git_repo.path());
    }

    // Resolve the query using the backend
    let result = resolve(backend.as_ref(), parsed_input.query()).await?;

    // Display the result (same output code works with both paths)
    output::display(result)?;

    Ok(())
}
