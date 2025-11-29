use clap::Parser;

pub mod cli;
pub mod constants;
pub mod error;
pub mod git;
pub mod github;
pub mod help;
pub mod identifier;
pub mod output;
pub mod parse_url;
pub mod remote;
pub mod repo_manager;

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
    let parsed_input = cli.parse_input().ok_or_else(|| WtgError::Cli {
        message: "Invalid input".to_string(),
        code: 1,
    })?;

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
    let result = Box::pin(identifier::identify(parsed_input.query(), git_repo)).await?;

    // Display the result
    output::display(result)?;

    Ok(())
}
