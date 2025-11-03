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

/// Parsed input that can come from either the input argument or a GitHub URL
#[derive(Debug, Clone)]
pub struct ParsedInput {
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub query: String,
}

impl Cli {
    /// Parse the input and -r flag to determine the repository and query
    #[must_use]
    pub fn parse_input(&self) -> Option<ParsedInput> {
        let input = self.input.as_ref()?;

        // If -r flag is provided, use it as the repo and input as the query
        if let Some(repo_url) = &self.repo {
            let (owner, repo) = parse_github_repo_url(repo_url)?;
            return Some(ParsedInput {
                owner: Some(owner),
                repo: Some(repo),
                query: input.clone(),
            });
        }

        // Try to parse input as a GitHub URL
        if let Some(parsed) = parse_github_url(input) {
            return Some(parsed);
        }

        // Otherwise, it's just a query (local repo)
        Some(ParsedInput {
            owner: None,
            repo: None,
            query: input.clone(),
        })
    }
}

/// Parse a GitHub URL to extract owner, repo, and optional query
/// Supports:
/// - <https://github.com/owner/repo>
/// - <https://github.com/owner/repo/commit/hash>
/// - <https://github.com/owner/repo/issues/123>
/// - <https://github.com/owner/repo/pull/123>
/// - <https://github.com/owner/repo/blob/branch/path/to/file>
fn parse_github_url(url: &str) -> Option<ParsedInput> {
    if !url.contains("github.com") {
        return None;
    }

    // Extract the path after github.com
    let path = if url.starts_with("git@") {
        // SSH format: git@github.com:owner/repo.git
        url.split(':').nth(1)?
    } else {
        // HTTPS format: https://github.com/owner/repo/...
        url.split("github.com/").nth(1)?
    };

    let path = path.trim_end_matches(".git");
    let parts: Vec<&str> = path.split('/').collect();

    if parts.len() < 2 {
        return None;
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();

    // Determine what the query should be based on the URL structure
    let query = if parts.len() == 2 {
        // Just the repo URL, no specific query
        return None;
    } else if parts.len() >= 4 {
        match parts[2] {
            "commit" => parts[3].to_string(),
            "issues" | "pull" => format!("#{}", parts[3]),
            "blob" | "tree" => {
                // Format: /blob/branch/path/to/file or /tree/branch/path/to/file
                // Extract the file path after branch name
                if parts.len() >= 5 {
                    parts[4..].join("/")
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    } else {
        return None;
    };

    Some(ParsedInput {
        owner: Some(owner),
        repo: Some(repo),
        query,
    })
}

/// Parse a simple GitHub repo URL (owner/repo or <https://github.com/owner/repo>)
fn parse_github_repo_url(url: &str) -> Option<(String, String)> {
    // Handle SSH format
    if url.starts_with("git@") {
        let path = url.split(':').nth(1)?;
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
        return None;
    }

    // Handle HTTPS format
    if url.contains("github.com/") {
        let path = url.split("github.com/").nth(1)?;
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
        return None;
    }

    // Handle simple owner/repo format
    let parts: Vec<&str> = url.split('/').collect();
    if parts.len() == 2 {
        return Some((parts[0].to_string(), parts[1].to_string()));
    }

    None
}
