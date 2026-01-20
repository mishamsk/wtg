//! Pure GitHub API backend implementation.
//!
//! This backend only uses GitHub API via `GitHubClient`.
//! It can fetch commits, PRs, issues, and releases, but cannot
//! efficiently walk file history or perform local git operations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

use super::Backend;
use crate::changelog;
use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, TagInfo, looks_like_commit_hash};
use crate::github::{ExtendedIssueInfo, GhRepoInfo, GitHubClient, PullRequestInfo};
use crate::notice::NoticeCallback;
use crate::parse_input::{ParsedQuery, Query};
use crate::release_filter::ReleaseFilter;

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

    /// Set the notice callback for the underlying client.
    pub(crate) fn set_notice_callback(&self, cb: NoticeCallback) {
        self.client.set_notice_callback(cb);
    }

    /// Find release for a commit by iterating through releases.
    async fn find_release_for_commit_impl(
        &self,
        commit_hash: &str,
        since: DateTime<Utc>,
        filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        // Fast path for specific tag lookup
        if let Some(tag_name) = filter.specific_tag() {
            // Fetch the specific tag
            let tag = self.client.fetch_tag(&self.gh_repo_info, tag_name).await?;

            // Check if commit is contained in this tag
            if self
                .client
                .tag_contains_commit(&self.gh_repo_info, tag_name, commit_hash)
                .await
            {
                return Some(tag);
            }
            return None;
        }

        let releases = self
            .client
            .fetch_releases_since(&self.gh_repo_info, since)
            .await;

        // Apply filter: skip prereleases if requested
        let skip_prereleases = filter.skips_prereleases();

        for release in releases {
            // Skip pre-releases if filter is active
            if skip_prereleases && release.prerelease {
                continue;
            }

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

    async fn find_previous_tag(&self, tag_name: &str) -> WtgResult<Option<TagInfo>> {
        // Fetch the current tag first
        let current = self.find_tag(tag_name).await?;

        // For GitHub, we need to list releases/tags and find the previous one
        // This is a simplified implementation - fetch recent releases
        let since = current.created_at - chrono::Duration::days(365);
        let releases = self
            .client
            .fetch_releases_since(&self.gh_repo_info, since)
            .await;

        if current.is_semver() {
            // Find previous by semver
            let mut semver_releases: Vec<_> = releases
                .iter()
                .filter(|r| crate::git::parse_semver(&r.tag_name).is_some())
                .collect();

            semver_releases.sort_by(|a, b| {
                let a_semver = crate::git::parse_semver(&a.tag_name).unwrap();
                let b_semver = crate::git::parse_semver(&b.tag_name).unwrap();
                a_semver.cmp(&b_semver)
            });

            if let Some(pos) = semver_releases.iter().position(|r| r.tag_name == tag_name)
                && pos > 0
            {
                let prev = &semver_releases[pos - 1];
                return self.find_tag(&prev.tag_name).await.map(Some);
            }
            return Ok(None);
        }

        // Non-semver: find by date
        let mut candidates: Vec<_> = releases
            .iter()
            .filter(|r| r.tag_name != tag_name)
            .filter(|r| r.created_at.is_some_and(|d| d < current.created_at))
            .collect();

        candidates.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        if let Some(prev) = candidates.first() {
            return self.find_tag(&prev.tag_name).await.map(Some);
        }

        Ok(None)
    }

    async fn commits_between_tags(
        &self,
        _from_tag: &str,
        _to_tag: &str,
        _limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        // GitHub compare API is not yet implemented in GitHubClient.
        // The CombinedBackend will use the git backend for this operation.
        Err(WtgError::Unsupported("GitHub commits between tags".into()))
    }

    async fn find_release_for_commit(
        &self,
        commit_hash: &str,
        commit_date: Option<DateTime<Utc>>,
        filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        let since = commit_date.unwrap_or_else(Utc::now);
        self.find_release_for_commit_impl(commit_hash, since, filter)
            .await
    }

    async fn fetch_release_body(&self, tag_name: &str) -> Option<String> {
        let release = self
            .client
            .fetch_release_by_tag(&self.gh_repo_info, tag_name)
            .await?;
        release.body.filter(|b| !b.trim().is_empty())
    }

    async fn changelog_for_version(&self, version: &str) -> Option<String> {
        // Try common CHANGELOG.md variations (case-insensitive on GitHub)
        for path in ["CHANGELOG.md", "changelog.md", "Changelog.md"] {
            if let Some(content) = self
                .client
                .fetch_file_content(&self.gh_repo_info, path)
                .await
                && let Some(section) = changelog::extract_version_section(&content, version)
            {
                return Some(section);
            }
        }
        None
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

    fn release_tag_url(&self, tag: &str) -> Option<String> {
        Some(GitHubClient::release_tag_url(&self.gh_repo_info, tag))
    }

    fn author_url_from_email(&self, email: &str) -> Option<String> {
        GitHubClient::author_url_from_email(email)
    }
}
