use octocrab::Octocrab;
use serde::{Deserialize, Serialize};

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
    pub state: String,
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
        // Try GITHUB_TOKEN env var first
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            return Octocrab::builder().personal_token(token).build().ok();
        }

        // Try reading from gh CLI config
        if let Some(token) = Self::read_gh_config() {
            return Octocrab::builder().personal_token(token).build().ok();
        }

        // Fall back to anonymous
        Octocrab::builder().build().ok()
    }

    /// Read GitHub token from gh CLI config (cross-platform)
    fn read_gh_config() -> Option<String> {
        // Use dirs::config_dir() for cross-platform support
        // On Unix: ~/.config/gh/hosts.yml
        // On Windows: %APPDATA%/gh/hosts.yml
        // On macOS: ~/Library/Application Support/gh/hosts.yml
        let mut config_path = dirs::config_dir()?;
        config_path.push("gh");
        config_path.push("hosts.yml");

        let content = std::fs::read_to_string(config_path).ok()?;
        let config: GhConfig = serde_yaml::from_str(&content).ok()?;

        config.github_com.oauth_token
    }

    /// Check if client is available
    #[allow(dead_code)] // Will be used for network availability checks
    pub const fn is_available(&self) -> bool {
        self.client.is_some()
    }

    /// Try to fetch a PR
    pub async fn fetch_pr(&self, number: u64) -> Option<PullRequestInfo> {
        let client = self.client.as_ref()?;

        if let Ok(pr) = client.pulls(&self.owner, &self.repo).get(number).await {
            let author = pr.user.as_ref().map(|u| u.login.clone());
            let author_url = author.as_ref().map(|login| Self::profile_url(login));
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

        if let Ok(issue) = client.issues(&self.owner, &self.repo).get(number).await {
            // If it has a pull_request field, it's actually a PR - skip it
            if issue.pull_request.is_some() {
                return None;
            }

            let author = issue.user.login.clone();
            let author_url = Some(Self::profile_url(&author));
            let created_at = Some(issue.created_at.to_string());

            // Find closing PRs via timeline
            let closing_prs = self.find_closing_prs(number).await;

            return Some(IssueInfo {
                number,
                title: issue.title,
                body: issue.body,
                state: format!("{:?}", issue.state),
                url: issue.html_url.to_string(),
                author: Some(author),
                author_url,
                closing_prs,
                created_at,
            });
        }

        None
    }

    /// Find closing PRs for an issue by examining timeline events
    /// Returns list of PR numbers that closed this issue
    async fn find_closing_prs(&self, issue_number: u64) -> Vec<u64> {
        let client = match self.client.as_ref() {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut closing_prs = Vec::new();

        let timeline_url = format!(
            "https://api.github.com/repos/{}/{}/issues/{}/timeline",
            self.owner, self.repo, issue_number
        );

        if let Ok(events) = client
            .get::<serde_json::Value, _, _>(&timeline_url, None::<&()>)
            .await
        {
            if let Some(events_array) = events.as_array() {
                for event in events_array {
                    let event_type = event.get("event").and_then(|v| v.as_str());

                    // "cross-referenced" events show PRs that reference this issue
                    if matches!(event_type, Some("cross-referenced" | "referenced")) {
                        if let Some(source) = event.get("source")
                            && let Some(issue) = source.get("issue")
                            && issue.get("pull_request").is_some()
                            && let Some(pr_number) =
                                issue.get("number").and_then(serde_json::Value::as_u64)
                            && !closing_prs.contains(&pr_number)
                        {
                            closing_prs.push(pr_number);
                        }
                    }
                }
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
        let client = match self.client.as_ref() {
            Some(c) => c,
            None => return Vec::new(),
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

            let page = match page_result {
                Ok(p) => p,
                Err(_) => break, // Stop on error
            };

            if page.items.is_empty() {
                break; // No more pages
            }

            let mut should_stop = false;

            for release in page.items {
                let published_at_str = release.published_at.map(|dt| dt.to_string());

                // Check if this release is too old
                if let Some(cutoff) = cutoff_timestamp {
                    if let Some(pub_at) = &release.published_at {
                        if pub_at.timestamp() < cutoff {
                            should_stop = true;
                            break; // Stop processing this page
                        }
                    }
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

    /// Build GitHub URLs for various things
    pub fn commit_url(&self, hash: &str) -> String {
        format!(
            "https://github.com/{}/{}/commit/{}",
            self.owner, self.repo, hash
        )
    }

    #[allow(dead_code)] // Will be used when displaying release info
    pub fn release_url(&self, tag: &str) -> String {
        format!(
            "https://github.com/{}/{}/releases/tag/{}",
            self.owner, self.repo, tag
        )
    }

    pub fn tag_url(&self, tag: &str) -> String {
        format!(
            "https://github.com/{}/{}/tree/{}",
            self.owner, self.repo, tag
        )
    }

    pub fn profile_url(username: &str) -> String {
        format!("https://github.com/{username}")
    }

    #[allow(dead_code)] // Will be used for issue link generation
    pub fn issue_url(&self, number: u64) -> String {
        format!(
            "https://github.com/{}/{}/issues/{}",
            self.owner, self.repo, number
        )
    }

    #[allow(dead_code)] // Will be used for PR link generation
    pub fn pr_url(&self, number: u64) -> String {
        format!(
            "https://github.com/{}/{}/pull/{}",
            self.owner, self.repo, number
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct GhConfig {
    #[serde(rename = "github.com")]
    github_com: GhHostConfig,
}

#[derive(Debug, Deserialize, Serialize)]
struct GhHostConfig {
    oauth_token: Option<String>,
}
