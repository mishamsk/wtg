use chrono::{DateTime, Utc};
use octocrab::{
    Octocrab, OctocrabBuilder, Result as OctoResult,
    models::{Event as TimelineEventType, timelines::TimelineEvent},
};
use serde::Deserialize;
use std::{future::Future, pin::Pin, time::Duration};

use crate::{
    error::{WtgError, WtgResult},
    parse_url::parse_github_repo_url,
};

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

#[derive(Debug, Clone)]
pub struct GitHubClient {
    auth_client: Option<Octocrab>,
    anonymous_client: Option<Octocrab>,
    repo_info: GhRepoInfo,
}

/// Information about a Pull Request
#[derive(Debug, Clone)]
pub struct PullRequestInfo {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub url: String,
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
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            state: format!("{:?}", pr.state),
            url: pr.html_url.map(|u| u.to_string()).unwrap_or_default(),
            merge_commit_sha: pr.merge_commit_sha,
            author,
            author_url,
            created_at,
        }
    }
}

/// Reference to a PR that may be in a different repository
#[derive(Debug, Clone)]
pub struct PullRequestRef {
    pub number: u64,
    pub owner: String,
    pub repo: String,
}

/// Information about an Issue
#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: octocrab::models::IssueState,
    pub url: String,
    pub author: Option<String>,
    pub author_url: Option<String>,
    pub closing_prs: Vec<PullRequestRef>, // PRs that closed this issue (may be cross-repo)
    pub created_at: Option<DateTime<Utc>>, // When the issue was created
}

impl TryFrom<octocrab::models::issues::Issue> for IssueInfo {
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
}

impl GitHubClient {
    /// Get the repository owner
    #[must_use]
    pub fn owner(&self) -> &str {
        self.repo_info.owner()
    }

    /// Get the repository name
    #[must_use]
    pub fn repo(&self) -> &str {
        self.repo_info.repo()
    }

