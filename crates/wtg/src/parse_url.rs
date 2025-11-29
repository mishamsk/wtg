use url::Url;

use crate::github::GhRepoInfo;

/// Parsed input that can come from either the input argument or a GitHub URL
#[derive(Debug, Clone)]
pub struct ParsedInput {
    gh_repo_info: Option<GhRepoInfo>,
    query: String,
}

impl ParsedInput {
    pub fn new_with_remote(gh_repo_info: GhRepoInfo, query: impl Into<String>) -> Self {
        Self {
            gh_repo_info: Some(gh_repo_info),
            query: query.into(),
        }
    }

    pub fn new_local_query(query: impl Into<String>) -> Self {
        Self {
            gh_repo_info: None,
            query: query.into(),
        }
    }

    #[must_use]
    pub const fn gh_repo_info(&self) -> Option<&GhRepoInfo> {
        self.gh_repo_info.as_ref()
    }

    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.gh_repo_info
            .as_ref()
            .map(super::github::GhRepoInfo::owner)
    }

    #[must_use]
    pub fn repo(&self) -> Option<&str> {
        self.gh_repo_info
            .as_ref()
            .map(super::github::GhRepoInfo::repo)
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }
}

/// Parse a GitHub URL to extract owner, repo, and optional query
/// Supports:
/// - <https://github.com/owner/repo/commit/hash>
/// - <https://github.com/owner/repo/issues/123>
/// - <https://github.com/owner/repo/pull/123>
/// - <https://github.com/owner/repo/blob/branch/path/to/file>
/// - <`git@github.com:owner/repo/pull/9#discussion_r123`>
#[must_use]
pub fn parse_github_url(url: &str) -> Option<ParsedInput> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(segments) = parse_git_ssh_segments(trimmed) {
        return parsed_input_from_segments(&segments, false);
    }

    let (segments, is_api) = parse_http_github_segments(trimmed)?;
    parsed_input_from_segments(&segments, is_api)
}

/// Parse a simple GitHub repo URL or just owner/repo format
/// Supports:
/// - owner/repo
/// - <https://github.com/owner/repo.git>
/// - <https://github.com/owner/repo>
/// - <https://www.github.com/owner/repo>
/// - <https://api.github.com/repos/owner/repo>
/// - <git@github.com:owner/repo.git>
#[must_use]
pub fn parse_github_repo_url(url: &str) -> Option<GhRepoInfo> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(segments) = parse_git_ssh_segments(trimmed) {
        return owner_repo_from_segments(&segments, false);
    }

    if let Some((segments, is_api)) = parse_http_github_segments(trimmed)
        && let Some(owner_repo) = owner_repo_from_segments(&segments, is_api)
    {
        return Some(owner_repo);
    }

    // Handle simple owner/repo format
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() == 2
        && let (Some(owner), Some(repo)) = (
            sanitize_owner_repo_segment(parts[0]),
            sanitize_owner_repo_segment(parts[1].trim_end_matches(".git")),
        )
    {
        return Some(GhRepoInfo::new(owner, repo));
    }

    None
}

fn parse_http_github_segments(url: &str) -> Option<(Vec<String>, bool)> {
    let mut parsed = parse_with_https_fallback(url)?;
    let host = parsed.host_str()?;

    let is_api = match is_allowed_github_host(host) {
        GhUrlHostType::Github => false,
        GhUrlHostType::GithubApi => true,
        GhUrlHostType::Other => return None,
    };

    parsed.set_fragment(None);
    parsed.set_query(None);
    Some((collect_segments(parsed.path()), is_api))
}

/// Parse Git SSH URL format:
/// - `git@github.com:owner/repo/pull/9#discussion_r123`
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

enum GhUrlHostType {
    Github,
    GithubApi,
    Other,
}

fn is_allowed_github_host(host: &str) -> GhUrlHostType {
    let host = host.trim_start_matches("www.").to_ascii_lowercase();

    if host == "github.com" {
        return GhUrlHostType::Github;
    }

    if host == "api.github.com" {
        return GhUrlHostType::GithubApi;
    }

    GhUrlHostType::Other
}

fn collect_segments(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn owner_repo_from_segments(segments: &[String], is_api: bool) -> Option<GhRepoInfo> {
    let min_segments = if is_api { 3 } else { 2 };

    if segments.len() < min_segments {
        return None;
    }

    let owner_segment_index = usize::from(is_api);

    let owner = sanitize_owner_repo_segment(segments[owner_segment_index].as_str())?;
    let repo =
        sanitize_owner_repo_segment(segments[owner_segment_index + 1].trim_end_matches(".git"))?;
    Some(GhRepoInfo::new(owner, repo))
}

fn parsed_input_from_segments(segments: &[String], is_api: bool) -> Option<ParsedInput> {
    if segments.len() < 3 {
        return None;
    }

    let repo_info = owner_repo_from_segments(segments, is_api)?;
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

    Some(ParsedInput::new_with_remote(repo_info, query))
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

pub fn sanitize_query(raw: &str) -> Option<String> {
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
    use super::{parse_github_repo_url, parse_github_url};

    fn assert_issue_or_pr(url: &str, expected_query: &str) {
        let parsed = parse_github_url(url).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some("owner"));
        assert_eq!(parsed.repo(), Some("repo"));
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
    fn parses_git_repo_urls() {
        let repo_info = parse_github_repo_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(repo_info.owner(), "owner");
        assert_eq!(repo_info.repo(), "repo");

        let repo_info = parse_github_repo_url("https://api.github.com/repos/owner/repo").unwrap();
        assert_eq!(repo_info.owner(), "owner");
        assert_eq!(repo_info.repo(), "repo");
    }

    #[test]
    fn parses_git_ssh_urls() {
        let parsed = parse_github_url("git@github.com:owner/repo/pull/9#discussion_r123").unwrap();
        assert_eq!(parsed.owner(), Some("owner"));
        assert_eq!(parsed.repo(), Some("repo"));
        assert_eq!(parsed.query, "#9");

        let repo_info = parse_github_repo_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(repo_info.owner(), "owner");
        assert_eq!(repo_info.repo(), "repo");
    }

    #[test]
    fn rejects_malformed_owner_repo_segments() {
        assert!(parse_github_repo_url("owner space/repo").is_none());
        assert!(parse_github_repo_url("owner/repo~").is_none());
        assert!(parse_github_url("https://github.com/owner space/repo/issues/1").is_none());
    }
}
