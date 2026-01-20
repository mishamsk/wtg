//! Backend trait abstraction for git/GitHub operations.
//!
//! This module provides a trait-based abstraction over data sources (local git, GitHub API, or both),
//! enabling:
//! - Cross-project references (issues referencing PRs in different repos)
//! - Future non-GitHub hosting support
//! - Optimal path selection when both local and remote sources are available

mod combined_backend;
mod git_backend;
mod github_backend;

pub(crate) use combined_backend::CombinedBackend;
pub use git_backend::GitBackend;
pub(crate) use github_backend::GitHubBackend;

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo};
use crate::github::{ExtendedIssueInfo, PullRequestInfo};
use crate::notice::{Notice, NoticeCallback, no_notices};
use crate::parse_input::{ParsedInput, ParsedQuery, Query};
use crate::release_filter::ReleaseFilter;
use crate::remote::{RemoteHost, RemoteInfo};

/// Unified backend trait for all git/GitHub operations.
///
/// Backends implement methods for operations they support. Default implementations
/// return `WtgError::Unsupported` for operations not available.
#[async_trait]
pub trait Backend: Send + Sync {
    // ============================================
    // Cross-project support (default: not supported)
    // ============================================

    /// Get a backend for fetching PR data if the PR is from a different repository.
    /// Returns None if same repo or cross-project not supported.
    async fn backend_for_pr(&self, _pr: &PullRequestInfo) -> Option<Box<dyn Backend>> {
        None
    }

    // ============================================
    // Commit operations (default: Unsupported)
    // ============================================

    /// Find commit by hash (short or full).
    async fn find_commit(&self, _hash: &str) -> WtgResult<CommitInfo> {
        Err(WtgError::Unsupported("commit lookup".into()))
    }

    /// Enrich commit with additional info (author URLs, commit URL, etc.).
    async fn enrich_commit(&self, commit: CommitInfo) -> CommitInfo {
        commit
    }

    /// Find commit info from a PR (using merge commit SHA).
    async fn find_commit_for_pr(&self, pr: &PullRequestInfo) -> WtgResult<CommitInfo> {
        if let Some(ref sha) = pr.merge_commit_sha {
            self.find_commit(sha).await
        } else {
            Err(WtgError::NotFound("PR has no merge commit".into()))
        }
    }

    // ============================================
    // File operations (default: Unsupported)
    // ============================================

    /// Find file and its history in the repository.
    async fn find_file(&self, _branch: &str, _path: &str) -> WtgResult<FileInfo> {
        Err(WtgError::Unsupported("file lookup".into()))
    }

    // ============================================
    // Tag/Release operations (default: Unsupported)
    // ============================================

    /// Find a specific tag by name.
    async fn find_tag(&self, _name: &str) -> WtgResult<TagInfo> {
        Err(WtgError::Unsupported("tag lookup".into()))
    }

    /// Find the previous tag before the given tag.
    ///
    /// For semver tags, returns the immediately preceding version by semver ordering.
    /// For non-semver tags, returns the most recent tag pointing to an earlier commit.
    async fn find_previous_tag(&self, _tag_name: &str) -> WtgResult<Option<TagInfo>> {
        Err(WtgError::Unsupported("find previous tag".into()))
    }

    /// Get commits between two tags (`from_tag` exclusive, `to_tag` inclusive).
    ///
    /// Returns up to `limit` commits, most recent first.
    async fn commits_between_tags(
        &self,
        _from_tag: &str,
        _to_tag: &str,
        _limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        Err(WtgError::Unsupported("commits between tags".into()))
    }

    /// Disambiguate a parsed query into a concrete query.
    async fn disambiguate_query(&self, query: &ParsedQuery) -> WtgResult<Query> {
        match query {
            ParsedQuery::Resolved(resolved) => Ok(resolved.clone()),
            ParsedQuery::Unknown(input) => Err(WtgError::NotFound(input.clone())),
            ParsedQuery::UnknownPath { segments } => Err(WtgError::NotFound(segments.join("/"))),
        }
    }

    /// Find a release/tag that contains the given commit.
    ///
    /// The `filter` parameter controls which tags are considered:
    /// - `Unrestricted`: All tags (default behavior)
    /// - `SkipPrereleases`: Filter out pre-release versions
    /// - `Specific(tag)`: Check if the commit is in a specific tag
    async fn find_release_for_commit(
        &self,
        _commit_hash: &str,
        _commit_date: Option<DateTime<Utc>>,
        _filter: &ReleaseFilter,
    ) -> Option<TagInfo> {
        None
    }

    /// Fetch the body/description of a GitHub release by tag name.
    async fn fetch_release_body(&self, _tag_name: &str) -> Option<String> {
        None
    }

    /// Parse changelog for a specific version from repository root.
    ///
    /// Returns the changelog section content for the given version, or None if
    /// not found. Backends implement this to access CHANGELOG.md via their
    /// native method (local filesystem or API).
    async fn changelog_for_version(&self, _version: &str) -> Option<String> {
        None
    }

    // ============================================
    // Issue operations (default: Unsupported)
    // ============================================

    /// Fetch issue details including closing PRs.
    async fn fetch_issue(&self, _number: u64) -> WtgResult<ExtendedIssueInfo> {
        Err(WtgError::Unsupported("issue lookup".into()))
    }

    // ============================================
    // Pull request operations (default: Unsupported)
    // ============================================

