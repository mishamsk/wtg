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
use crate::git::{CommitInfo, TagInfo, looks_like_commit_hash};
use crate::github::{ExtendedIssueInfo, GhRepoInfo, GitHubClient, PullRequestInfo};
use crate::parse_input::{ParsedQuery, Query};

/// Pure GitHub API backend.
///
/// Uses `GitHubClient` for all operations. Cannot perform local git operations,
/// so file queries will return `Unsupported`.
pub(crate) struct GitHubBackend {
    client: Arc<GitHubClient>,
    gh_repo_info: GhRepoInfo,
}

impl GitHubBackend {
    /// Create a new `GitHubBackend` for a repository.
    ///
    /// Returns `None` if no GitHub client can be created.
    #[must_use]
    pub(crate) fn new(gh_repo_info: GhRepoInfo) -> Option<Self> {
        Some(Self {
            client: Arc::new(GitHubClient::new()?),
            gh_repo_info,
        })
    }

    /// Create a `GitHubBackend` with a shared client.
    #[must_use]
    pub(crate) const fn with_client(client: Arc<GitHubClient>, gh_repo_info: GhRepoInfo) -> Self {
        Self {
            client,
            gh_repo_info,
        }
    }

    /// Get a reference to the Arc-wrapped `GitHubClient`.
    #[must_use]
    pub(crate) const fn client(&self) -> &Arc<GitHubClient> {
        &self.client
    }

    /// Get a reference to the repository info (internal use only).
    #[must_use]
    pub(crate) const fn repo_info(&self) -> &GhRepoInfo {
        &self.gh_repo_info
    }

    /// Find release for a commit by iterating through releases.
    async fn find_release_for_commit_impl(
        &self,
        commit_hash: &str,
        since: DateTime<Utc>,
    ) -> Option<TagInfo> {
        let releases = self
            .client
            .fetch_releases_since(&self.gh_repo_info, since)
            .await;

        for release in releases {
            if let Some(tag_info) = self
                .client
                .fetch_tag_info_for_release(&release, &self.gh_repo_info, commit_hash)
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
    async fn backend_for_pr(&self, pr: &PullRequestInfo) -> Option<Box<dyn Backend>> {
        let pr_repo = pr.repo_info.as_ref()?;

        // Same repo? No need for cross-project backend
        if pr_repo.owner() == self.gh_repo_info.owner()
            && pr_repo.repo() == self.gh_repo_info.repo()
        {
            return None;
        }

        // Spawn a new backend with shared client for cross-project refs
        Some(Box::new(Self::with_client(
            Arc::clone(&self.client),
            pr_repo.clone(),
        )))
    }

    // ============================================
    // Commit operations
    // ============================================

    async fn find_commit(&self, hash: &str) -> WtgResult<CommitInfo> {
        self.client
            .fetch_commit_full_info(&self.gh_repo_info, hash)
            .await
            .ok_or_else(|| WtgError::NotFound(hash.to_string()))
    }

    // ============================================
    // Issue/PR operations
    // ============================================

    async fn fetch_issue(&self, number: u64) -> WtgResult<ExtendedIssueInfo> {
        self.client
            .fetch_issue(&self.gh_repo_info, number)
            .await
            .ok_or_else(|| WtgError::NotFound(format!("Issue #{number}")))
    }

    async fn fetch_pr(&self, number: u64) -> WtgResult<PullRequestInfo> {
        self.client
            .fetch_pr(&self.gh_repo_info, number)
            .await
            .ok_or_else(|| WtgError::NotFound(format!("PR #{number}")))
    }

    // ============================================
    // Tag/Release operations
    // ============================================

    async fn find_tag(&self, name: &str) -> WtgResult<TagInfo> {
        self.client
            .fetch_tag(&self.gh_repo_info, name)
            .await
            .ok_or_else(|| WtgError::NotFound(format!("Tag {name}")))
    }

    async fn find_release_for_commit(
        &self,
        commit_hash: &str,
        commit_date: Option<DateTime<Utc>>,
    ) -> Option<TagInfo> {
        let since = commit_date.unwrap_or_else(Utc::now);
        self.find_release_for_commit_impl(commit_hash, since).await
    }

    async fn disambiguate_query(&self, query: &ParsedQuery) -> WtgResult<Query> {
        match query {
            ParsedQuery::Resolved(resolved) => Ok(resolved.clone()),
            ParsedQuery::Unknown(input) => {
                if looks_like_commit_hash(input) && self.find_commit(input).await.is_ok() {
                    return Ok(Query::GitCommit(input.clone()));
                }
                Err(WtgError::NotFound(input.clone()))
            }
            ParsedQuery::UnknownPath { segments } => Err(WtgError::NotFound(segments.join("/"))),
        }
    }

    // ============================================
    // URL generation
    // ============================================

    fn commit_url(&self, hash: &str) -> Option<String> {
        Some(GitHubClient::commit_url(&self.gh_repo_info, hash))
    }

    fn tag_url(&self, tag: &str) -> Option<String> {
        Some(GitHubClient::tag_url(&self.gh_repo_info, tag))
    }

    fn author_url_from_email(&self, email: &str) -> Option<String> {
        GitHubClient::author_url_from_email(email)
    }
}
