use std::path::{Path, PathBuf};

use url::Url;

use crate::github::GhRepoInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Query {
    /// A Git commit hash
    GitCommit(String),
    /// Either a GitHub issue or a pull request number
    IssueOrPr(u64),
    /// A GitHub issue number
    Issue(u64),
    /// A GitHub pull request number
    Pr(u64),
    /// A file path within the repository
    FilePath(PathBuf),
    /// Unknown query type
    Unknown(String),
}

/// Parsed input that can come from either the input argument or a GitHub URL
#[derive(Debug, Clone)]
pub(crate) struct ParsedInput {
    gh_repo_info: Option<GhRepoInfo>,
    query: Query,
}

impl ParsedInput {
    const fn new_with_remote(gh_repo_info: GhRepoInfo, query: Query) -> Self {
        Self {
            gh_repo_info: Some(gh_repo_info),
            query,
        }
    }

    const fn new_local_query(query: Query) -> Self {
        Self {
            gh_repo_info: None,
            query,
        }
    }

    #[must_use]
    pub(crate) const fn gh_repo_info(&self) -> Option<&GhRepoInfo> {
        self.gh_repo_info.as_ref()
    }

    // #[must_use]
    // pub(crate) fn query(&self) -> &Query {
    //     &self.query
    // }

    /// Convert query to string for legacy identifier code
    /// This is a temporary bridge until identifier is refactored to use Query directly
    #[must_use]
    pub(crate) fn query_as_string(&self) -> String {
        match &self.query {
            Query::GitCommit(hash) => hash.clone(),
            Query::IssueOrPr(num) | Query::Issue(num) | Query::Pr(num) => format!("#{num}"),
            Query::FilePath(path) => path.to_string_lossy().to_string(),
            Query::Unknown(s) => s.clone(),
        }
    }

    #[cfg(test)]
    #[must_use]
    fn owner(&self) -> Option<&str> {
        self.gh_repo_info.as_ref().map(GhRepoInfo::owner)
    }

