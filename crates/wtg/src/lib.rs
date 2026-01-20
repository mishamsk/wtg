use std::{env, ffi::OsString};

use clap::Parser;

use std::sync::Arc;

use crate::backend::resolve_backend_with_notices;
use crate::cli::Cli;
use crate::error::{WtgError, WtgResult};
use crate::release_filter::ReleaseFilter;
use crate::resolution::resolve;

pub mod backend;
pub mod changelog;
pub mod cli;
pub mod constants;
pub mod error;
pub mod git;
pub mod github;
pub mod help;
pub mod notice;
pub mod output;
pub mod parse_input;
pub mod release_filter;
pub mod remote;
pub mod resolution;
pub mod semver;

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
    // Initialize logging - respects RUST_LOG env var
    // Uses try_init to avoid panic if called multiple times (e.g., in tests)
    let _ = env_logger::try_init();

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
    log::debug!("Parsed input: {parsed_input:?}");

    // Create notice callback - all notices (capability warnings and operational info)
    // are delivered via callback and printed by output::print_notice
    let notice_cb = Arc::new(output::print_notice);

    // Create the backend based on available resources
    log::debug!("Resolving backend (fetch={})", cli.fetch);
    let backend = resolve_backend_with_notices(&parsed_input, cli.fetch, notice_cb)?;
    log::debug!("Backend resolved");

    // Build the release filter from CLI args
    let filter = if let Some(ref release) = cli.release {
        ReleaseFilter::Specific(release.clone())
    } else if cli.skip_prereleases {
        ReleaseFilter::SkipPrereleases
    } else {
        ReleaseFilter::Unrestricted
    };

    // Resolve the query using the backend
    log::debug!("Disambiguating query: {:?}", parsed_input.query());
    let query = backend.disambiguate_query(parsed_input.query()).await?;
    log::debug!("Disambiguated to: {query:?}");

    log::debug!("Resolving query");
    let result = resolve(backend.as_ref(), &query, &filter).await?;
    log::debug!("Resolution complete");

    // Display the result
    output::display(result, &filter)?;

    Ok(())
}
