use chrono::{DateTime, Utc};
use octocrab::{
    Octocrab, OctocrabBuilder, Result as OctoResult,
    models::{
        Event as TimelineEventType, commits::GithubCommitStatus, repos::RepoCommit,
        timelines::TimelineEvent,
    },
};
use serde::Deserialize;
use std::{future::Future, pin::Pin, sync::LazyLock, time::Duration};

use crate::{
    error::{WtgError, WtgResult},
    git::{CommitInfo, TagInfo, parse_semver},
    parse_url::parse_github_repo_url,
};

impl From<RepoCommit> for CommitInfo {
    fn from(commit: RepoCommit) -> Self {
        let message = commit.commit.message;
        let message_lines = message.lines().count();

        let author_name = commit
            .commit
            .author
            .as_ref()
            .map_or_else(|| "Unknown".to_string(), |a| a.name.clone());

        let author_email = commit.commit.author.as_ref().and_then(|a| a.email.clone());

        let commit_url = commit.html_url;

        let (author_login, author_url) = commit
            .author
            .map(|author| (Some(author.login), Some(author.html_url.into())))
            .unwrap_or_default();

        let (date, timestamp) = commit
            .commit
            .author
            .as_ref()
            .and_then(|a| a.date.as_ref())
            .map_or_else(
                || (String::new(), 0),
                |date_time| {
                    let date = date_time.format("%Y-%m-%d %H:%M:%S").to_string();
                    let timestamp = date_time.timestamp();
                    (date, timestamp)
                },
            );

        let full_hash = commit.sha;

        Self {
            hash: full_hash.clone(),
            short_hash: full_hash[..7.min(full_hash.len())].to_string(),
            message: message.lines().next().unwrap_or("").to_string(),
            message_lines,
            commit_url: Some(commit_url),
            author_name,
            author_email,
            author_login,
            author_url,
            date,
            timestamp,
        }
    }
}

const CONNECT_TIMEOUT_SECS: u64 = 5;
const READ_TIMEOUT_SECS: u64 = 30;
const REQUEST_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Deserialize)]
struct GhConfig {
    #[serde(rename = "github.com")]
    github_com: GhHostConfig,
}

#[derive(Debug, Deserialize)]
struct GhHostConfig {
    oauth_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GhRepoInfo {
    owner: String,
    repo: String,
}

impl GhRepoInfo {
    #[must_use]
    pub const fn new(owner: String, repo: String) -> Self {
        Self { owner, repo }
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub fn repo(&self) -> &str {
        &self.repo
    }
}

/// GitHub API client wrapper.
///
/// - Provides a simplified interface for common GitHub operations used in wtg over direct octocrab usage.
/// - Handles authentication via `GITHUB_TOKEN` env var or gh CLI config.
/// - Supports fallback to anonymous requests when auth fails.
/// - Converts known octocrab errors into `WtgError` variants.
#[derive(Debug)]
pub struct GitHubClient {
    auth_client: Option<Octocrab>,
    anonymous_client: LazyLock<Option<Octocrab>>,
}

/// Information about a Pull Request
#[derive(Debug, Clone)]
pub struct PullRequestInfo {
    pub number: u64,
    pub repo_info: Option<GhRepoInfo>,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub url: String,
    pub merged: bool,
    pub merge_commit_sha: Option<String>,
    pub author: Option<String>,
    pub author_url: Option<String>,
    pub created_at: Option<String>, // When the PR was created
}

impl From<octocrab::models::pulls::PullRequest> for PullRequestInfo {
    fn from(pr: octocrab::models::pulls::PullRequest) -> Self {
        let author = pr.user.as_ref().map(|u| u.login.clone());
        let author_url = pr.user.as_ref().map(|u| u.html_url.to_string());
        let created_at = pr.created_at.map(|dt| dt.to_string());

        Self {
            number: pr.number,
            repo_info: parse_github_repo_url(pr.url.as_str()),
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            state: format!("{:?}", pr.state),
            url: pr.html_url.map(|u| u.to_string()).unwrap_or_default(),
            merged: pr.merged.unwrap_or(false),
            merge_commit_sha: pr.merge_commit_sha,
            author,
            author_url,
            created_at,
        }
    }
}

/// Information about an Issue
#[derive(Debug, Clone)]
pub struct ExtendedIssueInfo {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: octocrab::models::IssueState,
    pub url: String,
    pub author: Option<String>,
    pub author_url: Option<String>,
    pub closing_prs: Vec<PullRequestInfo>, // PRs that closed this issue (may be cross-repo)
    pub created_at: Option<DateTime<Utc>>, // When the issue was created
}

impl TryFrom<octocrab::models::issues::Issue> for ExtendedIssueInfo {
    type Error = ();

