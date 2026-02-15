use std::{env, fs, future::Future, pin::Pin, sync::LazyLock, sync::OnceLock, time::Duration};

use chrono::{DateTime, Utc};
use octocrab::{
    Octocrab, OctocrabBuilder, Result as OctoResult,
    models::{
        Event as TimelineEventType, commits::GithubCommitStatus, repos::RepoCommit,
        timelines::TimelineEvent,
    },
};
use serde::Deserialize;

use crate::error::{LogError, WtgError, WtgResult};
use crate::git::{CommitInfo, TagInfo, parse_semver};
use crate::notice::{Notice, NoticeCallback};
use crate::parse_input::parse_github_repo_url;

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

        let date = commit
            .commit
            .author
            .as_ref()
            .and_then(|a| a.date.as_ref())
            .copied()
            .unwrap_or_else(Utc::now);

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

/// Describes which client served an API call and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientSelection {
    /// The main (possibly authenticated) client succeeded directly.
    Main,
    /// The backup (anonymous) client was used after the main client failed.
    Fallback(FallbackReason),
}

/// Why the main client failed, causing fallback to the backup client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FallbackReason {
    /// SAML SSO enforcement blocked the authenticated request (403).
    Saml,
    /// Token was invalid, expired, or revoked (401).
    BadCredentials,
}

/// Result from [`GitHubClient::call_api_and_get_client`] containing the API
/// response, the client that served it, and how the client was selected.
struct ApiCallResult<'a, T> {
    value: T,
    client: &'a Octocrab,
    selection: ClientSelection,
}

/// GitHub API client wrapper.
///
/// - Provides a simplified interface for common GitHub operations used in wtg over direct octocrab usage.
/// - Handles authentication via `GITHUB_TOKEN` env var or gh CLI config.
/// - Supports fallback to anonymous requests on SAML errors via backup client.
/// - Converts known octocrab errors into `WtgError` variants.
/// - Returns `None` from `new()` if no client can be created.
pub struct GitHubClient {
    main_client: Octocrab,
    /// Backup client for SAML fallback. Only populated when `main_client` is authenticated.
    /// When `main_client` is anonymous, there's no point in falling back to another anonymous client.
    backup_client: LazyLock<Option<Octocrab>>,
    /// Whether `main_client` is authenticated (vs anonymous).
    is_authenticated: bool,
    /// Callback for emitting notices (e.g., rate limit hit).
    /// Uses `OnceLock` since callback is set at most once after construction.
    notice_callback: OnceLock<NoticeCallback>,
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
    pub created_at: Option<DateTime<Utc>>, // When the PR was created
}

impl From<octocrab::models::pulls::PullRequest> for PullRequestInfo {
    fn from(pr: octocrab::models::pulls::PullRequest) -> Self {
        let author = pr.user.as_ref().map(|u| u.login.clone());
        let author_url = pr.user.as_ref().map(|u| u.html_url.to_string());
        let created_at = pr.created_at;

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
    /// Timeline data may be incomplete due to SAML-restricted org access.
    /// When true, closing PRs may exist but aren't visible without SSO auth.
    pub timeline_may_be_incomplete: bool,
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
            timeline_may_be_incomplete: false,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub prerelease: bool,
}

impl GitHubClient {
    /// Create a new GitHub client.
    ///
    /// Returns `None` if no client (neither authenticated nor anonymous) can be created.
    /// If authentication succeeds, an anonymous backup client is created for SAML fallback.
    /// If authentication fails, the anonymous client becomes the main client with no backup.
    #[must_use]
    pub fn new() -> Option<Self> {
        // Try authenticated client first
        if let Some(auth) = Self::build_auth_client() {
            // Auth succeeded - create anonymous as lazy backup for SAML fallback
            return Some(Self {
                main_client: auth,
                backup_client: LazyLock::new(Self::build_anonymous_client),
                is_authenticated: true,
                notice_callback: OnceLock::new(),
            });
        }

        // Auth failed - try anonymous as main
        // No backup needed: falling back to anonymous when already anonymous is pointless
        let anonymous = Self::build_anonymous_client()?;
        Some(Self {
            main_client: anonymous,
            backup_client: LazyLock::new(|| None),
            is_authenticated: false,
            notice_callback: OnceLock::new(),
        })
    }

