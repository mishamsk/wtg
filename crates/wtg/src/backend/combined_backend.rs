//! Combined backend using both local git and GitHub API.
//!
//! This backend uses optimal paths for each operation:
//! - Commits: Local git first (fast), fallback to API
//! - Files: Local git only (API would be too slow)
//! - PRs/Issues: GitHub API only
//! - Releases: Local tags + GitHub API for metadata

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::backend::{Backend, git_backend::GitBackend, github_backend::GitHubBackend};
use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo};
use crate::github::{ExtendedIssueInfo, PullRequestInfo};
use crate::notice::{Notice, NoticeCallback, no_notices};
use crate::parse_input::{ParsedQuery, Query};
use crate::release_filter::ReleaseFilter;

/// Combined backend using both local git and GitHub API.
///
/// Chooses the optimal path for each operation:
/// - Uses local git for fast commit/file/tag lookups
/// - Uses GitHub API for PR/issue data and release metadata
/// - Enriches local data with API data when beneficial
///
/// This backend overrides the individual trait methods with fallback logic,
/// then uses the default `resolve()` implementation which calls those methods.
/// This ensures fallback works correctly for all query types.
pub(crate) struct CombinedBackend {
    git: GitBackend,
    github: GitHubBackend,
    notice_cb: NoticeCallback,
}

impl CombinedBackend {
    /// Create a new `CombinedBackend` from git and GitHub backends.
    #[must_use]
    pub(crate) fn new(git: GitBackend, github: GitHubBackend) -> Self {
        Self {
            git,
            github,
            notice_cb: no_notices(),
        }
    }

    /// Set the notice callback for emitting operational messages.
    pub(crate) fn set_notice_callback(&mut self, cb: NoticeCallback) {
        self.notice_cb = cb.clone();
        self.git.set_notice_callback(cb.clone());
        self.github.set_notice_callback(cb);
    }

    /// Emit a notice via the callback.
    fn emit(&self, notice: Notice) {
        (self.notice_cb)(notice);
    }

    /// Find the best release/tag for a commit using both local and API data.
    ///
    /// Strategy:
    /// 1. Get local tag candidates containing the commit
    /// 2. Apply filter to candidates
    /// 3. Enrich candidates with GitHub release metadata
    /// 4. Pick best tag (prefer semver releases)
    /// 5. If no local candidates, fall back to GitHub API release search
    #[allow(clippy::too_many_lines)]
    async fn find_release_combined(
        &self,
        commit_hash: &str,
        commit_date: Option<DateTime<Utc>>,
        filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        let repo = self.git.git_repo();
        let gh_repo_info = self.github.repo_info();
        let client = self.github.client();

        // Fast path for specific tag lookup
        if let Some(tag_name) = filter.specific_tag() {
            // Try local tag first
            if let Some(tag) = repo.get_tags().into_iter().find(|t| t.name == tag_name)
                && repo.tag_contains_commit(&tag.commit_hash, commit_hash)
            {
                // Enrich with release info if available
                let mut result = tag;
                if let Some(release) = client.fetch_release_by_tag(gh_repo_info, tag_name).await {
                    result.is_release = true;
                    result.release_name.clone_from(&release.name);
                    result.release_url = Some(release.url);
                    result.published_at = release.published_at;
                }
                return Some(result);
            }
            // Fall back to API for specific tag check
            return self
                .github
                .find_release_for_commit(commit_hash, commit_date, filter)
                .await;
        }

        // Get local tag candidates (ensure_tags is called internally)
        let candidates = repo.tags_containing_commit(commit_hash);

        // Apply filter to candidates
        let filtered_candidates = filter.filter_tags(candidates);

        let has_semver = filtered_candidates.iter().any(TagInfo::is_semver);

        // Build timestamp map for sorting
        let timestamps: HashMap<String, i64> = filtered_candidates
            .iter()
            .map(|tag| {
                (
                    tag.commit_hash.clone(),
                    repo.get_commit_timestamp(&tag.commit_hash),
                )
            })
            .collect();

        // Enrich candidates with release metadata from GitHub
        let mut enriched_candidates = filtered_candidates.clone();
        if !filtered_candidates.is_empty() {
            let target_names: Vec<_> = if has_semver {
                filtered_candidates
                    .iter()
                    .filter(|c| c.is_semver())
                    .map(|c| c.name.clone())
                    .collect()
            } else {
                filtered_candidates.iter().map(|c| c.name.clone()).collect()
            };

            for tag_name in &target_names {
                if let Some(release) = client.fetch_release_by_tag(gh_repo_info, tag_name).await {
                    // Find the candidate with matching name and enrich it
                    if let Some(candidate) =
                        enriched_candidates.iter_mut().find(|c| &c.name == tag_name)
                    {
                        candidate.is_release = true;
                        candidate.release_name.clone_from(&release.name);
                        candidate.release_url = Some(release.url.clone());
                        candidate.published_at = release.published_at;
                        candidate.tag_url = Some(release.url);
                    }
                }
            }
        }

        // Pick best from local candidates
        let local_best = Self::pick_best_tag(&enriched_candidates, &timestamps);

        // If we have a semver tag, prefer it
        if has_semver {
            return local_best;
        }

        // Otherwise, try fetching releases from API as fallback
        let skip_prereleases = filter.skips_prereleases();
        if filtered_candidates.is_empty()
            && let Some(since) = commit_date
        {
            let releases = client.fetch_releases_since(gh_repo_info, since).await;
            let mut api_candidates: Vec<TagInfo> = Vec::new();

            for release in releases {
                // Early filter: skip prereleases BEFORE making expensive API calls.
                // This is intentionally separate from filter_tags() which operates on
                // already-fetched TagInfo objects.
                if skip_prereleases && release.prerelease {
                    continue;
                }

                // Try local tag first
                if let Some(mut tag) = repo.tag_from_release(&release)
                    && repo.tag_contains_commit(&tag.commit_hash, commit_hash)
                {
                    tag.is_release = true;
                    tag.release_name.clone_from(&release.name);
                    tag.release_url = Some(release.url.clone());
                    tag.published_at = release.published_at;
                    api_candidates.push(tag);
                    continue;
                }

                // Fallback to API check
                if let Some(tag) = client
                    .fetch_tag_info_for_release(&release, gh_repo_info, commit_hash)
                    .await
                {
                    api_candidates.push(tag);
                }
            }

            // Pick best from API candidates using same logic as local candidates
            if !api_candidates.is_empty() {
                let api_timestamps: HashMap<String, i64> = api_candidates
                    .iter()
                    .map(|tag| {
                        (
                            tag.commit_hash.clone(),
                            repo.get_commit_timestamp(&tag.commit_hash),
                        )
                    })
                    .collect();
                return Self::pick_best_tag(&api_candidates, &api_timestamps);
            }
        }

        local_best
    }

