use std::{env, ffi::OsString};

use clap::Parser;

use crate::backend::resolve_backend;
use crate::cli::Cli;
use crate::error::{WtgError, WtgResult};
use crate::resolution::resolve;

pub mod backend;
pub mod cli;
pub mod constants;
pub mod error;
pub mod git;
pub mod github;
pub mod help;
pub mod output;
pub mod parse_input;
pub mod remote;
pub mod resolution;

/// Run the CLI using the process arguments.
pub fn run() -> WtgResult<()> {
    run_with_args(env::args())
}

/// Run the CLI using a custom iterator of arguments.
pub fn run_with_args<I, T>(args: I) -> WtgResult<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
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

    // Create the backend based on available resources
    let resolved = resolve_backend(&parsed_input, cli.fetch)?;

    // Display backend notice if operating in reduced-functionality mode
    if let Some(notice) = &resolved.notice {
        output::display_backend_notice(notice);
    }

    // Resolve the query using the backend
    let result = resolve(resolved.backend.as_ref(), parsed_input.query()).await?;

    // Display the result
    output::display(result)?;

    Ok(())
}