    /// Fetch PR details.
    async fn fetch_pr(&self, _number: u64) -> WtgResult<PullRequestInfo> {
        Err(WtgError::Unsupported("PR lookup".into()))
    }

    // ============================================
    // URL generation (default: None)
    // ============================================

    /// Generate URL to view a commit.
    fn commit_url(&self, _hash: &str) -> Option<String> {
        None
    }

    /// Generate URL to view a tag (tree view for plain git tags).
    fn tag_url(&self, _tag: &str) -> Option<String> {
        None
    }

    /// Generate URL to view a release (releases page for tags with releases).
    fn release_tag_url(&self, _tag: &str) -> Option<String> {
        None
    }

    /// Generate author profile URL from email address.
    fn author_url_from_email(&self, _email: &str) -> Option<String> {
        None
    }
}

// ============================================
// Backend resolution
// ============================================

/// Resolve the best backend based on available resources.
///
/// # Arguments
/// * `parsed_input` - The parsed user input
/// * `allow_user_repo_fetch` - If true, allow fetching into user's local repo
///
/// Decision tree:
/// 1. Explicit repo info provided → Use cached/cloned repo + GitHub API (hard error if GitHub client fails)
/// 2. In local repo with GitHub remote → Combined backend (soft notice if GitHub client fails)
/// 3. In local repo without GitHub remote → Git-only backend with appropriate notice
/// 4. Not in repo and no info → Error
pub fn resolve_backend(
    parsed_input: &ParsedInput,
    allow_user_repo_fetch: bool,
) -> WtgResult<Box<dyn Backend>> {
    resolve_backend_with_notices(parsed_input, allow_user_repo_fetch, no_notices())
}

/// Resolve the best backend based on available resources, with a notice callback.
pub fn resolve_backend_with_notices(
    parsed_input: &ParsedInput,
    allow_user_repo_fetch: bool,
    notice_cb: NoticeCallback,
) -> WtgResult<Box<dyn Backend>> {
    // Case 1: Explicit repo info provided (from URL/flags)
    if let Some(repo_info) = parsed_input.gh_repo_info() {
        // User explicitly provided GitHub info - GitHub client failure is a hard error
        let github = GitHubBackend::new(repo_info.clone()).ok_or(WtgError::GitHubClientFailed)?;

        // Try to get local git repo for combined backend
        if let Ok(git_repo) = GitRepo::remote_with_notices(repo_info.clone(), notice_cb.clone()) {
            let git = GitBackend::new(git_repo);
            let mut combined = CombinedBackend::new(git, github);
            combined.set_notice_callback(notice_cb);
            Ok(Box::new(combined))
        } else {
            // Can't access git locally, use pure API (soft notice)
            notice_cb(Notice::ApiOnly);
            Ok(Box::new(github))
        }
    } else {
        // Case 2: Local repo detection
        resolve_local_backend_with_notices(allow_user_repo_fetch, notice_cb)
    }
}

fn resolve_local_backend_with_notices(
    allow_user_repo_fetch: bool,
    notice_cb: NoticeCallback,
) -> WtgResult<Box<dyn Backend>> {
    let mut git_repo = GitRepo::open()?;
    if allow_user_repo_fetch {
        git_repo.set_allow_fetch(true);
    }
    git_repo.set_notice_callback(notice_cb.clone());

    // Collect and sort remotes by priority (upstream > origin > other, GitHub first)
    let mut remotes: Vec<RemoteInfo> = git_repo.remotes().collect();
    remotes.sort_by_key(RemoteInfo::priority);

    // No remotes at all
    if remotes.is_empty() {
        notice_cb(Notice::NoRemotes);
        let git = GitBackend::new(git_repo);
        return Ok(Box::new(git));
    }

    // Find the best GitHub remote (if any)
    let github_remote = remotes
        .iter()
        .find(|r| r.host == Some(RemoteHost::GitHub))
        .cloned();

    if let Some(github_remote) = github_remote {
        // We have a GitHub remote - try to use it
        if let Some(repo_info) = git_repo.github_remote() {
            let git = GitBackend::new(git_repo);

            if let Some(github) = GitHubBackend::new(repo_info) {
                // Full GitHub support!
                let mut combined = CombinedBackend::new(git, github);
                combined.set_notice_callback(notice_cb);
                return Ok(Box::new(combined));
            }

            // GitHub remote found but client creation failed
            notice_cb(Notice::UnreachableGitHub {
                remote: github_remote,
            });
            return Ok(Box::new(git));
        }
    }

    // No GitHub remote - analyze what we have
    let git = GitBackend::new(git_repo);
    let unique_hosts: HashSet<Option<RemoteHost>> = remotes.iter().map(|r| r.host).collect();

    // Check if we have mixed hosts (excluding None/unknown)
    let known_hosts: Vec<RemoteHost> = unique_hosts.iter().filter_map(|h| *h).collect();

    if known_hosts.len() > 1 {
        // Multiple different known hosts - we're lost!
        notice_cb(Notice::MixedRemotes {
            hosts: known_hosts,
            count: remotes.len(),
        });
        return Ok(Box::new(git));
    }

    // Single host type (or all unknown) - return the best one
    let best_remote = remotes.into_iter().next().unwrap();
    notice_cb(Notice::UnsupportedHost { best_remote });
    Ok(Box::new(git))
}
