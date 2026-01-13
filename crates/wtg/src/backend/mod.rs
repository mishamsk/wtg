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
use crate::parse_input::ParsedInput;
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

/// Notice about backend operating in reduced-functionality mode.
/// These are soft warnings - we continue with degraded capabilities.
#[derive(Debug)]
pub enum BackendNotice {
    /// No remotes configured at all
    NoRemotes,
    /// Single host type detected but it's not GitHub
    /// (all remotes point to GitLab, Bitbucket, or unknown host)
    UnsupportedHost {
        /// The best remote we found (by priority: origin > upstream > other)
        best_remote: RemoteInfo,
    },
    /// Multiple different hosts detected, none of them GitHub
    /// (e.g., origin=GitLab, upstream=Bitbucket)
    MixedRemotes {
        /// All the unique hosts we found
        hosts: Vec<RemoteHost>,
        /// Total remote count
        count: usize,
    },
    /// GitHub remote found but API client couldn't be created
    /// (missing token, network issue, etc.)
    UnreachableGitHub {
        /// The GitHub remote we found
        remote: RemoteInfo,
    },
    /// Local git repo couldn't be opened, using pure API
    /// (only for remote repo queries)
    ApiOnly,
}

/// Result of backend resolution.
pub struct ResolvedBackend {
    pub backend: Box<dyn Backend>,
    /// Optional notice about reduced functionality.
    pub notice: Option<BackendNotice>,
}

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
) -> WtgResult<ResolvedBackend> {
    // Case 1: Explicit repo info provided (from URL/flags)
    if let Some(repo_info) = parsed_input.gh_repo_info() {
        // User explicitly provided GitHub info - GitHub client failure is a hard error
        let github = GitHubBackend::new(repo_info.clone()).ok_or(WtgError::GitHubClientFailed)?;

        // Try to get local git repo for combined backend
        match GitRepo::remote(repo_info.clone()) {
            Ok(git_repo) => {
                let git = GitBackend::new(git_repo);
                Ok(ResolvedBackend {
                    backend: Box::new(CombinedBackend::new(git, github)),
                    notice: None,
                })
            }
            Err(_) => {
                // Can't access git locally, use pure API (soft notice)
                Ok(ResolvedBackend {
                    backend: Box::new(github),
                    notice: Some(BackendNotice::ApiOnly),
                })
            }
        }
    } else {
        // Case 2: Local repo detection
        resolve_local_backend(allow_user_repo_fetch)
    }
}

fn resolve_local_backend(allow_user_repo_fetch: bool) -> WtgResult<ResolvedBackend> {
    let mut git_repo = GitRepo::open()?;
    if allow_user_repo_fetch {
        git_repo.set_allow_fetch(true);
    }

    // Collect and sort remotes by priority (origin > upstream > other, GitHub first)
    let mut remotes: Vec<RemoteInfo> = git_repo.remotes().collect();
    remotes.sort_by_key(RemoteInfo::priority);

    // No remotes at all
    if remotes.is_empty() {
        let git = GitBackend::new(git_repo);
        return Ok(ResolvedBackend {
            backend: Box::new(git),
            notice: Some(BackendNotice::NoRemotes),
        });
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
                return Ok(ResolvedBackend {
                    backend: Box::new(CombinedBackend::new(git, github)),
                    notice: None,
                });
            }

            // GitHub remote found but client creation failed
            return Ok(ResolvedBackend {
                backend: Box::new(git),
                notice: Some(BackendNotice::UnreachableGitHub {
                    remote: github_remote,
                }),
            });
        }
    }

    // No GitHub remote - analyze what we have
    let git = GitBackend::new(git_repo);
    let unique_hosts: HashSet<Option<RemoteHost>> = remotes.iter().map(|r| r.host).collect();

    // Check if we have mixed hosts (excluding None/unknown)
    let known_hosts: Vec<RemoteHost> = unique_hosts.iter().filter_map(|h| *h).collect();

    if known_hosts.len() > 1 {
        // Multiple different known hosts - we're lost!
        return Ok(ResolvedBackend {
            backend: Box::new(git),
            notice: Some(BackendNotice::MixedRemotes {
                hosts: known_hosts,
                count: remotes.len(),
            }),
        });
    }

    // Single host type (or all unknown) - return the best one
    let best_remote = remotes.into_iter().next().unwrap();
    Ok(ResolvedBackend {
        backend: Box::new(git),
        notice: Some(BackendNotice::UnsupportedHost { best_remote }),
    })
}
