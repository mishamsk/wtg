use clap::Parser;

use crate::{
    constants,
    error::{WtgError, WtgResult},
    parse_input::{ParsedInput, try_parse_input},
};

#[derive(Parser, Debug)]
#[command(
    name = "wtg",
    version,
    about = constants::DESCRIPTION,
    disable_help_flag = true,
)]
pub struct Cli {
    /// The thing to identify: commit hash (c62bbcc), issue/PR (#123), file path (Cargo.toml), tag (v1.2.3), or a GitHub URL
    #[arg(value_name = "COMMIT|ISSUE|FILE|TAG|URL")]
    pub input: Option<String>,

    /// GitHub repository URL to operate on (e.g., <https://github.com/owner/repo>)
    #[arg(short = 'r', long, value_name = "URL")]
    pub repo: Option<String>,

    /// Allow fetching missing refs from remote into local repository
    ///
    /// By default, local repositories don't fetch to avoid unexpected network calls.
    /// Use this flag to enable fetching when a commit/tag isn't found locally.
    #[arg(long)]
    pub fetch: bool,

    /// Skip pre-release versions when finding releases
    ///
    /// Filters out tags with pre-release identifiers (e.g., -beta, -rc, -alpha)
    /// when determining which release contains a commit.
    #[arg(short = 'S', long)]
    pub skip_prereleases: bool,

    /// Specific tag/release to check against
    ///
    /// If provided, checks whether the input (commit, PR, issue) is contained
    /// in this specific release/tag rather than finding the earliest release.
    #[arg(value_name = "RELEASE")]
    pub release: Option<String>,

    /// Print help information
    #[arg(short, long, action = clap::ArgAction::Help)]
    help: Option<bool>,
}

impl Cli {
    /// Parse the input and -r flag to determine the repository and query
    pub(crate) fn parse_input(&self) -> WtgResult<ParsedInput> {
        let input = self.input.as_ref().ok_or_else(|| WtgError::EmptyInput)?;

        try_parse_input(input, self.repo.as_deref())
    }
}