    /// Create a GitHub client with a specific token.
    ///
    /// Builds an authenticated client using the given token, with an anonymous
    /// backup for fallback on SAML or bad credentials errors.
    /// Returns `None` if the client cannot be built.
    #[must_use]
    pub fn new_with_token(token: String) -> Option<Self> {
        let connect_timeout = Some(Self::connect_timeout());
        let read_timeout = Some(Self::read_timeout());

        let auth = OctocrabBuilder::new()
            .personal_token(token)
            .set_connect_timeout(connect_timeout)
            .set_read_timeout(read_timeout)
            .build()
            .ok()?;

        Some(Self {
            main_client: auth,
            backup_client: LazyLock::new(Self::build_anonymous_client),
            is_authenticated: true,
            notice_callback: OnceLock::new(),
        })
    }

    /// Set the notice callback for this client.
    /// Can be called even when client is behind an `Arc`.
    /// First call wins - subsequent calls are ignored.
    pub fn set_notice_callback(&self, callback: NoticeCallback) {
        // set() returns Err if already set - we ignore since first-set wins
        let _ = self.notice_callback.set(callback);
    }

    /// Emit a notice via the callback, if one is set.
    pub(crate) fn emit(&self, notice: Notice) {
        if let Some(cb) = self.notice_callback.get() {
            (cb)(notice);
        }
    }

    /// Build an authenticated octocrab client.
    /// Returns `None` if `WTG_GH_NO_AUTH` is set or no valid token is found.
    fn build_auth_client() -> Option<Octocrab> {
        // Undocumented: skip all authentication when WTG_GH_NO_AUTH is set
        if env::var("WTG_GH_NO_AUTH").is_ok() {
            log::debug!("WTG_GH_NO_AUTH set, skipping GitHub authentication");
            return None;
        }

        // Set reasonable timeouts: 5s connect, 30s read/write
        let connect_timeout = Some(Self::connect_timeout());
        let read_timeout = Some(Self::read_timeout());

        // Try GITHUB_TOKEN env var first (skip empty/whitespace-only values)
        if let Ok(token) = env::var("GITHUB_TOKEN")
            && !token.trim().is_empty()
        {
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
            if let Ok(content) = fs::read_to_string(&xdg_path)
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

            if let Ok(content) = fs::read_to_string(&config_path)
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
            .log_err(&format!(
                "fetch_commit_full_info failed for {}/{} commit {}",
                repo_info.owner(),
                repo_info.repo(),
                commit_hash
            ))?;

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
            .log_err(&format!(
                "fetch_pr failed for {}/{} PR #{}",
                repo_info.owner(),
                repo_info.repo(),
                number
            ))?;

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
            .log_err(&format!(
                "fetch_issue failed for {}/{} issue #{}",
                repo_info.owner(),
                repo_info.repo(),
                number
            ))?;

        let mut issue_info = ExtendedIssueInfo::try_from(issue).ok()?;

        // Only fetch timeline for closed issues (open issues can't have closing PRs)
        if matches!(issue_info.state, octocrab::models::IssueState::Closed) {
            let (closing_prs, saml_fallback) =
                self.find_closing_prs(repo_info, issue_info.number).await;
            issue_info.closing_prs = closing_prs;
            issue_info.timeline_may_be_incomplete = saml_fallback;
        }

        Some(issue_info)
    }

    /// Find closing PRs for an issue by examining timeline events.
    /// Returns `(prs, saml_fallback)` where `saml_fallback` is true when the
    /// timeline was fetched via an anonymous client after a SAML error, meaning
    /// cross-project PR references may be missing from the response.
    async fn find_closing_prs(
        &self,
        repo_info: &GhRepoInfo,
        issue_number: u64,
    ) -> (Vec<PullRequestInfo>, bool) {
        let mut closing_prs = Vec::new();

        // Try to get first page with auth client, fallback to anonymous
        let Ok(result) = self
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
            return (Vec::new(), false);
        };

        let mut current_page = result.value;
        let client = result.client;
        let saml_fallback = matches!(
            result.selection,
            ClientSelection::Fallback(FallbackReason::Saml)
        );

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
                            // Note: GitHub API returns PRs as issues, so issue.number is the PR number
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

        (closing_prs, saml_fallback)
    }