    /// Pick the best tag from candidates based on priority rules.
    fn pick_best_tag(candidates: &[TagInfo], timestamps: &HashMap<String, i64>) -> Option<TagInfo> {
        fn select_with_pred<F>(
            candidates: &[TagInfo],
            timestamps: &HashMap<String, i64>,
            predicate: F,
        ) -> Option<TagInfo>
        where
            F: Fn(&TagInfo) -> bool,
        {
            candidates
                .iter()
                .filter(|tag| predicate(tag))
                .min_by_key(|tag| {
                    timestamps
                        .get(&tag.commit_hash)
                        .copied()
                        .unwrap_or(i64::MAX)
                })
                .cloned()
        }

        // Priority: released semver > unreleased semver > released non-semver > unreleased non-semver
        select_with_pred(candidates, timestamps, |t| t.is_release && t.is_semver())
            .or_else(|| {
                select_with_pred(candidates, timestamps, |t| !t.is_release && t.is_semver())
            })
            .or_else(|| {
                select_with_pred(candidates, timestamps, |t| t.is_release && !t.is_semver())
            })
            .or_else(|| {
                select_with_pred(candidates, timestamps, |t| !t.is_release && !t.is_semver())
            })
    }
}

#[async_trait]
impl Backend for CombinedBackend {
    async fn backend_for_pr(&self, pr: &PullRequestInfo) -> Option<Box<dyn Backend>> {
        let pr_repo = pr.repo_info.as_ref()?;
        let our_repo = self.github.repo_info();

        // Same repo? No need for cross-project backend
        if pr_repo.owner() == our_repo.owner() && pr_repo.repo() == our_repo.repo() {
            return None;
        }

        // Create GitHubBackend with shared client
        let github = GitHubBackend::with_client(Arc::clone(self.github.client()), pr_repo.clone());

        // Try to create GitRepo for cross-project git operations
        match GitRepo::remote(pr_repo.clone()) {
            Ok(git_repo) => {
                let git = GitBackend::new(git_repo);
                Some(Box::new(Self::new(git, github)))
            }
            Err(e) => {
                self.emit(Notice::CrossProjectFallbackToApi {
                    owner: pr_repo.owner().to_string(),
                    repo: pr_repo.repo().to_string(),
                    error: e.to_string(),
                });
                Some(Box::new(github))
            }
        }
    }

    // ============================================
    // Commit operations - local first, fallback to API
    // ============================================

    async fn find_commit(&self, hash: &str) -> WtgResult<CommitInfo> {
        // Try local first (fast)
        match self.git.find_commit(hash).await {
            Ok(commit) => Ok(commit),
            Err(WtgError::NotFound(_)) => {
                // Commit might be in remote but not pulled - try GitHub API
                self.github.find_commit(hash).await
            }
            Err(e) => Err(e),
        }
    }

