use clap::Parser;

use crate::constants;

#[derive(Parser, Debug)]
#[command(
    name = "wtg",
    version,
    about = constants::DESCRIPTION,
    disable_help_flag = true,
)]
pub struct Cli {
    /// The thing to identify: commit hash (c62bbcc), issue/PR (#123), file path (Cargo.toml), or tag (v1.2.3)
    #[arg(value_name = "COMMIT|ISSUE|FILE|TAG")]
    pub input: Option<String>,

    /// Print help information
    #[arg(short, long, action = clap::ArgAction::Help)]
    help: Option<bool>,
}
