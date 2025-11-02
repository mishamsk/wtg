use clap::Parser;

pub mod cli;
pub mod constants;
pub mod error;
pub mod git;
pub mod github;
pub mod help;
pub mod identifier;
pub mod output;
pub mod remote;

use cli::Cli;
use error::{Result, WtgError};

/// Run the CLI using the process arguments.
pub fn run() -> Result<()> {
    run_with_args(std::env::args())
}

/// Run the CLI using a custom iterator of arguments.
pub fn run_with_args<I, T>(args: I) -> Result<()>
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

fn run_with_cli(cli: Cli) -> Result<()> {
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

async fn run_async(cli: Cli) -> Result<()> {
    // At this point, input is guaranteed to be Some because we check in run_with_cli
    let input = cli.input.expect("input should be Some at this point");

    // Check git repo and remote status first
    let git_repo = git::GitRepo::open()?;
    let remote_info = git_repo.github_remote();

    // Print snarky messages if no GitHub remote
    remote::check_remote_and_snark(remote_info, git_repo.path());

    // Detect what type of input we have
    let result = Box::pin(identifier::identify(&input, git_repo)).await?;

    // Display the result
    output::display(result)?;

    Ok(())
}
