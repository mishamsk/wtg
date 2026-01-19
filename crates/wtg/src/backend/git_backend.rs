//! Pure local git backend implementation.
//!
//! This backend wraps a `GitRepo` and provides git-only operations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;

use super::{Backend, NoticeCallback};
use crate::changelog;
use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo, looks_like_commit_hash};
use crate::github::GitHubClient;
use crate::parse_input::{ParsedQuery, Query};
use crate::release_filter::ReleaseFilter;

/// Pure local git backend wrapping a `GitRepo`.
///
/// Uses `GitRepo` for all operations including smart fetching.
/// Cannot access GitHub API, so PR/Issue queries will return `Unsupported`.
pub struct GitBackend {
    repo: GitRepo,
}

impl GitBackend {
    /// Create a `GitBackend` from an existing `GitRepo`.
    #[must_use]
    pub const fn new(repo: GitRepo) -> Self {
        Self { repo }
    }

    /// Get a reference to the underlying `GitRepo`.
    pub const fn git_repo(&self) -> &GitRepo {
        &self.repo
    }

    /// Set the notice callback for emitting operational messages.
    pub fn set_notice_callback(&mut self, cb: NoticeCallback) {
        self.repo.set_notice_callback(cb);
    }

    /// Find tags containing a commit and pick the best one, applying the filter.
    fn find_best_tag_for_commit(
        &self,
        commit_hash: &str,
        filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        // Fast path for specific tag lookup
        if let Some(tag_name) = filter.specific_tag() {
            // Find the tag first
            let tag = self
                .repo
                .get_tags()
                .into_iter()
                .find(|t| t.name == tag_name)?;

            // Check if the commit is in this tag
            if self.repo.tag_contains_commit(&tag.commit_hash, commit_hash) {
                return Some(tag);
            }
            return None;
        }

        let candidates = self.repo.tags_containing_commit(commit_hash);
        if candidates.is_empty() {
            return None;
        }

        // Apply filter to candidates
        let filtered = filter.filter_tags(candidates);

        if filtered.is_empty() {
            return None;
        }

        // Build timestamp map for sorting
        let timestamps: HashMap<String, i64> = filtered
            .iter()
            .map(|tag| {
                (
                    tag.commit_hash.clone(),
                    self.repo.get_commit_timestamp(&tag.commit_hash),
                )
            })
            .collect();

        // Pick best tag: prefer semver releases, then semver, then any release, then any
        Self::pick_best_tag(&filtered, &timestamps)
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

    fn disambiguate_input_string(&self, input: &str) -> WtgResult<Query> {
        if self.repo.get_tags().iter().any(|tag| tag.name == input) {
            return Ok(Query::Tag(input.to_string()));
        }

        if self.repo.has_path_at_head(input) {
            return Ok(Query::FilePath {
                branch: "HEAD".to_string(),
                path: PathBuf::from(input),
            });
        }

        if looks_like_commit_hash(input) && self.repo.find_commit_local(input).is_some() {
            return Ok(Query::GitCommit(input.to_string()));
        }

        Err(WtgError::NotFound(input.to_string()))
    }

    fn disambiguate_unknown_path(&self, segments: &[String]) -> Option<Query> {
        let (branch, remainder) = self.repo.find_branch_path_match(segments)?;
        let mut path = PathBuf::new();
        for segment in remainder {
            path.push(segment);
        }
        Some(Query::FilePath { branch, path })
    }
}

#[async_trait]
impl Backend for GitBackend {
    // Note: backend_for_pr() uses default (returns None) since GitBackend
    // doesn't have API access for cross-project resolution.

    // ============================================
    // Commit operations
    // ============================================

    async fn find_commit(&self, hash: &str) -> WtgResult<CommitInfo> {
        // Use smart find that can fetch on demand
        self.repo
            .find_commit(hash)?
            .ok_or_else(|| WtgError::NotFound(hash.to_string()))
    }

    async fn enrich_commit(&self, mut commit: CommitInfo) -> CommitInfo {
        // Add commit URL if we have repo info
        if commit.commit_url.is_none()
            && let Some(repo_info) = self.repo.github_remote()
        {
            commit.commit_url = Some(GitHubClient::commit_url(&repo_info, &commit.hash));
        }
        commit
    }