    fn try_from(issue: octocrab::models::issues::Issue) -> Result<Self, Self::Error> {
        // If it has a pull_request field, it's actually a PR - reject it
        if issue.pull_request.is_some() {
            return Err(());
        }

        let author = issue.user.login.clone();
        let author_url = Some(issue.user.html_url.to_string());
        let created_at = Some(issue.created_at);

        Ok(Self {
            number: issue.number,
            title: issue.title,
            body: issue.body,
            state: issue.state,
            url: issue.html_url.to_string(),
            author: Some(author),
            author_url,
            closing_prs: Vec::new(), // Will be populated by caller if needed
            created_at,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubClient {
    /// Create a new GitHub client with authentication
    #[must_use]
    pub fn new() -> Self {
        let auth_client = Self::build_auth_client();

        Self {
            auth_client,
            anonymous_client: LazyLock::new(Self::build_anonymous_client),
        }
    }

    /// Build an authenticated octocrab client
    fn build_auth_client() -> Option<Octocrab> {
        // Set reasonable timeouts: 5s connect, 30s read/write
        let connect_timeout = Some(Self::connect_timeout());
        let read_timeout = Some(Self::read_timeout());

        // Try GITHUB_TOKEN env var first
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            return OctocrabBuilder::new()
                .personal_token(token)
                .set_connect_timeout(connect_timeout)
                .set_read_timeout(read_timeout)
                .build()
                .ok();
        }

        // Try reading from gh CLI config
        if let Some(token) = Self::read_gh_config() {
            return OctocrabBuilder::new()
                .personal_token(token)
                .set_connect_timeout(connect_timeout)
                .set_read_timeout(read_timeout)
                .build()
                .ok();
        }

        None
    }

    /// Build an anonymous octocrab client (no authentication)
    fn build_anonymous_client() -> Option<Octocrab> {
        let connect_timeout = Some(Self::connect_timeout());
        let read_timeout = Some(Self::read_timeout());

        OctocrabBuilder::new()
            .set_connect_timeout(connect_timeout)
            .set_read_timeout(read_timeout)
            .build()
            .ok()
    }

    /// Read GitHub token from gh CLI config (cross-platform)
    fn read_gh_config() -> Option<String> {
        // gh CLI follows XDG conventions and stores config in:
        // - Unix/macOS: ~/.config/gh/hosts.yml
        // - Windows: %APPDATA%/gh/hosts.yml (but dirs crate handles this)

        // Try XDG-style path first (~/.config/gh/hosts.yml)
        if let Some(home) = dirs::home_dir() {
            let xdg_path = home.join(".config").join("gh").join("hosts.yml");
            if let Ok(content) = std::fs::read_to_string(&xdg_path)
                && let Ok(config) = serde_yaml::from_str::<GhConfig>(&content)
                && let Some(token) = config.github_com.oauth_token
            {
                return Some(token);
            }
        }

        // Fall back to platform-specific config dir
        // (~/Library/Application Support/gh/hosts.yml on macOS)
        if let Some(mut config_path) = dirs::config_dir() {
            config_path.push("gh");
            config_path.push("hosts.yml");

            if let Ok(content) = std::fs::read_to_string(&config_path)
                && let Ok(config) = serde_yaml::from_str::<GhConfig>(&content)
            {
                return config.github_com.oauth_token;
            }
        }

        None
    }

    /// Fetch full commit information from a specific repository
    /// Returns None if the commit doesn't exist on GitHub or client errors
    pub async fn fetch_commit_full_info(
        &self,
        repo_info: &GhRepoInfo,
        commit_hash: &str,
    ) -> Option<CommitInfo> {
        let commit = self
            .call_client_api_with_fallback(move |client| {
                let hash = commit_hash.to_string();
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .commits(repo_info.owner(), repo_info.repo())
                        .get(&hash)
                        .await
                })
            })
            .await
            .ok()?;

        Some(commit.into())
    }

    /// Try to fetch a PR
    pub async fn fetch_pr(&self, repo_info: &GhRepoInfo, number: u64) -> Option<PullRequestInfo> {
        let pr = self
            .call_client_api_with_fallback(move |client| {
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .pulls(repo_info.owner(), repo_info.repo())
                        .get(number)
                        .await
                })
            })
            .await
            .ok()?;