    /// Create a new GitHub client with authentication
    #[must_use]
    pub fn new(repo_info: GhRepoInfo) -> Self {
        let auth_client = Self::build_auth_client();
        let anonymous_client = Self::build_anonymous_client();

        Self {
            auth_client,
            anonymous_client,
            repo_info,
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

    /// Fetch the GitHub username and URLs for a commit
    /// Returns None if the commit doesn't exist on GitHub
    pub async fn fetch_commit_info(
        &self,
        commit_hash: &str,
    ) -> Option<(String, String, Option<(String, String)>)> {
        let commit_hash = commit_hash.to_string();
        let commit = self
            .call_client_api_with_fallback(|client, gh| {
                let hash = commit_hash.clone();
                Box::pin(async move { client.commits(gh.owner(), gh.repo()).get(&hash).await })
            })
            .await
            .ok()?;

        let commit_url = commit.html_url;
        let author_info = commit
            .author
            .map(|author| (author.login, author.html_url.into()));

        Some((commit_hash, commit_url, author_info))
    }

    /// Try to fetch a PR
    pub async fn fetch_pr(&self, number: u64) -> Option<PullRequestInfo> {
        let pr = self
            .call_client_api_with_fallback(|client, gh| {
                Box::pin(async move { client.pulls(gh.owner(), gh.repo()).get(number).await })
            })
            .await
            .ok()?;

        Some(pr.into())
    }

    pub async fn fetch_pr_ref(&self, pr_ref: PullRequestRef) -> Option<PullRequestInfo> {
        let pr = self
            .call_client_api_with_fallback(move |client, _| {
                let pr_ref = pr_ref.clone();
                Box::pin(async move {
                    client
                        .pulls(&pr_ref.owner, &pr_ref.repo)
                        .get(pr_ref.number)
                        .await
                })
            })
            .await
            .ok()?;

        Some(pr.into())
    }

    /// Try to fetch an issue
    pub async fn fetch_issue(&self, number: u64) -> Option<IssueInfo> {
        let issue = self
            .call_client_api_with_fallback(|client, gh| {
                Box::pin(async move { client.issues(gh.owner(), gh.repo()).get(number).await })
            })
            .await
            .ok()?;

        let mut issue_info = IssueInfo::try_from(issue).ok()?;

        // Only fetch timeline for closed issues (open issues can't have closing PRs)
        if matches!(issue_info.state, octocrab::models::IssueState::Closed) {
            issue_info.closing_prs = self.find_closing_prs(issue_info.number).await;
        }

        Some(issue_info)
    }

    /// Find closing PRs for an issue by examining timeline events
    /// Returns list of PR references (may be from different repositories)
    async fn find_closing_prs(&self, issue_number: u64) -> Vec<PullRequestRef> {
        let mut closing_prs = Vec::new();

        // Try to get first page with auth client, fallback to anonymous
        let Ok((mut current_page, client)) = self
            .call_api_and_get_client(|client, gh| {
                Box::pin(async move {
                    client
                        .issues(gh.owner(), gh.repo())
                        .list_timeline_events(issue_number)
                        .per_page(100)
                        .send()
                        .await
                })
            })
            .await
        else {
            return closing_prs;
        };

        loop {
            for event in &current_page.items {
                if let Some(source) = event.source.as_ref()
                    && matches!(
                        event.event,
                        TimelineEventType::CrossReferenced | TimelineEventType::Referenced
                    )
                {
                    let issue = &source.issue;
                    if issue.pull_request.is_some() {
                        // Extract repository info from repository_url using existing parser
                        if let Some(repo_info) =
                            parse_github_repo_url(issue.repository_url.as_str())
                        {
                            let pr_ref = PullRequestRef {
                                number: issue.number,
                                owner: repo_info.owner().to_string(),
                                repo: repo_info.repo().to_string(),
                            };
                            // Check if we already have this PR
                            if !closing_prs.iter().any(|p| {
                                p.number == pr_ref.number
                                    && p.owner == pr_ref.owner
                                    && p.repo == pr_ref.repo
                            }) {
                                closing_prs.push(pr_ref);
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

    /// Fetch all releases from GitHub
    pub async fn fetch_releases(&self) -> Vec<ReleaseInfo> {
        self.fetch_releases_since(None).await
    }

    /// Fetch releases from GitHub, optionally filtered by date
    /// If `since_date` is provided, stop fetching releases older than this date
    /// This significantly speeds up lookups for recent PRs/issues
    #[allow(clippy::too_many_lines)]
    pub async fn fetch_releases_since(&self, since_date: Option<&str>) -> Vec<ReleaseInfo> {
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
            .call_api_and_get_client(|client, gh| {
                Box::pin(async move {
                    client
                        .repos(gh.owner(), gh.repo())
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
                });
            }

            if should_stop {
                break; // Stop pagination
            }

            page_num += 1;

            // Fetch next page
            current_page = match Self::await_with_timeout_and_error(
                client
                    .repos(self.owner(), self.repo())
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
    pub async fn fetch_release_by_tag(&self, tag: &str) -> Option<ReleaseInfo> {
        let tag = tag.to_string();
        let release = self
            .call_client_api_with_fallback(|client, gh| {
                let tag = tag.clone();
                Box::pin(async move {
                    client
                        .repos(gh.owner(), gh.repo())
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
        })
    }

    /// Build GitHub URLs for various things
    /// Build a commit URL (fallback when API data unavailable)
    /// Uses URL encoding to prevent injection
    pub fn commit_url(&self, hash: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}/{}/commit/{}",
            utf8_percent_encode(self.owner(), NON_ALPHANUMERIC),
            utf8_percent_encode(self.repo(), NON_ALPHANUMERIC),
            utf8_percent_encode(hash, NON_ALPHANUMERIC)
        )
    }

    /// Build a tag URL (fallback when API data unavailable)
    /// Uses URL encoding to prevent injection
    pub fn tag_url(&self, tag: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}/{}/tree/{}",
            utf8_percent_encode(self.owner(), NON_ALPHANUMERIC),
            utf8_percent_encode(self.repo(), NON_ALPHANUMERIC),
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
        for<'a> F:
            Fn(&'a Octocrab, &'a Self) -> Pin<Box<dyn Future<Output = OctoResult<T>> + Send + 'a>>,
    {
        let (result, _client) = self.call_api_and_get_client(api_call).await?;
        Ok(result)
    }

    /// Call a GitHub API with fallback from authenticated to anonymous client.
    /// Returns results & the client used, or error.
    async fn call_api_and_get_client<F, T>(&self, api_call: F) -> WtgResult<(T, &Octocrab)>
    where
        for<'a> F:
            Fn(&'a Octocrab, &'a Self) -> Pin<Box<dyn Future<Output = OctoResult<T>> + Send + 'a>>,
    {
        // Try with authenticated client first
        if let Some(client) = self.auth_client.as_ref() {
            match Self::await_with_timeout_and_error(api_call(client, self)).await {
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

        let result = Self::await_with_timeout_and_error(api_call(client, self)).await?;

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