    /// Fetch releases from GitHub, optionally filtered by date
    /// If `since_date` is provided, stop fetching releases older than this date
    /// This significantly speeds up lookups for recent PRs/issues
    #[allow(clippy::too_many_lines)]
    pub async fn fetch_releases_since(
        &self,
        repo_info: &GhRepoInfo,
        since_date: DateTime<Utc>,
    ) -> Vec<ReleaseInfo> {
        let mut releases = Vec::new();
        let mut page_num = 1u32;
        let per_page = 100u8; // Max allowed by GitHub API

        // Try to get first page with auth client, fallback to anonymous
        let Ok(result) = self
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
        let mut current_page = result.value;
        let client = result.client;

        'pagintaion: loop {
            if current_page.items.is_empty() {
                break; // No more pages
            }

            // Sort releases by created_at descending
            current_page
                .items
                .sort_by(|a, b| b.created_at.cmp(&a.created_at));

            for release in current_page.items {
                // Check if this release is too old
                let release_tag_created_at = release.created_at.unwrap_or_default();

                if release_tag_created_at < since_date {
                    break 'pagintaion; // Stop processing
                }

                releases.push(ReleaseInfo {
                    tag_name: release.tag_name,
                    name: release.name,
                    body: release.body,
                    url: release.html_url.to_string(),
                    published_at: release.published_at,
                    created_at: release.created_at,
                    prerelease: release.prerelease,
                });
            }

            if current_page.next.is_none() {
                break; // No more pages
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
            .log_err(&format!(
                "fetch_release_by_tag failed for {}/{} tag {}",
                repo_info.owner(),
                repo_info.repo(),
                tag
            ))?;

        Some(ReleaseInfo {
            tag_name: release.tag_name,
            name: release.name,
            body: release.body,
            url: release.html_url.to_string(),
            published_at: release.published_at,
            created_at: release.created_at,
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
            .log_err(&format!(
                "fetch_tag_info_for_release failed for {}/{} tag {} vs commit {}",
                repo_info.owner(),
                repo_info.repo(),
                release.tag_name,
                target_commit
            ))?;

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
            created_at: release.created_at?,
            is_release: true,
            release_name: release.name.clone(),
            release_url: Some(release.url.clone()),
            published_at: release.published_at,
            tag_url: Some(release.url.clone()),
        })
    }

    /// Check if a tag contains a specific commit using the GitHub compare API.
    ///
    /// Returns true if the commit is in the tag's history (status is "behind" or "identical").
    pub async fn tag_contains_commit(
        &self,
        repo_info: &GhRepoInfo,
        tag: &str,
        commit: &str,
    ) -> bool {
        let compare = self
            .call_client_api_with_fallback(move |client| {
                let tag = tag.to_string();
                let commit = commit.to_string();
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .commits(repo_info.owner(), repo_info.repo())
                        .compare(&tag, &commit)
                        .per_page(1)
                        .send()
                        .await
                })
            })
            .await
            .ok();

        matches!(
            compare.map(|c| c.status),
            Some(GithubCommitStatus::Behind | GithubCommitStatus::Identical)
        )
    }

    /// Fetch tag info by name.
    /// Uses the commits API (which accepts refs) to resolve the tag to a commit,
    /// then optionally enriches with release info if available.
    pub async fn fetch_tag(&self, repo_info: &GhRepoInfo, tag_name: &str) -> Option<TagInfo> {
        // Use commits API with tag name as ref to get the commit
        let commit = self.fetch_commit_full_info(repo_info, tag_name).await?;

        // Try to get release info (may not exist if tag has no release)
        let release = self.fetch_release_by_tag(repo_info, tag_name).await;

        let semver_info = parse_semver(tag_name);

        // Compute tag_url: release URL for releases, tree URL for plain tags
        let tag_url = Some(
            release
                .as_ref()
                .map_or_else(|| Self::tag_url(repo_info, tag_name), |r| r.url.clone()),
        );

        Some(TagInfo {
            name: tag_name.to_string(),
            commit_hash: commit.hash,
            semver_info,
            created_at: commit.date,
            is_release: release.is_some(),
            release_name: release.as_ref().and_then(|r| r.name.clone()),
            release_url: release.as_ref().map(|r| r.url.clone()),
            published_at: release.and_then(|r| r.published_at),
            tag_url,
        })
    }

