//! Notices emitted during backend/git operations.
//!
//! All notices are delivered via callback - the CLI layer decides how to display them.

use std::sync::Arc;

use crate::remote::{RemoteHost, RemoteInfo};

/// Notices emitted during backend/git operations.
/// All notices are delivered via callback - the CLI layer decides how to display them.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Notice {
    // --- Backend capability notices ---
    /// No remotes configured at all
    NoRemotes,
    /// Single host type detected but it's not GitHub
    UnsupportedHost {
        /// The best remote we found (by priority: upstream > origin > other)
        best_remote: RemoteInfo,
    },
    /// Multiple different hosts detected, none of them GitHub
    MixedRemotes {
        /// All the unique hosts we found
        hosts: Vec<RemoteHost>,
        /// Total remote count
        count: usize,
    },
    /// GitHub remote found but API client couldn't be created
    UnreachableGitHub {
        /// The GitHub remote we found
        remote: RemoteInfo,
    },
    /// Local git repo couldn't be opened, using pure API
    ApiOnly,

    // --- Operational notices ---
    /// Failed to update a cached repository
    CacheUpdateFailed { error: String },
    /// Repository is shallow, falling back to API
    ShallowRepoDetected,
    /// Starting to clone a remote repository
    CloningRepo { url: String },
    /// Clone succeeded
    CloneSucceeded { used_filter: bool },
    /// Filter clone failed, falling back to bare clone
    CloneFallbackToBare { error: String },
    /// Starting to update a cached repository
    UpdatingCache,
    /// Cache update completed
    CacheUpdated,
    /// Cross-project reference falling back to API-only
    CrossProjectFallbackToApi {
        owner: String,
        repo: String,
        error: String,
    },
    /// GitHub API rate limit was hit
    GhRateLimitHit {
        /// Whether the client was authenticated or anonymous
        authenticated: bool,
    },
}

/// Callback for emitting notices during operations.
pub type NoticeCallback = Arc<dyn Fn(Notice) + Send + Sync>;

/// Create a no-op callback for when notices should be ignored.
#[must_use]
pub fn no_notices() -> NoticeCallback {
    Arc::new(|_| {})
}
