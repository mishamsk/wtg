use clap::Parser;

use crate::{
    constants,
    parse_url::{ParsedInput, parse_github_repo_url, parse_github_url, sanitize_query},
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

    /// Print help information
    #[arg(short, long, action = clap::ArgAction::Help)]
    help: Option<bool>,
}

impl Cli {
    /// Parse the input and -r flag to determine the repository and query
    #[must_use]
    pub fn parse_input(&self) -> Option<ParsedInput> {
        let input = self.input.as_ref()?;

        // If -r flag is provided, use it as the repo and input as the query
        if let Some(repo_url) = &self.repo {
            let repo_info = parse_github_repo_url(repo_url)?;
            let query = sanitize_query(input)?;
            return Some(ParsedInput::new_with_remote(repo_info, query));
        }

        // Try to parse input as a GitHub URL
        if let Some(parsed) = parse_github_url(input) {
            return Some(parsed);
        }

        // Otherwise, it's just a query (local repo)
        sanitize_query(input).map(ParsedInput::new_local_query)
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;

    #[test]
    fn sanitizes_plain_query_inputs() {
        let cli = Cli {
            input: Some("   \n".into()),
            repo: Some("owner/repo".into()),
            help: None,
        };
        assert!(cli.parse_input().is_none());

        let cli = Cli {
            input: Some("  #99  ".into()),
            repo: Some("owner/repo".into()),
            help: None,
        };
        let parsed = cli.parse_input().unwrap();
        assert_eq!(parsed.query(), "#99");
    }
}
