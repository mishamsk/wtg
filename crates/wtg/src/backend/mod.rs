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

pub use combined_backend::CombinedBackend;
pub use git_backend::GitBackend;
pub use github_backend::GitHubBackend;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, TagInfo};
use crate::github::{ExtendedIssueInfo, GhRepoInfo, PullRequestInfo};
use crate::identifier::{EnrichedInfo, EntryPoint, FileResult, IdentifiedThing};
use crate::parse_input::{ParsedInput, Query};
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
// Resolution functions (operate on backends)
// ============================================

/// Resolve a query to identified information using the provided backend.
pub(crate) async fn resolve(backend: &dyn Backend, query: &Query) -> WtgResult<IdentifiedThing> {
    match query {
        Query::GitCommit(hash) => resolve_commit(backend, hash).await,
        Query::Pr(number) => resolve_pr(backend, *number).await,
        Query::Issue(number) => resolve_issue(backend, *number).await,
        Query::IssueOrPr(number) => {
            // Try PR first, then issue
            if let Ok(result) = resolve_pr(backend, *number).await {
                return Ok(result);
            }
            if let Ok(result) = resolve_issue(backend, *number).await {
                return Ok(result);
            }
            Err(WtgError::NotFound(format!("#{number}")))
        }
        Query::FilePath(path) => resolve_file(backend, &path.to_string_lossy()).await,
        Query::Unknown(input) => resolve_unknown(backend, input).await,
    }
}

/// Resolve a commit hash to `IdentifiedThing`.
async fn resolve_commit(backend: &dyn Backend, hash: &str) -> WtgResult<IdentifiedThing> {
    let commit = backend.find_commit(hash).await?;
    let commit = backend.enrich_commit(commit).await;
    let release = backend
        .find_release_for_commit(&commit.hash, Some(commit.date))
        .await;

    Ok(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point: EntryPoint::Commit(hash.to_string()),
        commit: Some(commit),
        pr: None,
        issue: None,
        release,
    })))
}

/// Resolve a PR number to `IdentifiedThing`.
async fn resolve_pr(backend: &dyn Backend, number: u64) -> WtgResult<IdentifiedThing> {
    let pr = backend.fetch_pr(number).await?;

    let commit = backend.find_commit_for_pr(&pr).await.ok();
    let commit = match commit {
        Some(c) => Some(backend.enrich_commit(c).await),
        None => None,
    };

    let release = if let Some(ref c) = commit {
        backend.find_release_for_commit(&c.hash, Some(c.date)).await
    } else {
        None
    };

    Ok(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point: EntryPoint::PullRequestNumber(number),
        commit,
        pr: Some(pr),
        issue: None,
        release,
    })))
}

/// Resolve an issue number to `IdentifiedThing`.
///
/// Handles cross-project PRs by spawning a backend for the PR's repository.
async fn resolve_issue(backend: &dyn Backend, number: u64) -> WtgResult<IdentifiedThing> {
    let ext_issue = backend.fetch_issue(number).await?;
    let display_issue = (&ext_issue).into();

    // Try to find closing PR info
    let closing_pr = ext_issue.closing_prs.into_iter().next();

    let (commit, release) = if let Some(ref pr) = closing_pr {
        if let Some(merge_sha) = &pr.merge_commit_sha {
            // Check if PR is from a different repo (cross-project)
            let is_cross_repo = pr.repo_info.as_ref().is_some_and(|pr_repo| {
                backend
                    .repo_info()
                    .is_some_and(|ri| pr_repo.owner() != ri.owner() || pr_repo.repo() != ri.repo())
            });

            if is_cross_repo {
                // Fetch from PR's repo using cross-project backend
                if let Some(pr_repo) = &pr.repo_info
                    && let Some(cross_backend) = backend.for_repo(pr_repo).await
                {
                    let commit = cross_backend.find_commit(merge_sha).await.ok();
                    let commit = match commit {
                        Some(c) => Some(cross_backend.enrich_commit(c).await),
                        None => None,
                    };
                    let release = if let Some(ref c) = commit {
                        cross_backend
                            .find_release_for_commit(&c.hash, Some(c.date))
                            .await
                    } else {
                        None
                    };
                    (commit, release)
                } else {
                    (None, None)
                }
            } else {
                // Same repo - use provided backend
                let commit = backend.find_commit(merge_sha).await.ok();
                let commit = match commit {
                    Some(c) => Some(backend.enrich_commit(c).await),
                    None => None,
                };
                let release = if let Some(ref c) = commit {
                    backend.find_release_for_commit(&c.hash, Some(c.date)).await
                } else {
                    None
                };
                (commit, release)
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    Ok(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point: EntryPoint::IssueNumber(number),
        commit,
        pr: closing_pr,
        issue: Some(display_issue),
        release,
    })))
}

