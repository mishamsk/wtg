use clap::Parser;

mod cli;
mod error;
mod git;
mod github;
mod identifier;
mod output;
mod remote;

use cli::Cli;
use error::Result;

fn main() {
    // Create a tokio runtime for async GitHub API calls
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    if let Err(e) = runtime.block_on(run()) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    // Check git repo and remote status first
    let git_repo = git::GitRepo::open()?;
    let remote_info = git_repo.github_remote();

    // Print snarky messages if no GitHub remote
    remote::check_remote_and_snark(remote_info, git_repo.path());

    // Detect what type of input we have
    let result = identifier::identify(&cli.input).await?;

    // Display the result
    output::display(result)?;

    Ok(())
}
