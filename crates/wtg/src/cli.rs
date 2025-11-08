use clap::Parser;
use url::Url;

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
            let query = sanitize_query(input)?;
            return Some(ParsedInput {
                owner: Some(owner),
                repo: Some(repo),
                query,
            });
        }

        // Try to parse input as a GitHub URL
        if let Some(parsed) = parse_github_url(input) {
            return Some(parsed);
        }

        // Otherwise, it's just a query (local repo)
        sanitize_query(input).map(|query| ParsedInput {
            owner: None,
            repo: None,
            query,
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
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(segments) = parse_git_ssh_segments(trimmed) {
        return parsed_input_from_segments(&segments);
    }

    let segments = parse_http_github_segments(trimmed)?;
    parsed_input_from_segments(&segments)
}

/// Parse a simple GitHub repo URL (owner/repo or <https://github.com/owner/repo>)
fn parse_github_repo_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(segments) = parse_git_ssh_segments(trimmed) {
        return owner_repo_from_segments(&segments);
    }

    if let Some(mut parsed) = parse_with_https_fallback(trimmed) {
        let host = parsed.host_str()?;
        if !is_allowed_github_host(host) {
            return None;
        }
        parsed.set_fragment(None);
        parsed.set_query(None);
        let segments = collect_segments(parsed.path());
        if let Some(owner_repo) = owner_repo_from_segments(&segments) {
            return Some(owner_repo);
        }
    }

    // Handle simple owner/repo format
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() == 2
        && let (Some(owner), Some(repo)) = (
            sanitize_owner_repo_segment(parts[0]),
            sanitize_owner_repo_segment(parts[1].trim_end_matches(".git")),
        )
    {
        return Some((owner, repo));
    }

    None
}

fn parse_http_github_segments(url: &str) -> Option<Vec<String>> {
    let mut parsed = parse_with_https_fallback(url)?;
    let host = parsed.host_str()?;
    if !is_allowed_github_host(host) {
        return None;
    }
    parsed.set_fragment(None);
    parsed.set_query(None);
    Some(collect_segments(parsed.path()))
}

fn parse_git_ssh_segments(url: &str) -> Option<Vec<String>> {
    let normalized = url.trim();
    if !normalized.starts_with("git@github.com:") {
        return None;
    }
    let path = normalized.split(':').nth(1)?;
    let path = path.split('#').next().unwrap_or(path);
    let path = path.split('?').next().unwrap_or(path);
    Some(collect_segments(path))
}

fn parse_with_https_fallback(input: &str) -> Option<Url> {
    Url::parse(input).map_or_else(
        |_| {
            let lower = input.to_ascii_lowercase();
            if lower.starts_with("github.com/") || lower.starts_with("www.github.com/") {
                Url::parse(&format!("https://{input}")).ok()
            } else if lower.starts_with("//github.com/") {
                Url::parse(&format!("https:{input}")).ok()
            } else {
                None
            }
        },
        Some,
    )
}

fn is_allowed_github_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "github.com" | "www.github.com"
    )
}

fn collect_segments(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn owner_repo_from_segments(segments: &[String]) -> Option<(String, String)> {
    if segments.len() < 2 {
        return None;
    }
    let owner = sanitize_owner_repo_segment(segments[0].as_str())?;
    let repo = sanitize_owner_repo_segment(segments[1].trim_end_matches(".git"))?;
    Some((owner, repo))
}

fn parsed_input_from_segments(segments: &[String]) -> Option<ParsedInput> {
    if segments.len() < 3 {
        return None;
    }

    let (owner, repo) = owner_repo_from_segments(segments)?;
    let query = match segments.get(2)?.as_str() {
        "commit" => segments.get(3)?.clone(),
        "issues" | "pull" => format!("#{}", segments.get(3)?),
        "blob" | "tree" => {
            if segments.len() >= 5 {
                segments[4..].join("/")
            } else {
                return None;
            }
        }
        _ => return None,
    };

    let query = sanitize_query(&query)?;

    Some(ParsedInput {
        owner: Some(owner),
        repo: Some(repo),
        query,
    })
}

fn sanitize_owner_repo_segment(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn sanitize_query(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.chars().any(char::is_control) {
        return None;
    }

    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Cli, parse_github_repo_url, parse_github_url};

    fn assert_issue_or_pr(url: &str, expected_query: &str) {
        let parsed = parse_github_url(url).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner.as_deref(), Some("owner"));
        assert_eq!(parsed.repo.as_deref(), Some("repo"));
        assert_eq!(parsed.query, expected_query);
    }

    #[test]
    fn parses_issue_urls_with_fragments_and_queries() {
        let urls = [
            "https://github.com/owner/repo/issues/42",
            "https://github.com/owner/repo/issues/42#issuecomment-123456",
            "https://github.com/owner/repo/issues/42?tab=comments",
        ];

        for url in urls {
            assert_issue_or_pr(url, "#42");
        }
    }

    #[test]
    fn parses_pr_urls_with_files_views_and_comments() {
        let urls = [
            "https://github.com/owner/repo/pull/7",
            "https://github.com/owner/repo/pull/7/files",
            "https://github.com/owner/repo/pull/7/files?diff=split",
            "https://github.com/owner/repo/pull/7#discussion_r987654321",
            "https://github.com/owner/repo/pull/7#issuecomment-abcdef",
        ];

        for url in urls {
            assert_issue_or_pr(url, "#7");
        }
    }

    #[test]
    fn parses_www_and_scheme_less_urls() {
        let urls = [
            "github.com/owner/repo/issues/101#issuecomment-1",
            "//github.com/owner/repo/pull/15?tab=commits",
            "https://www.github.com/owner/repo/pull/7#discussion_r42",
        ];

        assert_issue_or_pr(urls[0], "#101");
        assert_issue_or_pr(urls[1], "#15");
        assert_issue_or_pr(urls[2], "#7");
    }

    #[test]
    fn parses_git_ssh_urls() {
        let parsed = parse_github_url("git@github.com:owner/repo/pull/9#discussion_r123").unwrap();
        assert_eq!(parsed.owner.as_deref(), Some("owner"));
        assert_eq!(parsed.repo.as_deref(), Some("repo"));
        assert_eq!(parsed.query, "#9");

        let repo = parse_github_repo_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(repo.0, "owner");
        assert_eq!(repo.1, "repo");
    }

    #[test]
    fn rejects_malformed_owner_repo_segments() {
        assert!(parse_github_repo_url("owner space/repo").is_none());
        assert!(parse_github_repo_url("owner/repo~").is_none());
        assert!(parse_github_url("https://github.com/owner space/repo/issues/1").is_none());
    }

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
        assert_eq!(parsed.query, "#99");
    }
}