    /// Fetch file content from the default branch.
    ///
    /// Returns the decoded file content as a String, or None if the file
    /// doesn't exist or can't be decoded (e.g., binary files).
    pub async fn fetch_file_content(&self, repo_info: &GhRepoInfo, path: &str) -> Option<String> {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;

        let content = self
            .call_client_api_with_fallback(move |client| {
                let path = path.to_string();
                let repo_info = repo_info.clone();
                Box::pin(async move {
                    client
                        .repos(repo_info.owner(), repo_info.repo())
                        .get_content()
                        .path(&path)
                        .send()
                        .await
                })
            })
            .await
            .ok()?;

        // The API returns an array for directories, single item for files
        let file_content = match content.items.into_iter().next()? {
            octocrab::models::repos::Content {
                content: Some(encoded),
                ..
            } => {
                // Content is base64 encoded with newlines, need to remove them
                let cleaned: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
                STANDARD.decode(&cleaned).ok()?
            }
            _ => return None, // No content or it's a directory
        };

        String::from_utf8(file_content).ok()
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

    /// Build a tag URL pointing to the tree view (for plain git tags).
    /// Uses URL encoding to prevent injection.
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

    /// Build a release URL pointing to the releases page (for tags with releases).
    /// Uses URL encoding to prevent injection.
    #[must_use]
    pub fn release_tag_url(repo_info: &GhRepoInfo, tag: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}/{}/releases/tag/{}",
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

    /// Build a profile URL from a GitHub noreply email address.
    ///
    /// Extracts username from patterns:
    /// - `username@users.noreply.github.com`
    /// - `id+username@users.noreply.github.com`
    #[must_use]
    pub fn author_url_from_email(email: &str) -> Option<String> {
        if email.ends_with("@users.noreply.github.com") {
            let parts: Vec<&str> = email.split('@').collect();
            if let Some(user_part) = parts.first()
                && let Some(username) = user_part.split('+').next_back()
            {
                return Some(Self::profile_url(username));
            }
        }
        None
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
        let result = self.call_api_and_get_client(api_call).await?;
        Ok(result.value)
    }

    /// Call a GitHub API with fallback to backup client on SAML or bad-credentials errors.
    /// Returns an [`ApiCallResult`] containing the value, the client used, and how the
    /// client was selected (see [`ClientSelection`]).
    /// Emits `Notice::GhRateLimitHit` if rate limit is detected.
    async fn call_api_and_get_client<F, T>(&self, api_call: F) -> WtgResult<ApiCallResult<'_, T>>
    where
        for<'a> F: Fn(&'a Octocrab) -> Pin<Box<dyn Future<Output = OctoResult<T>> + Send + 'a>>,
    {
        // Try with main client first
        let (main_error, fallback_reason) =
            match Self::await_with_timeout_and_error(api_call(&self.main_client)).await {
                Ok(result) => {
                    return Ok(ApiCallResult {
                        value: result,
                        client: &self.main_client,
                        selection: ClientSelection::Main,
                    });
                }
                Err(e) if e.is_gh_rate_limit() => {
                    log::debug!(
                        "GitHub API rate limit hit (authenticated={}): {:?}",
                        self.is_authenticated,
                        e
                    );
                    self.emit(Notice::GhRateLimitHit {
                        authenticated: self.is_authenticated,
                    });
                    return Err(e);
                }
                Err(e) if e.is_gh_saml() && self.is_authenticated => {
                    // SAML error with authenticated client - fall through to try backup
                    (e, FallbackReason::Saml)
                }
                Err(e) if e.is_gh_bad_credentials() && self.is_authenticated => {
                    // Bad credentials (401) - token is invalid/expired, fall through to try backup
                    log::debug!("GitHub API bad credentials, falling back to anonymous client");
                    (e, FallbackReason::BadCredentials)
                }
                Err(e) => {
                    // Non-recoverable error, or unauthenticated client (no fallback possible)
                    log::debug!("GitHub API error: {e:?}");
                    return Err(e);
                }
            };

        // Try with backup client (only reached if authenticated and main failed with SAML or bad creds)
        let Some(backup) = self.backup_client.as_ref() else {
            // Backup client failed to build - connection was lost between auth and now
            return Err(WtgError::GhConnectionLost);
        };

        // Try the backup; if it also fails, return the original main-client error for SAML
        match Self::await_with_timeout_and_error(api_call(backup)).await {
            Ok(result) => Ok(ApiCallResult {
                value: result,
                client: backup,
                selection: ClientSelection::Fallback(fallback_reason),
            }),
            Err(e) if e.is_gh_rate_limit() => {
                log::debug!("GitHub API rate limit hit on backup client: {e:?}");
                // Emit notice for anonymous fallback (authenticated was true to reach here,
                // but backup is anonymous)
                self.emit(Notice::GhRateLimitHit {
                    authenticated: false,
                });
                Err(e)
            }
            Err(e) if e.is_gh_saml() => Err(main_error), // Return original SAML error
            Err(e) => {
                log::debug!("GitHub API error on backup client: {e:?}");
                Err(e)
            }
        }
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
