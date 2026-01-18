//! Pure local git backend implementation.
//!
//! This backend wraps a `GitRepo` and provides git-only operations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;

use super::{Backend, NoticeCallback};
use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo, looks_like_commit_hash};
use crate::github::GitHubClient;
use crate::parse_input::{ParsedQuery, Query};

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

    /// Find tags containing a commit and pick the best one.
    fn find_best_tag_for_commit(&self, commit_hash: &str) -> Option<TagInfo> {
        let candidates = self.repo.tags_containing_commit(commit_hash);
        if candidates.is_empty() {
            return None;
        }

        // Build timestamp map for sorting
        let timestamps: HashMap<String, i64> = candidates
            .iter()
            .map(|tag| {
                (
                    tag.commit_hash.clone(),
                    self.repo.get_commit_timestamp(&tag.commit_hash),
                )
            })
            .collect();

        // Pick best tag: prefer semver releases, then semver, then any release, then any
        Self::pick_best_tag(&candidates, &timestamps)
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
    ) -> Option<TagInfo> {
        self.find_best_tag_for_commit(commit_hash)
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

    fn author_url_from_email(&self, email: &str) -> Option<String> {
        GitHubClient::author_url_from_email(email)
    }
}
