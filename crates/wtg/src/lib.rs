use clap::Parser;

pub mod cli;
pub mod error;
pub mod git;
pub mod github;
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
    let cli = Cli::try_parse_from(args).map_err(|err| WtgError::Cli {
        message: err.to_string(),
        code: err.exit_code(),
    })?;
    run_with_cli(cli)
}

fn run_with_cli(cli: Cli) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;

    runtime.block_on(async move { run_async(cli).await })
}

async fn run_async(cli: Cli) -> Result<()> {
    // Check git repo and remote status first
    let git_repo = git::GitRepo::open()?;
    let remote_info = git_repo.github_remote();

    // Print snarky messages if no GitHub remote
    remote::check_remote_and_snark(remote_info, git_repo.path());

    // Detect what type of input we have
    let result = identifier::identify(&cli.input, git_repo).await?;

    // Display the result
    output::display(result)?;

    Ok(())
}
