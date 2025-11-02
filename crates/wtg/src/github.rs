use octocrab::Octocrab;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: Option<Octocrab>,
    owner: String,
    repo: String,
}

#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub is_pr: bool,
    #[allow(dead_code)] // Will be used when we implement issue state display
    pub state: String,
    pub url: String,
    pub closing_commits: Vec<String>,
    pub closing_prs: Vec<u64>, // PR numbers that closed this issue
    pub merge_commit_sha: Option<String>,
    pub author: Option<String>,
    pub author_url: Option<String>,
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

    /// Try to fetch an issue or PR
    pub async fn fetch_issue(&self, number: u64) -> Option<IssueInfo> {
        let client = self.client.as_ref()?;

        // Try as PR first to get merge commit
        if let Ok(pr) = client.pulls(&self.owner, &self.repo).get(number).await {
            let author = pr.user.as_ref().map(|u| u.login.clone());
            let author_url = author.as_ref().map(|login| Self::profile_url(login));

            return Some(IssueInfo {
                number,
                title: pr.title.unwrap_or_default(),
                is_pr: true,
                state: format!("{:?}", pr.state),
                url: pr.html_url.map(|u| u.to_string()).unwrap_or_default(),
                closing_commits: Vec::new(),
                closing_prs: Vec::new(),
                merge_commit_sha: pr.merge_commit_sha,
                author,
                author_url,
            });
        }

        // Fall back to issue API
        if let Ok(issue) = client.issues(&self.owner, &self.repo).get(number).await {
            let author = issue.user.login.clone();
            let author_url = Some(Self::profile_url(&author));

            // For issues (not PRs), try to find closing PRs via timeline
            let (closing_commits, closing_prs) = self.find_closing_pr_commits(number).await;

            return Some(IssueInfo {
                number,
                title: issue.title,
                is_pr: issue.pull_request.is_some(),
                state: format!("{:?}", issue.state),
                url: issue.html_url.to_string(),
                closing_commits,
                closing_prs,
                merge_commit_sha: None,
                author: Some(author),
                author_url,
            });
        }

        None
    }

    /// Find closing PR commits for an issue by examining timeline events
    /// Returns (`commit_shas`, `pr_numbers`)
    async fn find_closing_pr_commits(&self, issue_number: u64) -> (Vec<String>, Vec<u64>) {
        let client = match self.client.as_ref() {
            Some(c) => c,
            None => return (Vec::new(), Vec::new()),
        };

        let mut closing_commits = Vec::new();
        let mut closing_prs = Vec::new();

        // Fetch timeline events for the issue
        // Timeline events include "closed" events that may reference a PR
        let timeline_url = format!(
            "https://api.github.com/repos/{}/{}/issues/{}/timeline",
            self.owner, self.repo, issue_number
        );

        // Use octocrab's raw API to fetch timeline (as it may not have full timeline support)
        if let Ok(events) = client
            .get::<serde_json::Value, _, _>(&timeline_url, None::<&()>)
            .await
        {
            // Parse timeline events to find "closed" events with PR references
            if let Some(events_array) = events.as_array() {
                for event in events_array {
                    // Look for "closed" events with a source PR
                    if let Some(event_type) = event.get("event").and_then(|v| v.as_str())
                        && event_type == "closed"
                    {
                        // Check if there's a commit_id or source PR
                        if let Some(commit_id) = event.get("commit_id").and_then(|v| v.as_str()) {
                            closing_commits.push(commit_id.to_string());
                        }
                        // Also check for source.issue (cross-referenced PR)
                        if let Some(source) = event.get("source")
                            && let Some(issue) = source.get("issue")
                            && let Some(pr_number) =
                                issue.get("number").and_then(serde_json::Value::as_u64)
                        {
                            closing_prs.push(pr_number);
                            // Fetch this PR to get its merge commit
                            if let Ok(pr) =
                                client.pulls(&self.owner, &self.repo).get(pr_number).await
                                && let Some(merge_sha) = pr.merge_commit_sha
                            {
                                closing_commits.push(merge_sha);
                            }
                        }
                    }
                }
            }
        } else {
            // Timeline API might not be available or issue might not exist
            // Return empty lists
        }

        (closing_commits, closing_prs)
    }

    /// Fetch all releases from GitHub
    pub async fn fetch_releases(&self) -> Vec<ReleaseInfo> {
        let client = match self.client.as_ref() {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut releases = Vec::new();

        if let Ok(page) = client
            .repos(&self.owner, &self.repo)
            .releases()
            .list()
            .send()
            .await
        {
            for release in page.items {
                releases.push(ReleaseInfo {
                    tag_name: release.tag_name,
                    name: release.name,
                    url: release.html_url.to_string(),
                    published_at: release.published_at.map(|dt| dt.to_string()),
                });
            }
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
