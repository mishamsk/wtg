//! Pure GitHub API backend implementation.
//!
//! This backend only uses GitHub API via `GitHubClient`.
//! It can fetch commits, PRs, issues, and releases, but cannot
//! efficiently walk file history or perform local git operations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

use super::Backend;
use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, TagInfo};
use crate::github::{ExtendedIssueInfo, GhRepoInfo, GitHubClient, PullRequestInfo};

/// Pure GitHub API backend.
///
/// Uses `GitHubClient` for all operations. Cannot perform local git operations,
/// so file queries will return `Unsupported`.
pub struct GitHubBackend {
    client: Arc<GitHubClient>,
    repo_info: GhRepoInfo,
}

impl GitHubBackend {
    /// Create a new `GitHubBackend` for a repository.
    #[must_use]
    pub fn new(repo_info: GhRepoInfo) -> Self {
        Self {
            client: Arc::new(GitHubClient::new()),
            repo_info,
        }
    }

    /// Create a `GitHubBackend` with a shared client.
    #[must_use]
    pub const fn with_client(client: Arc<GitHubClient>, repo_info: GhRepoInfo) -> Self {
        Self { client, repo_info }
    }

    /// Get a reference to the `GitHubClient`.
    #[must_use]
    pub fn client(&self) -> &GitHubClient {
        &self.client
    }

    /// Find release for a commit by iterating through releases.
    async fn find_release_for_commit_impl(
        &self,
        commit_hash: &str,
        since: DateTime<Utc>,
    ) -> Option<TagInfo> {
        let releases = self
            .client
            .fetch_releases_since(&self.repo_info, since)
            .await;

        for release in releases {
            if let Some(tag_info) = self
                .client
                .fetch_tag_info_for_release(&release, &self.repo_info, commit_hash)
                .await
            {
                // Found a release containing the commit
                if tag_info.is_semver() {
                    // Semver releases are preferred, stop here
                    return Some(tag_info);
                }
                // Continue looking for semver, but remember this one
                return Some(tag_info);
            }
        }

        None
    }
}

#[async_trait]
impl Backend for GitHubBackend {
    fn repo_info(&self) -> Option<&GhRepoInfo> {
        Some(&self.repo_info)
    }

    async fn for_repo(&self, repo_info: &GhRepoInfo) -> Option<Box<dyn Backend>> {
        // Spawn a new backend with shared client for cross-project refs
        Some(Box::new(Self::with_client(
            Arc::clone(&self.client),
            repo_info.clone(),
        )))
    }

    // ============================================
    // Commit operations
    // ============================================

    async fn find_commit(&self, hash: &str) -> WtgResult<CommitInfo> {
        self.client
            .fetch_commit_full_info(&self.repo_info, hash)
            .await
            .ok_or_else(|| WtgError::NotFound(hash.to_string()))
    }

    // ============================================
    // Issue/PR operations
    // ============================================

    async fn fetch_issue(&self, number: u64) -> WtgResult<ExtendedIssueInfo> {
        self.client
            .fetch_issue(&self.repo_info, number)
            .await
            .ok_or_else(|| WtgError::NotFound(format!("Issue #{number}")))
    }

    async fn fetch_pr(&self, number: u64) -> WtgResult<PullRequestInfo> {
        self.client
            .fetch_pr(&self.repo_info, number)
            .await
            .ok_or_else(|| WtgError::NotFound(format!("PR #{number}")))
    }

    // ============================================
    // Release operations
    // ============================================

    async fn find_release_for_commit(
        &self,
        commit_hash: &str,
        commit_date: Option<DateTime<Utc>>,
    ) -> Option<TagInfo> {
        let since = commit_date.unwrap_or_else(Utc::now);
        self.find_release_for_commit_impl(commit_hash, since).await
    }

    // ============================================
    // URL generation
    // ============================================

    fn commit_url(&self, hash: &str) -> Option<String> {
        Some(GitHubClient::commit_url(&self.repo_info, hash))
    }

    fn tag_url(&self, tag: &str) -> Option<String> {
        Some(GitHubClient::tag_url(&self.repo_info, tag))
    }

    fn author_url_from_email(&self, email: &str) -> Option<String> {
        // GitHub emails are typically: username@users.noreply.github.com
        // Or: id+username@users.noreply.github.com
        if email.ends_with("@users.noreply.github.com") {
            let parts: Vec<&str> = email.split('@').collect();
            if let Some(user_part) = parts.first()
                && let Some(username) = user_part.split('+').next_back()
            {
                return Some(GitHubClient::profile_url(username));
            }
        }
        None
    }
}