    #[cfg(test)]
    #[must_use]
    fn repo(&self) -> Option<&str> {
        self.gh_repo_info.as_ref().map(GhRepoInfo::repo)
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
fn try_parse_input_from_github_url(url: &str) -> Option<ParsedInput> {
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

#[must_use]
fn try_parse_input_str(raw_input: &str) -> Option<Query> {
    // Sanitize input
    let input = sanitize_query(raw_input)?;

    // If it starts with a '#', try to parse as issue or PR number
    if let Some(stripped) = input.strip_prefix('#')
        && let Ok(number) = stripped.parse()
    {
        return Some(Query::IssueOrPr(number));
    }

    // Otherwise we have to treat as unknown, since path & branches
    // may look the same, and other git refs may be indistinguishable
    // from commit hashes without querying the repo
    Some(Query::Unknown(input))
}

pub(crate) fn try_parse_input(raw_input: &str, repo_url: Option<&str>) -> Option<ParsedInput> {
    // If repo url is explicitly provided, use it as the repo and input as the query
    if let Some(repo_url) = repo_url {
        let repo_info = parse_github_repo_url(repo_url)?;
        let query = try_parse_input_str(raw_input)?;
        return Some(ParsedInput::new_with_remote(repo_info, query));
    }

    // Otherwise, try to parse input as a GitHub URL
    if let Some(parsed) = try_parse_input_from_github_url(raw_input) {
        return Some(parsed);
    }

    // And finally, treat as a local query
    try_parse_input_str(raw_input).map(ParsedInput::new_local_query)
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
pub(crate) fn parse_github_repo_url(url: &str) -> Option<GhRepoInfo> {
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
    split_url_segments(segments, is_api).map(|(repo_info, _)| repo_info)
}

fn split_url_segments(segments: &[String], is_api: bool) -> Option<(GhRepoInfo, &[String])> {
    let min_segments = if is_api { 3 } else { 2 };

    if segments.len() < min_segments {
        return None;
    }

    let owner_segment_index = usize::from(is_api);

    let owner = sanitize_owner_repo_segment(segments[owner_segment_index].as_str())?;
    let repo =
        sanitize_owner_repo_segment(segments[owner_segment_index + 1].trim_end_matches(".git"))?;
    Some((
        GhRepoInfo::new(owner, repo),
        &segments[owner_segment_index + 2..],
    ))
}

fn parsed_input_from_segments(segments: &[String], is_api: bool) -> Option<ParsedInput> {
    let (repo_info, segments) = split_url_segments(segments, is_api)?;

    let query = match segments.first()?.as_str() {
        "commit" => Query::GitCommit(sanitize_query(segments.get(1)?)?),
        "issues" => Query::Issue((segments.get(1)?).parse().ok()?),
        "pull" => Query::Pr((segments.get(1)?).parse().ok()?),
        // File path will start from segment index 2, e.g., /blob/branch/path/to/file
        "blob" | "tree" if segments.len() >= 2 => {
            // TODO: this is not correct when branch names contain slashes. Deterministically
            // resolving branch vs path requires API calls.
            let path = segments[2..]
                .iter()
                .map(|s| sanitize_query(s))
                .fold(PathBuf::new(), |path, seg| {
                    path.join(seg.unwrap_or_default())
                });

            // Do a security check on the path
            if !check_path(&path) {
                return None;
            }

            Query::FilePath(path)
        }
        _ => return None,
    };

    Some(ParsedInput::new_with_remote(repo_info, query))
}

/// Sanitize owner or repo segment by trimming whitespace and allowing only certain characters
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

/// Sanitize a query string by trimming whitespace and rejecting control characters
pub(crate) fn sanitize_query(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.chars().any(char::is_control) {
        return None;
    }

    Some(trimmed.to_string())
}

/// Checks whether the given path is valid & safe to use
pub(crate) fn check_path(path: &Path) -> bool {
    // We may have an empty path after sanitation, or it may be
    // absolute, or contain parent components - reject those
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::PathBuf;

    // ========================================================================
    // Helper Types for Flexible Test Assertions
    // ========================================================================

    /// Helper enum to allow flexible query matching in tests
    enum QueryMatcher {
        Exact(Query),
        Commit(String),
    }

    impl From<Query> for QueryMatcher {
        fn from(q: Query) -> Self {
            Self::Exact(q)
        }
    }

    impl From<&str> for QueryMatcher {
        fn from(s: &str) -> Self {
            Self::Commit(s.to_string())
        }
    }

    impl QueryMatcher {
        fn assert_matches(&self, actual: &Query) {
            match self {
                Self::Exact(expected) => assert_eq!(actual, expected),
                Self::Commit(hash) => {
                    assert_eq!(actual, &Query::GitCommit(hash.clone()));
                }
            }
        }
    }

    // ========================================================================
    // Local & URL Parsing Tests
    // ========================================================================

    #[rstest]
    #[case::basic_issue(
        "https://github.com/owner/repo/issues/42",
        "owner",
        "repo",
        Query::Issue(42)
    )]
    #[case::issue_with_comment(
        "https://github.com/owner/repo/issues/42#issuecomment-123456",
        "owner",
        "repo",
        Query::Issue(42)
    )]
    #[case::issue_with_query(
        "https://github.com/owner/repo/issues/42?tab=comments",
        "owner",
        "repo",
        Query::Issue(42)
    )]
    #[case::issue_large_number(
        "https://github.com/owner/repo/issues/999999",
        "owner",
        "repo",
        Query::Issue(999_999)
    )]
    fn parses_github_issue_urls(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_query: Query,
    ) {
        let parsed = try_parse_input(url, None).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, expected_query);
    }

    #[rstest]
    #[case::basic_pr("https://github.com/owner/repo/pull/7", "owner", "repo", Query::Pr(7))]
    #[case::pr_files(
        "https://github.com/owner/repo/pull/7/files",
        "owner",
        "repo",
        Query::Pr(7)
    )]
    #[case::pr_files_diff(
        "https://github.com/owner/repo/pull/7/files?diff=split",
        "owner",
        "repo",
        Query::Pr(7)
    )]
    #[case::pr_discussion(
        "https://github.com/owner/repo/pull/7#discussion_r987654321",
        "owner",
        "repo",
        Query::Pr(7)
    )]
    #[case::pr_comment(
        "https://github.com/owner/repo/pull/7#issuecomment-abcdef",
        "owner",
        "repo",
        Query::Pr(7)
    )]
    #[case::pr_large_number(
        "https://github.com/owner/repo/pull/123456",
        "owner",
        "repo",
        Query::Pr(123_456)
    )]
    fn parses_github_pr_urls(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_query: Query,
    ) {
        let parsed = try_parse_input(url, None).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, expected_query);
    }

    #[rstest]
    #[case::full_hash(
        "https://github.com/owner/repo/commit/abc123def456",
        "owner",
        "repo",
        "abc123def456"
    )]
    #[case::short_hash(
        "https://github.com/owner/repo/commit/abc123d",
        "owner",
        "repo",
        "abc123d"
    )]
    #[case::commit_with_fragment(
        "https://github.com/owner/repo/commit/abc123#diff-1",
        "owner",
        "repo",
        "abc123"
    )]
    fn parses_github_commit_urls(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_hash: &str,
    ) {
        let parsed = try_parse_input(url, None).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, Query::GitCommit(expected_hash.to_string()));
    }

    #[rstest]
    #[case::blob_single_file(
        "https://github.com/owner/repo/blob/main/README.md",
        "owner",
        "repo",
        "README.md"
    )]
    #[case::blob_deep_nesting(
        "https://github.com/owner/repo/blob/main/a/b/c/d.txt",
        "owner",
        "repo",
        "a/b/c/d.txt"
    )]
    #[case::tree_directory("https://github.com/owner/repo/tree/main/src", "owner", "repo", "src")]
    /// TODO: this is obvisouly wrong, but deterministically resolving branch vs path requires API calls
    #[case::tree_nested_branch(
        "https://github.com/owner/repo/tree/feat/new-feature/docs/api",
        "owner",
        "repo",
        "new-feature/docs/api"
    )]
    fn parses_github_file_urls(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_path: &str,
    ) {
        let parsed =
            try_parse_input_from_github_url(url).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, Query::FilePath(PathBuf::from(expected_path)));
    }

    #[rstest]
    #[case::no_scheme("github.com/owner/repo/issues/101", "owner", "repo", Query::Issue(101))]
    #[case::no_scheme_with_comment(
        "github.com/owner/repo/issues/101#issuecomment-1",
        "owner",
        "repo",
        Query::Issue(101)
    )]
    #[case::scheme_only("//github.com/owner/repo/pull/15", "owner", "repo", Query::Pr(15))]
    #[case::scheme_only_with_query(
        "//github.com/owner/repo/pull/15?tab=commits",
        "owner",
        "repo",
        Query::Pr(15)
    )]
    #[case::www_prefix(
        "https://www.github.com/owner/repo/pull/7",
        "owner",
        "repo",
        Query::Pr(7)
    )]
    #[case::www_with_fragment(
        "https://www.github.com/owner/repo/pull/7#discussion_r42",
        "owner",
        "repo",
        Query::Pr(7)
    )]
    fn parses_alternate_github_url_formats(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_query: Query,
    ) {
        let parsed = try_parse_input(url, None).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, expected_query);
    }

    #[rstest]
    #[case::basic_ssh("git@github.com:owner/repo/pull/9", "owner", "repo", Query::Pr(9))]
    #[case::ssh_with_fragment(
        "git@github.com:owner/repo/pull/9#discussion_r123",
        "owner",
        "repo",
        Query::Pr(9)
    )]
    #[case::ssh_issue(
        "git@github.com:owner/repo/issues/42",
        "owner",
        "repo",
        Query::Issue(42)
    )]
    #[case::ssh_commit("git@github.com:owner/repo/commit/abc123", "owner", "repo", "abc123")]
    fn parses_github_ssh_urls(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_query: impl Into<QueryMatcher>,
    ) {
        let parsed = try_parse_input(url, None).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        expected_query.into().assert_matches(&parsed.query);
    }

    #[rstest]
    #[case::api_issue(
        "https://api.github.com/repos/owner/repo/issues/42",
        "owner",
        "repo",
        Query::Issue(42)
    )]
    fn parses_github_api_urls(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_query: Query,
    ) {
        let parsed = try_parse_input(url, None).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, expected_query);
    }

    #[rstest]
    #[case::hash_with_prefix("#42", Query::IssueOrPr(42))]
    #[case::hash_without_prefix("42", Query::Unknown("42".to_string()))]
    #[case::hash_with_whitespace("  #99  ", Query::IssueOrPr(99))]
    #[case::short_hash("abc123d", Query::Unknown("abc123d".to_string()))]
    #[case::hash_with_whitespace("  abc123  ", Query::Unknown("abc123".to_string()))]
    #[case::simple_tag("v1.0.0", Query::Unknown("v1.0.0".to_string()))]
    #[case::simple_file("README.md", Query::Unknown("README.md".to_string()))]
    #[case::nested_file("src/lib.rs", Query::Unknown("src/lib.rs".to_string()))]
    #[case::unicode_path("src/файл.rs", Query::Unknown("src/файл.rs".to_string()))]
    #[case::unicode_tag("версия-1.0", Query::Unknown("версия-1.0".to_string()))]
    #[case::emoji_in_path("src/👍.md", Query::Unknown("src/👍.md".to_string()))]
    fn parses_local_inputs(#[case] input: &str, #[case] expected: Query) {
        let parsed = try_parse_input(input, None).expect("Should parse issue/PR number");
        assert_eq!(parsed.query, expected);
        assert!(parsed.gh_repo_info().is_none());
    }

    // ========================================================================
    // Repository URL Parsing Tests
    // ========================================================================

    #[rstest]
    #[case::simple_format("owner/repo", "owner", "repo")]
    #[case::with_dash("my-org/my-repo", "my-org", "my-repo")]
    #[case::with_underscore("my_org/my_repo", "my_org", "my_repo")]
    #[case::with_dot("my.org/my.repo", "my.org", "my.repo")]
    #[case::mixed_separators("my-org_test/repo.name-2", "my-org_test", "repo.name-2")]
    fn parses_simple_owner_repo_format(
        #[case] input: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
    ) {
        let parsed = try_parse_input("dummy", Some(input))
            .unwrap_or_else(|| panic!("failed to parse {input}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, Query::Unknown("dummy".to_string()));
    }

    #[rstest]
    #[case::https("https://github.com/owner/repo", "owner", "repo")]
    #[case::https_with_git("https://github.com/owner/repo.git", "owner", "repo")]
    #[case::https_www("https://www.github.com/owner/repo", "owner", "repo")]
    #[case::api_repos("https://api.github.com/repos/owner/repo", "owner", "repo")]
    #[case::ssh("git@github.com:owner/repo", "owner", "repo")]
    #[case::ssh_with_git("git@github.com:owner/repo.git", "owner", "repo")]
    fn parses_various_repo_url_formats(
        #[case] url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
    ) {
        let parsed =
            try_parse_input("dummy", Some(url)).unwrap_or_else(|| panic!("failed to parse {url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, Query::Unknown("dummy".to_string()));
    }

    // ========================================================================
    // Combined Parsing Tests (try_parse_input)
    // ========================================================================

    #[rstest]
    #[case::issue_with_repo("#42", "owner/repo", "owner", "repo", Query::IssueOrPr(42))]
    #[case::hash_with_repo("abc123", "owner/repo", "owner", "repo", Query::Unknown("abc123".to_string()))]
    #[case::file_with_repo("src/lib.rs", "https://github.com/owner/repo", "owner", "repo", Query::Unknown("src/lib.rs".to_string()))]
    fn parses_input_with_explicit_repo(
        #[case] input: &str,
        #[case] repo_url: &str,
        #[case] expected_owner: &str,
        #[case] expected_repo: &str,
        #[case] expected_query: Query,
    ) {
        let parsed = try_parse_input(input, Some(repo_url))
            .unwrap_or_else(|| panic!("failed to parse {input} with repo {repo_url}"));
        assert_eq!(parsed.owner(), Some(expected_owner));
        assert_eq!(parsed.repo(), Some(expected_repo));
        assert_eq!(parsed.query, expected_query);
    }

    // ========================================================================
    // Rejection Tests (Negative Cases)
    // ========================================================================

    #[rstest]
    #[case::owner_with_space("https://github.com/owner space/repo/issues/1")]
    #[case::repo_with_space("https://github.com/owner/repo space/issues/1")]
    #[case::owner_with_tilde("https://github.com/owner~/repo/issues/1")]
    #[case::repo_with_tilde("https://github.com/owner/repo~/issues/1")]
    #[case::empty_owner("https://github.com//repo/issues/1")]
    #[case::empty_repo("https://github.com/owner//issues/1")]
    #[case::whitespace_owner("https://github.com/   /repo/issues/1")]
    fn rejects_malformed_github_urls(#[case] url: &str) {
        let parsed = try_parse_input(url, None);
        assert!(
            parsed.is_none(),
            "Should reject malformed URL: {url}. Got {parsed:?}"
        );
    }

    #[rstest]
    #[case::parent_traversal("https://github.com/owner/repo/blob/main/../../../etc/passwd")]
    #[case::parent_in_middle("https://github.com/owner/repo/blob/main/src/../../../etc/passwd")]
    #[case::absolute_path("/etc/passwd")]
    #[case::parent_traversal("../../../etc/passwd")]
    #[case::parent_in_path("src/../../../etc/passwd")]
    fn rejects_unsafe_file_paths_in_urls_and_local(#[case] input: &str) {
        let parsed = try_parse_input(input, None);
        assert!(
            parsed.is_none(),
            "Should reject unsafe path in: {input}. Got {parsed:?}"
        );
    }

    #[rstest]
    #[case::empty_string("")]
    #[case::whitespace_only("   ")]
    #[case::newlines_only("\n\n")]
    #[case::tabs_only("\t\t")]
    fn rejects_empty_url_inputs(#[case] url: &str) {
        let parsed = try_parse_input(url, None);
        assert!(
            parsed.is_none(),
            "Should reject empty input: {url:?}. Got {parsed:?}"
        );
    }

    #[rstest]
    #[case::null_byte("test\0data")]
    #[case::newline_in_middle("test\ndata")]
    #[case::carriage_return("test\rdata")]
    #[case::tab_in_middle("test\tdata")]
    fn rejects_control_characters(#[case] input: &str) {
        let parsed = try_parse_input(input, None);
        assert!(
            parsed.is_none(),
            "Should reject input with control chars: {input:?}. Got {parsed:?}"
        );
    }

    #[rstest]
    #[case::owner_with_space("owner space/repo")]
    #[case::repo_with_space("owner/repo space")]
    #[case::owner_with_tilde("owner~/repo")]
    #[case::repo_with_tilde("owner/repo~")]
    #[case::owner_with_bang("owner!/repo")]
    #[case::too_many_slashes("owner/repo/extra")]
    #[case::single_segment("justowner")]
    #[case::empty_owner("/repo")]
    #[case::empty_repo("owner/")]
    #[case::empty_string("")]
    #[case::whitespace_only("   ")]
    fn rejects_malformed_repo_urls(#[case] input: &str) {
        let parsed = try_parse_input("dummy", Some(input));

        assert!(
            parsed.is_none(),
            "Should reject malformed repo URL: {input}. Got {parsed:?}"
        );
    }
}