/// Resolve a file path to `IdentifiedThing`.
async fn resolve_file(backend: &dyn Backend, path: &str) -> WtgResult<IdentifiedThing> {
    let file_info = backend.find_file(path).await?;
    let commit_url = backend.commit_url(&file_info.last_commit.hash);

    // Generate author URLs from emails
    let author_urls: Vec<Option<String>> = file_info
        .previous_authors
        .iter()
        .map(|(_, _, email)| backend.author_url_from_email(email))
        .collect();

    let release = backend
        .find_release_for_commit(
            &file_info.last_commit.hash,
            Some(file_info.last_commit.date),
        )
        .await;

    Ok(IdentifiedThing::File(Box::new(FileResult {
        file_info,
        commit_url,
        author_urls,
        release,
    })))
}

/// Resolve a tag name to `IdentifiedThing`.
async fn resolve_tag(backend: &dyn Backend, name: &str) -> WtgResult<IdentifiedThing> {
    let tag = backend.find_tag(name).await?;
    let url = backend.tag_url(name);
    Ok(IdentifiedThing::TagOnly(Box::new(tag), url))
}

/// Resolve unknown input by trying each possibility.
async fn resolve_unknown(backend: &dyn Backend, input: &str) -> WtgResult<IdentifiedThing> {
    // Try as commit hash
    if let Ok(result) = resolve_commit(backend, input).await {
        return Ok(result);
    }

    // Try as PR/issue number (if numeric)
    if let Ok(number) = input.parse::<u64>() {
        if let Ok(result) = resolve_pr(backend, number).await {
            return Ok(result);
        }
        if let Ok(result) = resolve_issue(backend, number).await {
            return Ok(result);
        }
    }

    // Try as file path
    if let Ok(result) = resolve_file(backend, input).await {
        return Ok(result);
    }

    // Try as tag
    if let Ok(result) = resolve_tag(backend, input).await {
        return Ok(result);
    }

    Err(WtgError::NotFound(input.to_string()))
}

// ============================================
// Backend resolution
// ============================================

/// Resolve the best backend based on available resources.
///
/// Decision tree:
/// 1. Explicit repo info provided → Use cached/cloned repo + GitHub API
/// 2. In local repo with GitHub remote → Combined backend
/// 3. In local repo without remote → Git-only backend
/// 4. Not in repo and no info → Error
pub(crate) fn resolve_backend(parsed_input: &ParsedInput) -> WtgResult<Box<dyn Backend>> {
    if let Some(repo_info) = parsed_input.gh_repo_info() {
        // Explicit repo info - use RepoManager for cache handling
        let repo_manager = RepoManager::remote(repo_info.clone())?;
        let git = GitBackend::from_path(repo_manager.path())?;
        let github = GitHubBackend::new(repo_info.clone());

        // Combined backend with both local and API access
        return Ok(Box::new(CombinedBackend::new(git, github)));
    }

    // No explicit repo info - must be in local repo
    let git = GitBackend::open()?;

    // Try to find GitHub remote
    if let Some(repo_info) = git.repo_info().cloned() {
        let github = GitHubBackend::new(repo_info);
        return Ok(Box::new(CombinedBackend::new(git, github)));
    }

    // Local git only - no GitHub access
    Ok(Box::new(git))
}
