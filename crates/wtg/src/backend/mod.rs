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
pub(crate) use git_backend::GitBackend;
pub(crate) use github_backend::GitHubBackend;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, TagInfo};
use crate::github::{ExtendedIssueInfo, GhRepoInfo, PullRequestInfo};
use crate::parse_input::ParsedInput;
use crate::repo_manager::RepoManager;

/// Unified backend trait for all git/GitHub operations.
///
/// Backends implement methods for operations they support. Default implementations
/// return `WtgError::Unsupported` for operations not available.
#[async_trait]
pub(crate) trait Backend: Send + Sync {
    /// Get repository info if known (for URL building, cross-project refs).
    fn repo_info(&self) -> Option<&GhRepoInfo>;

    // ============================================
    // Cross-project support (default: not supported)
    // ============================================

    /// Spawn a backend for a different repository (for cross-project references).
    async fn for_repo(&self, _repo_info: &GhRepoInfo) -> Option<Box<dyn Backend>> {
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
    async fn find_file(&self, _path: &str) -> WtgResult<FileInfo> {
        Err(WtgError::Unsupported("file lookup".into()))
    }

    // ============================================
    // Tag/Release operations (default: Unsupported)
    // ============================================

    /// Find a specific tag by name.
    async fn find_tag(&self, _name: &str) -> WtgResult<TagInfo> {
        Err(WtgError::Unsupported("tag lookup".into()))
    }

    /// Find a release/tag that contains the given commit.
    async fn find_release_for_commit(
        &self,
        _commit_hash: &str,
        _commit_date: Option<DateTime<Utc>>,
    ) -> Option<TagInfo> {
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

    /// Generate URL to view a tag.
    fn tag_url(&self, _tag: &str) -> Option<String> {
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
/// 1. Explicit repo info provided → Use cached/cloned repo + GitHub API (or git-only if GitHub client fails)
/// 2. In local repo with GitHub remote → Combined backend (or git-only if GitHub client fails)
/// 3. In local repo without remote → Git-only backend
/// 4. Not in repo and no info → Error
pub(crate) fn resolve_backend(
    parsed_input: &ParsedInput,
    allow_user_repo_fetch: bool,
) -> WtgResult<Box<dyn Backend>> {
    if let Some(repo_info) = parsed_input.gh_repo_info() {
        // Explicit repo info - use RepoManager for cache handling
        // Remote repos always allow fetching to keep cache fresh
        let repo_manager = RepoManager::remote(repo_info.clone())?;
        let git = GitBackend::new(repo_manager);

        // Try creating GitHub backend, fall back to git-only with warning
        if let Some(github) = GitHubBackend::new(repo_info.clone()) {
            return Ok(Box::new(CombinedBackend::new(git, github)));
        }
        eprintln!("Warning: GitHub features unavailable (no client could be created)");
        return Ok(Box::new(git));
    }

    // No explicit repo info - must be in local repo
    let mut repo_manager = RepoManager::local()?;

    // Only allow fetching into user's repo if explicitly requested via --fetch
    if allow_user_repo_fetch {
        repo_manager.set_allow_fetch(true);
    }

    let git = GitBackend::new(repo_manager);

    // Try to find GitHub remote
    if let Some(repo_info) = git.repo_info().cloned() {
        // Try GitHub, fall back to git-only with warning
        if let Some(github) = GitHubBackend::new(repo_info) {
            return Ok(Box::new(CombinedBackend::new(git, github)));
        }
        eprintln!("Warning: GitHub features unavailable (no client could be created)");
    }

    // Local git only - no GitHub access
    Ok(Box::new(git))
}
