use octocrab::{
    Octocrab, OctocrabBuilder,
    models::{Event as TimelineEventType, timelines::TimelineEvent},
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: Option<Octocrab>,
    owner: String,
    repo: String,
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
    pub closing_prs: Vec<u64>,      // PR numbers that closed this issue
    pub created_at: Option<String>, // When the issue was created
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub url: String,
    pub published_at: Option<String>,
}

impl GitHubClient {
    /// Create a new GitHub client with authentication
    #[must_use]
    pub fn new(owner: String, repo: String) -> Self {
        let client = Self::build_client();

        Self {
            client,
            owner,
            repo,
        }
    }

    /// Build an authenticated octocrab client
    fn build_client() -> Option<Octocrab> {
        // Set reasonable timeouts: 5s connect, 30s read/write
        let connect_timeout = Some(Duration::from_secs(5));
        let read_timeout = Some(Duration::from_secs(30));

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

        // Fall back to anonymous
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
        let client = self.client.as_ref()?;

        let commit = client
            .commits(&self.owner, &self.repo)
            .get(commit_hash)
            .await
            .ok()?;

        let commit_url = commit.html_url;
        let author_info = commit
            .author
            .map(|author| (author.login, author.html_url.into()));

        Some((commit_hash.to_string(), commit_url, author_info))
    }

    /// Try to fetch a PR
    pub async fn fetch_pr(&self, number: u64) -> Option<PullRequestInfo> {
        let client = self.client.as_ref()?;

        if let Ok(pr) = client.pulls(&self.owner, &self.repo).get(number).await {
            let author = pr.user.as_ref().map(|u| u.login.clone());
            let author_url = pr.user.as_ref().map(|u| u.html_url.to_string());
            let created_at = pr.created_at.map(|dt| dt.to_string());

            return Some(PullRequestInfo {
                number,
                title: pr.title.unwrap_or_default(),
                body: pr.body,
                state: format!("{:?}", pr.state),
                url: pr.html_url.map(|u| u.to_string()).unwrap_or_default(),
                merge_commit_sha: pr.merge_commit_sha,
                author,
                author_url,
                created_at,
            });
        }

        None
    }

    /// Try to fetch an issue
    pub async fn fetch_issue(&self, number: u64) -> Option<IssueInfo> {
        let client = self.client.as_ref()?;

        match client.issues(&self.owner, &self.repo).get(number).await {
            Ok(issue) => {
                // If it has a pull_request field, it's actually a PR - skip it
                if issue.pull_request.is_some() {
                    return None;
                }

                let author = issue.user.login.clone();
                let author_url = Some(issue.user.html_url.to_string());
                let created_at = Some(issue.created_at.to_string());

                // OPTIMIZED: Only fetch timeline for closed issues (open issues can't have closing PRs)
                let is_closed = matches!(issue.state, octocrab::models::IssueState::Closed);
                let closing_prs = if is_closed {
                    self.find_closing_prs(number).await
                } else {
                    Vec::new()
                };

                Some(IssueInfo {
                    number,
                    title: issue.title,
                    body: issue.body,
                    state: issue.state,
                    url: issue.html_url.to_string(),
                    author: Some(author),
                    author_url,
                    closing_prs,
                    created_at,
                })
            }
            Err(_) => None,
        }
    }

    /// Find closing PRs for an issue by examining timeline events
    /// Returns list of PR numbers that closed this issue
    async fn find_closing_prs(&self, issue_number: u64) -> Vec<u64> {
        let Some(client) = self.client.as_ref() else {
            return Vec::new();
        };

        let mut closing_prs = Vec::new();

        let Ok(mut page) = client
            .issues(&self.owner, &self.repo)
            .list_timeline_events(issue_number)
            .per_page(100)
            .send()
            .await
        else {
            return closing_prs;
        };

        loop {
            for event in &page.items {
                if let Some(source) = event.source.as_ref()
                    && matches!(
                        event.event,
                        TimelineEventType::CrossReferenced | TimelineEventType::Referenced
                    )
                {
                    let issue = &source.issue;
                    if issue.pull_request.is_some() && !closing_prs.contains(&issue.number) {
                        closing_prs.push(issue.number);
                    }
                }
            }

            match client
                .get_page::<TimelineEvent>(&page.next)
                .await
                .ok()
                .flatten()
            {
                Some(next_page) => page = next_page,
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
    pub async fn fetch_releases_since(&self, since_date: Option<&str>) -> Vec<ReleaseInfo> {
        let Some(client) = self.client.as_ref() else {
            return Vec::new();
        };

        let mut releases = Vec::new();
        let mut page_num = 1u32;
        let per_page = 100u8; // Max allowed by GitHub API

        // Parse the cutoff date if provided
        let cutoff_timestamp = since_date.and_then(|date_str| {
            chrono::DateTime::parse_from_rfc3339(date_str)
                .ok()
                .map(|dt| dt.timestamp())
        });

        loop {
            let page_result = client
                .repos(&self.owner, &self.repo)
                .releases()
                .list()
                .per_page(per_page)
                .page(page_num)
                .send()
                .await;

            let Ok(page) = page_result else {
                break; // Stop on error
            };

            if page.items.is_empty() {
                break; // No more pages
            }

            let mut should_stop = false;

            for release in page.items {
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
        }

        releases
    }

    /// Fetch a GitHub release by tag.
    pub async fn fetch_release_by_tag(&self, tag: &str) -> Option<ReleaseInfo> {
        let client = self.client.as_ref()?;
        let release = client
            .repos(&self.owner, &self.repo)
            .releases()
            .get_by_tag(tag)
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
            utf8_percent_encode(&self.owner, NON_ALPHANUMERIC),
            utf8_percent_encode(&self.repo, NON_ALPHANUMERIC),
            utf8_percent_encode(hash, NON_ALPHANUMERIC)
        )
    }

    /// Build a tag URL (fallback when API data unavailable)
    /// Uses URL encoding to prevent injection
    pub fn tag_url(&self, tag: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        format!(
            "https://github.com/{}/{}/tree/{}",
            utf8_percent_encode(&self.owner, NON_ALPHANUMERIC),
            utf8_percent_encode(&self.repo, NON_ALPHANUMERIC),
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
}

#[derive(Debug, Deserialize)]
struct GhConfig {
    #[serde(rename = "github.com")]
    github_com: GhHostConfig,
}

#[derive(Debug, Deserialize)]
struct GhHostConfig {
    oauth_token: Option<String>,
}