        Some(pr.into())
    }

    /// Try to fetch an issue
    pub async fn fetch_issue(
        &self,
        repo_info: &GhRepoInfo,
        number: u64,
    ) -> Option<ExtendedIssueInfo> {
        let issue = self
            .call_client_api_with_fallback(move |client| {
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .issues(repo_info.owner(), repo_info.repo())
                        .get(number)
                        .await
                })
            })
            .await
            .ok()?;

        let mut issue_info = ExtendedIssueInfo::try_from(issue).ok()?;

        // Only fetch timeline for closed issues (open issues can't have closing PRs)
        if matches!(issue_info.state, octocrab::models::IssueState::Closed) {
            issue_info.closing_prs = self.find_closing_prs(repo_info, issue_info.number).await;
        }

        Some(issue_info)
    }

    /// Find closing PRs for an issue by examining timeline events
    /// Returns list of PR references (may be from different repositories)
    /// Priority:
    /// 1. Closed events with `commit_id` (clearly indicate the PR/commit that closed the issue)
    /// 2. CrossReferenced/Referenced events (fallback, but only merged PRs)
    async fn find_closing_prs(
        &self,
        repo_info: &GhRepoInfo,
        issue_number: u64,
    ) -> Vec<PullRequestInfo> {
        let mut closing_prs = Vec::new();

        // Try to get first page with auth client, fallback to anonymous
        let Ok((mut current_page, client)) = self
            .call_api_and_get_client(move |client| {
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .issues(repo_info.owner(), repo_info.repo())
                        .list_timeline_events(issue_number)
                        .per_page(100)
                        .send()
                        .await
                })
            })
            .await
        else {
            return Vec::new();
        };

        // Collect all timeline events to get closing commits and referenced PRs
        loop {
            for event in &current_page.items {
                // Collect candidate PRs from cross-references
                if let Some(source) = event.source.as_ref() {
                    let issue = &source.issue;
                    if issue.pull_request.is_some() {
                        // Extract repository info from repository_url using existing parser
                        if let Some(repo_info) =
                            parse_github_repo_url(issue.repository_url.as_str())
                        {
                            let Some(pr_info) =
                                Box::pin(self.fetch_pr(&repo_info, issue.number)).await
                            else {
                                continue; // Skip if PR fetch failed
                            };

                            if !pr_info.merged {
                                continue; // Only consider merged PRs
                            }

                            if matches!(event.event, TimelineEventType::Closed) {
                                // If it's a Closed event, assume this is the closing PR
                                closing_prs.push(pr_info);
                                break; // No need to check further events
                            }

                            // Otherwise, only consider CrossReferenced/Referenced events
                            if !matches!(
                                event.event,
                                TimelineEventType::CrossReferenced | TimelineEventType::Referenced
                            ) {
                                continue;
                            }

                            // Check if we already have this PR
                            if !closing_prs.iter().any(|p| {
                                p.number == issue.number
                                    && p.repo_info
                                        .as_ref()
                                        .is_some_and(|ri| ri.owner() == repo_info.owner())
                                    && p.repo_info
                                        .as_ref()
                                        .is_some_and(|ri| ri.repo() == repo_info.repo())
                            }) {
                                closing_prs.push(pr_info);
                            }
                        }
                    }
                }
            }

            match Self::await_with_timeout_and_error(
                client.get_page::<TimelineEvent>(&current_page.next),
            )
            .await
            .ok()
            .flatten()
            {
                Some(next_page) => current_page = next_page,
                None => break,
            }
        }

        closing_prs
    }

    /// Fetch releases from GitHub, optionally filtered by date
    /// If `since_date` is provided, stop fetching releases older than this date
    /// This significantly speeds up lookups for recent PRs/issues
    #[allow(clippy::too_many_lines)]
    pub async fn fetch_releases_since(
        &self,
        repo_info: &GhRepoInfo,
        since_date: Option<&str>,
    ) -> Vec<ReleaseInfo> {
        let mut releases = Vec::new();
        let mut page_num = 1u32;
        let per_page = 100u8; // Max allowed by GitHub API

        // Parse the cutoff date if provided
        let cutoff_timestamp = since_date.and_then(|date_str| {
            chrono::DateTime::parse_from_rfc3339(date_str)
                .ok()
                .map(|dt| dt.timestamp())
        });

        // Try to get first page with auth client, fallback to anonymous
        let Ok((mut current_page, client)) = self
            .call_api_and_get_client(move |client| {
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .repos(repo_info.owner(), repo_info.repo())
                        .releases()
                        .list()
                        .per_page(per_page)
                        .page(page_num)
                        .send()
                        .await
                })
            })
            .await
        else {
            return releases;
        };

        loop {
            if current_page.items.is_empty() {
                break; // No more pages
            }

            let mut should_stop = false;

            for release in current_page.items {
                let published_at_str = release.published_at.map(|dt| dt.to_string());

                // Check if this release is too old
                if let Some(cutoff) = cutoff_timestamp
                    && let Some(pub_at) = &release.published_at
                    && pub_at.timestamp() < cutoff
                {
                    should_stop = true;
                    break; // Stop processing this page
                }

                releases.push(ReleaseInfo {
                    tag_name: release.tag_name,
                    name: release.name,
                    url: release.html_url.to_string(),
                    published_at: published_at_str,
                    prerelease: release.prerelease,
                });
            }

            if should_stop {
                break; // Stop pagination
            }

            page_num += 1;

            // Fetch next page
            current_page = match Self::await_with_timeout_and_error(
                client
                    .repos(repo_info.owner(), repo_info.repo())
                    .releases()
                    .list()
                    .per_page(per_page)
                    .page(page_num)
                    .send(),
            )
            .await
            .ok()
            {
                Some(page) => page,
                None => break, // Stop on error
            };
        }

        releases
    }

    /// Fetch a GitHub release by tag.
    pub async fn fetch_release_by_tag(
        &self,
        repo_info: &GhRepoInfo,
        tag: &str,
    ) -> Option<ReleaseInfo> {
        let release = self
            .call_client_api_with_fallback(move |client| {
                let tag = tag.to_string();
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .repos(repo_info.owner(), repo_info.repo())
                        .releases()
                        .get_by_tag(tag.as_str())
                        .await
                })
            })
            .await
            .ok()?;

        Some(ReleaseInfo {
            tag_name: release.tag_name,
            name: release.name,
            url: release.html_url.to_string(),
            published_at: release.published_at.map(|dt| dt.to_string()),
            prerelease: release.prerelease,
        })
    }

    /// Fetch tag info for a release by checking if target commit is contained in the tag.
    /// Uses GitHub compare API to verify ancestry and get tag's commit hash.
    /// Returns None if the tag doesn't contain the target commit.
    pub async fn fetch_tag_info_for_release(
        &self,
        release: &ReleaseInfo,
        repo_info: &GhRepoInfo,
        target_commit: &str,
    ) -> Option<TagInfo> {
        // Use compare API with per_page=1 to optimize
        let compare = self
            .call_client_api_with_fallback(move |client| {
                let tag_name = release.tag_name.clone();
                let target_commit = target_commit.to_string();
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .commits(repo_info.owner(), repo_info.repo())
                        .compare(&tag_name, &target_commit)
                        .per_page(1)
                        .send()
                        .await
                })
            })
            .await
            .ok()?;

        // If status is "behind" or "identical", the target commit is in the tag's history
        // "ahead" or "diverged" means the commit is NOT in the tag
        if !matches!(
            compare.status,
            GithubCommitStatus::Behind | GithubCommitStatus::Identical
        ) {
            return None;
        }

        let semver_info = parse_semver(&release.tag_name);

        Some(TagInfo {
            name: release.tag_name.clone(),
            commit_hash: compare.base_commit.sha,
            semver_info,
            is_release: true,
            release_name: release.name.clone(),
            release_url: Some(release.url.clone()),
            published_at: release.published_at.clone(),
        })
    }

    /// Build GitHub URLs for various things
    /// Build a commit URL (fallback when API data unavailable)
    /// Uses URL encoding to prevent injection
    #[must_use]
    pub fn commit_url(repo_info: &GhRepoInfo, hash: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}/{}/commit/{}",
            utf8_percent_encode(repo_info.owner(), NON_ALPHANUMERIC),
            utf8_percent_encode(repo_info.repo(), NON_ALPHANUMERIC),
            utf8_percent_encode(hash, NON_ALPHANUMERIC)
        )
    }

    /// Build a tag URL (fallback when API data unavailable)
    /// Uses URL encoding to prevent injection
    #[must_use]
    pub fn tag_url(repo_info: &GhRepoInfo, tag: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}/{}/tree/{}",
            utf8_percent_encode(repo_info.owner(), NON_ALPHANUMERIC),
            utf8_percent_encode(repo_info.repo(), NON_ALPHANUMERIC),
            utf8_percent_encode(tag, NON_ALPHANUMERIC)
        )
    }

    /// Build a profile URL (fallback when API data unavailable)
    /// Uses URL encoding to prevent injection
    #[must_use]
    pub fn profile_url(username: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}",
            utf8_percent_encode(username, NON_ALPHANUMERIC)
        )
    }

    const fn connect_timeout() -> Duration {
        Duration::from_secs(CONNECT_TIMEOUT_SECS)
    }

    const fn read_timeout() -> Duration {
        Duration::from_secs(READ_TIMEOUT_SECS)
    }

    const fn request_timeout() -> Duration {
        Duration::from_secs(REQUEST_TIMEOUT_SECS)
    }

    /// Call a GitHub API with fallback from authenticated to anonymous client.
    async fn call_client_api_with_fallback<F, T>(&self, api_call: F) -> WtgResult<T>
    where
        for<'a> F: Fn(&'a Octocrab) -> Pin<Box<dyn Future<Output = OctoResult<T>> + Send + 'a>>,
    {
        let (result, _client) = self.call_api_and_get_client(api_call).await?;
        Ok(result)
    }

    /// Call a GitHub API with fallback from authenticated to anonymous client.
    /// Returns results & the client used, or error.
    async fn call_api_and_get_client<F, T>(&self, api_call: F) -> WtgResult<(T, &Octocrab)>
    where
        for<'a> F: Fn(&'a Octocrab) -> Pin<Box<dyn Future<Output = OctoResult<T>> + Send + 'a>>,
    {
        // Try with authenticated client first
        if let Some(client) = self.auth_client.as_ref() {
            match Self::await_with_timeout_and_error(api_call(client)).await {
                Ok(result) => return Ok((result, client)),
                Err(e) if e.is_gh_saml() => {
                    // Fall through to try anonymous client
                }
                Err(e) => {
                    // Non-SAML error or timeout, don't retry
                    return Err(e);
                }
            }
        }

        // Try with anonymous client (either as fallback or if no authenticated client)
        let Some(client) = self.anonymous_client.as_ref() else {
            return Err(WtgError::GhNoClient);
        };

        let result = Self::await_with_timeout_and_error(api_call(client)).await?;

        Ok((result, client))
    }

    /// Await with timeout, returning non-timeout error if any
    async fn await_with_timeout_and_error<F, T>(future: F) -> WtgResult<T>
    where
        F: Future<Output = OctoResult<T>>,
    {
        match tokio::time::timeout(Self::request_timeout(), future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Err(WtgError::Timeout),
        }
    }
}