    async fn enrich_commit(&self, mut commit: CommitInfo) -> CommitInfo {
        // Already enriched?
        if commit.commit_url.is_some() && commit.author_url.is_some() {
            return commit;
        }

        let gh_repo_info = self.github.repo_info();

        // Try to extract username from email first (cheap, no API call)
        if commit.author_url.is_none()
            && let Some(email) = commit.author_email.as_deref()
        {
            commit.author_url = self.author_url_from_email(email);
        }

        // Add commit URL if missing
        if commit.commit_url.is_none() {
            commit.commit_url = self.commit_url(&commit.hash);
        }

        // If still missing author info, try API
        if (commit.author_url.is_none() || commit.author_login.is_none())
            && let Some(enriched) = self
                .github
                .client()
                .fetch_commit_full_info(gh_repo_info, &commit.hash)
                .await
        {
            commit.author_login = enriched.author_login;
            if commit.author_url.is_none() {
                commit.author_url = enriched.author_url;
            }
        }

        commit
    }

    // ============================================
    // File operations - local only
    // ============================================

    async fn find_file(&self, branch: &str, path: &str) -> WtgResult<FileInfo> {
        // Only git has efficient file history
        self.git.find_file(branch, path).await
    }

    // ============================================
    // Tag/Release operations - combined
    // ============================================

    async fn find_tag(&self, name: &str) -> WtgResult<TagInfo> {
        let mut tag = self.git.find_tag(name).await?;

        // Check if tag has a GitHub release (git backend can't determine this)
        if let Some(release) = self
            .github
            .client()
            .fetch_release_by_tag(self.github.repo_info(), name)
            .await
        {
            tag.is_release = true;
            tag.release_name = release.name;
            tag.release_url = Some(release.url.clone());
            tag.published_at = release.published_at;
            tag.tag_url = Some(release.url);
        } else {
            // Plain tag - set tag_url to tree view
            tag.tag_url = self.tag_url(name);
        }

        Ok(tag)
    }

    async fn find_previous_tag(&self, tag_name: &str) -> WtgResult<Option<TagInfo>> {
        // Use git backend for local tag lookup (faster)
        self.git.find_previous_tag(tag_name).await
    }

    async fn commits_between_tags(
        &self,
        from_tag: &str,
        to_tag: &str,
        limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        // Try git first, fall back to GitHub
        match self.git.commits_between_tags(from_tag, to_tag, limit).await {
            Ok(commits) if !commits.is_empty() => Ok(commits),
            _ => {
                self.github
                    .commits_between_tags(from_tag, to_tag, limit)
                    .await
            }
        }
    }

    async fn find_release_for_commit(
        &self,
        commit_hash: &str,
        commit_date: Option<DateTime<Utc>>,
        filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        self.find_release_combined(commit_hash, commit_date, filter)
            .await
    }

    async fn fetch_release_body(&self, tag_name: &str) -> Option<String> {
        self.github.fetch_release_body(tag_name).await
    }

    async fn changelog_for_version(&self, version: &str) -> Option<String> {
        // Try git backend first (local filesystem is faster)
        if let Some(content) = self.git.changelog_for_version(version).await {
            return Some(content);
        }
        // Fallback to GitHub API
        self.github.changelog_for_version(version).await
    }

    async fn disambiguate_query(&self, query: &ParsedQuery) -> WtgResult<Query> {
        match query {
            ParsedQuery::Resolved(resolved) => Ok(resolved.clone()),
            ParsedQuery::Unknown(input) => {
                // Try git disambiguation first (tag, file, commit)
                if let Ok(q) = self.git.disambiguate_query(query).await {
                    return Ok(q);
                }
                // Fall back: treat numeric input as issue/PR number
                if let Ok(number) = input.parse::<u64>() {
                    return Ok(Query::IssueOrPr(number));
                }
                Err(WtgError::NotFound(input.clone()))
            }
            ParsedQuery::UnknownPath { .. } => self.git.disambiguate_query(query).await,
        }
    }

    // ============================================
    // Issue/PR operations - GitHub API only
    // ============================================

    async fn fetch_issue(&self, number: u64) -> WtgResult<ExtendedIssueInfo> {
        self.github.fetch_issue(number).await
    }

    async fn fetch_pr(&self, number: u64) -> WtgResult<PullRequestInfo> {
        self.github.fetch_pr(number).await
    }

    // ============================================
    // URL generation - delegate to GitHub backend
    // ============================================

    fn commit_url(&self, hash: &str) -> Option<String> {
        self.github.commit_url(hash)
    }

    fn tag_url(&self, tag: &str) -> Option<String> {
        self.github.tag_url(tag)
    }

    fn release_tag_url(&self, tag: &str) -> Option<String> {
        self.github.release_tag_url(tag)
    }

    fn author_url_from_email(&self, email: &str) -> Option<String> {
        self.github.author_url_from_email(email)
    }
}