    // ============================================
    // File operations
    // ============================================

    async fn find_file(&self, branch: &str, path: &str) -> WtgResult<FileInfo> {
        self.repo
            .find_file_on_branch(branch, path)
            .ok_or_else(|| WtgError::NotFound(path.to_string()))
    }

    // ============================================
    // Tag/Release operations
    // ============================================

    async fn find_tag(&self, name: &str) -> WtgResult<TagInfo> {
        self.repo
            .get_tags()
            .into_iter()
            .find(|t| t.name == name)
            .ok_or_else(|| WtgError::NotFound(name.to_string()))
    }

    async fn find_previous_tag(&self, tag_name: &str) -> WtgResult<Option<TagInfo>> {
        let tags = self.repo.get_tags();
        let current_tag = tags.iter().find(|t| t.name == tag_name);

        let Some(current) = current_tag else {
            return Ok(None);
        };

        // If current is semver, find previous by semver ordering
        if current.is_semver() {
            let mut semver_tags: Vec<_> = tags.iter().filter(|t| t.is_semver()).collect();

            // Sort by semver (ascending)
            semver_tags.sort_by(|a, b| {
                let a_semver = a.semver_info.as_ref().unwrap();
                let b_semver = b.semver_info.as_ref().unwrap();
                a_semver.cmp(b_semver)
            });

            // Find current position and return previous
            if let Some(pos) = semver_tags.iter().position(|t| t.name == tag_name)
                && pos > 0
            {
                return Ok(Some(semver_tags[pos - 1].clone()));
            }
            return Ok(None);
        }

        // Non-semver: find most recent tag on an earlier commit
        let current_timestamp = self.repo.get_commit_timestamp(&current.commit_hash);

        let mut candidates: Vec<_> = tags
            .iter()
            .filter(|t| t.name != tag_name)
            .filter(|t| t.commit_hash != current.commit_hash)
            .filter(|t| self.repo.get_commit_timestamp(&t.commit_hash) < current_timestamp)
            .collect();

        // Sort by timestamp descending (most recent first)
        candidates.sort_by(|a, b| {
            let a_ts = self.repo.get_commit_timestamp(&a.commit_hash);
            let b_ts = self.repo.get_commit_timestamp(&b.commit_hash);
            b_ts.cmp(&a_ts)
        });

        Ok(candidates.first().map(|t| (*t).clone()))
    }

    async fn commits_between_tags(
        &self,
        from_tag: &str,
        to_tag: &str,
        limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        Ok(self.repo.commits_between(from_tag, to_tag, limit))
    }

    async fn disambiguate_query(&self, query: &ParsedQuery) -> WtgResult<Query> {
        match query {
            ParsedQuery::Resolved(resolved) => Ok(resolved.clone()),
            ParsedQuery::Unknown(input) => self.disambiguate_input_string(input),
            ParsedQuery::UnknownPath { segments } => self
                .disambiguate_unknown_path(segments)
                .ok_or_else(|| WtgError::NotFound(segments.join("/"))),
        }
    }

    async fn find_release_for_commit(
        &self,
        commit_hash: &str,
        _commit_date: Option<DateTime<Utc>>,
        filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        self.find_best_tag_for_commit(commit_hash, filter)
    }

    async fn changelog_for_version(&self, version: &str) -> Option<String> {
        changelog::parse_changelog_for_version(self.repo.path(), version)
    }

    // ============================================
    // URL generation
    // ============================================

    fn commit_url(&self, hash: &str) -> Option<String> {
        self.repo
            .github_remote()
            .map(|ri| GitHubClient::commit_url(&ri, hash))
    }

    fn tag_url(&self, tag: &str) -> Option<String> {
        self.repo
            .github_remote()
            .map(|ri| GitHubClient::tag_url(&ri, tag))
    }

    fn release_tag_url(&self, tag: &str) -> Option<String> {
        self.repo
            .github_remote()
            .map(|ri| GitHubClient::release_tag_url(&ri, tag))
    }

    fn author_url_from_email(&self, email: &str) -> Option<String> {
        GitHubClient::author_url_from_email(email)
    }
}
