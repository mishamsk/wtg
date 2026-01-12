use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, GitRepo};
use crate::github::GhRepoInfo;
use git2::{FetchOptions, RemoteCallbacks, Repository};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Tracks what data has been synchronized from remote.
///
/// This helps avoid redundant network calls:
/// - If `full_metadata_synced`, we've done a filter clone or full fetch, so all refs are known
/// - If a commit is in `fetched_commits`, we've already fetched it individually
/// - If `tags_synced`, we've fetched all tags
#[derive(Default)]
pub struct FetchState {
    /// True if we did a full metadata fetch (filter clone or fetch --all)
    pub full_metadata_synced: bool,
    /// Specific commits we've fetched individually
    pub fetched_commits: HashSet<String>,
    /// True if we've fetched all tags
    pub tags_synced: bool,
}

/// Manages repository access for both local and remote repositories.
///
/// This is the self-contained API for all git operations, handling:
/// - Local repos vs cached remote repos
/// - Lazy clone/fetch on demand
/// - State tracking to avoid redundant network calls
/// - Shallow repo detection
pub struct RepoManager {
    /// The actual git repository
    git_repo: GitRepo,
    /// Repository info (owner/repo) if known
    repo_info: Option<GhRepoInfo>,
    /// Remote URL for fetching
    remote_url: Option<String>,
    /// True if this is user's working directory (not a cached clone)
    is_local: bool,
    /// True if the repo is shallow
    is_shallow: bool,
    /// Tracks what's been synced from remote
    fetch_state: Mutex<FetchState>,
    /// Whether fetching is allowed (opt-in for local repos)
    allow_fetch: bool,
}

impl RepoManager {
    /// Create a repo manager for the current local repository.
    ///
    /// By default, fetching is disabled for local repos to avoid
    /// modifying the user's working directory unexpectedly.
    pub fn local() -> WtgResult<Self> {
        let git_repo = GitRepo::open()?;
        let remote_url = git_repo.remote_url();
        let is_shallow = git_repo.is_shallow();
        let repo_info = git_repo.github_remote();

        Ok(Self {
            git_repo,
            repo_info,
            remote_url,
            is_local: true,
            is_shallow,
            fetch_state: Mutex::new(FetchState::default()),
            allow_fetch: false, // Default: don't fetch into local repos
        })
    }

    /// Create a repo manager for a remote GitHub repository.
    ///
    /// This will clone the repo to a cache directory if not already present,
    /// or fetch updates if the cache exists to ensure metadata is fresh.
    pub fn remote(repo_info: GhRepoInfo) -> WtgResult<Self> {
        let cache_dir = get_cache_dir()?;
        let repo_cache_path = cache_dir.join(format!("{}/{}", repo_info.owner(), repo_info.repo()));

        // Check if already cloned
        let full_metadata_synced =
            if repo_cache_path.exists() && Repository::open(&repo_cache_path).is_ok() {
                // Cache exists - try to fetch to ensure metadata is fresh
                match update_remote_repo(&repo_cache_path) {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!("⚠️  Failed to update cached repo: {e}");
                        false // Continue with stale cache
                    }
                }
            } else {
                // Clone it (with filter=blob:none for efficiency)
                clone_remote_repo(repo_info.owner(), repo_info.repo(), &repo_cache_path)?;
                true // Fresh clone has all metadata
            };

        let git_repo = GitRepo::from_path(&repo_cache_path)?;
        let remote_url = Some(format!(
            "https://github.com/{}/{}.git",
            repo_info.owner(),
            repo_info.repo()
        ));
        let is_shallow = git_repo.is_shallow();

        Ok(Self {
            git_repo,
            repo_info: Some(repo_info),
            remote_url,
            is_local: false,
            is_shallow,
            fetch_state: Mutex::new(FetchState {
                full_metadata_synced,
                ..Default::default()
            }),
            allow_fetch: true, // Remote/cached repos can fetch
        })
    }

    /// Set whether fetching is allowed.
    ///
    /// Use this to enable `--fetch` flag for local repos.
    pub const fn set_allow_fetch(&mut self, allow: bool) {
        self.allow_fetch = allow;
    }

    // ========================================
    // Accessors
    // ========================================

    /// Get a reference to the underlying `GitRepo`.
    #[must_use]
    pub const fn git_repo(&self) -> &GitRepo {
        &self.git_repo
    }

    /// Get the repository path.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.git_repo.path()
    }

    /// Check if this is a local (user's working directory) repository.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        self.is_local
    }

    /// Check if this is a remote (cached) repository.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        !self.is_local
    }

    /// Check if the repository is shallow.
    #[must_use]
    pub const fn is_shallow(&self) -> bool {
        self.is_shallow
    }

    /// Get the owner/repo info if known.
    #[must_use]
    pub const fn repo_info(&self) -> Option<&GhRepoInfo> {
        self.repo_info.as_ref()
    }

    // ========================================
    // Smart commit operations
    // ========================================

    /// Find a commit by hash (local lookup only, no fetching).
    #[must_use]
    pub fn find_commit(&self, hash: &str) -> Option<CommitInfo> {
        self.git_repo.find_commit(hash)
    }

    /// Find a commit, fetching if needed based on state.
    ///
    /// This is the smart API that:
    /// 1. Tries local lookup first
    /// 2. If `full_metadata_synced`, commit doesn't exist - return None
    /// 3. If local repo and fetch not allowed, return None
    /// 4. Check ls-remote before fetching (avoid wasted downloads)
    /// 5. Fetch and retry
    pub fn find_commit_or_fetch(&self, hash: &str) -> WtgResult<Option<CommitInfo>> {
        // 1. Try local first
        if let Some(commit) = self.find_commit(hash) {
            return Ok(Some(commit));
        }

        // 2. If we've already synced all metadata, commit doesn't exist
        {
            let state = self.fetch_state.lock().expect("fetch state mutex poisoned");
            if state.full_metadata_synced {
                return Ok(None);
            }
            // Check if we've already tried to fetch this commit
            if state.fetched_commits.contains(hash) {
                return Ok(None);
            }
        }

        // 3. If local repo and fetch not allowed, return None
        if self.is_local && !self.allow_fetch {
            return Ok(None);
        }

        // 4. For shallow repos, warn and prefer API fallback to avoid huge downloads
        //    (unless explicitly allowed via --fetch)
        if self.is_shallow && !self.allow_fetch {
            eprintln!(
                "⚠️  Shallow repository detected: using API for commit lookup (use --fetch to override)"
            );
            return Ok(None);
        }

        // 5. Need remote URL to fetch
        let Some(remote_url) = &self.remote_url else {
            return Ok(None);
        };

        // 6. Check ls-remote before fetching (avoid downloading if ref doesn't exist)
        if !ls_remote_ref_exists(remote_url, hash)? {
            // Mark as fetched (attempted) so we don't retry
            self.fetch_state
                .lock()
                .expect("fetch state mutex poisoned")
                .fetched_commits
                .insert(hash.to_string());
            return Ok(None);
        }

        // 7. Fetch the specific commit
        self.fetch_commit(hash)?;

        // 8. Retry local lookup
        Ok(self.find_commit(hash))
    }

    /// Fetch a specific commit by hash.
    fn fetch_commit(&self, hash: &str) -> WtgResult<()> {
        let Some(remote_url) = &self.remote_url else {
            return Err(WtgError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No remote URL available for fetching",
            )));
        };

        fetch_commit(self.git_repo.path(), remote_url, hash)?;

        // Mark as fetched
        self.fetch_state
            .lock()
            .expect("fetch state mutex poisoned")
            .fetched_commits
            .insert(hash.to_string());

        Ok(())
    }

    /// Get tags containing a commit, fetching tags first if needed.
    pub fn tags_containing_commit(&self, commit_hash: &str) -> Vec<crate::git::TagInfo> {
        let _ = self.ensure_tags();
        self.git_repo.tags_containing_commit(commit_hash)
    }

    /// Ensure all tags are available (fetches if needed).
    fn ensure_tags(&self) -> WtgResult<()> {
        {
            let state = self.fetch_state.lock().expect("fetch state mutex poisoned");
            if state.tags_synced || state.full_metadata_synced {
                return Ok(());
            }
        }

        if self.is_local && !self.allow_fetch {
            return Ok(()); // Don't fetch for local repos unless explicitly allowed
        }

        let Some(remote_url) = &self.remote_url else {
            return Ok(()); // No remote to fetch from
        };

        fetch_tags(self.git_repo.path(), remote_url)?;

        self.fetch_state
            .lock()
            .expect("fetch state mutex poisoned")
            .tags_synced = true;

        Ok(())
    }
}

/// Get the cache directory for remote repositories
fn get_cache_dir() -> WtgResult<PathBuf> {
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| {
            WtgError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine cache directory",
            ))
        })?
        .join("wtg")
        .join("repos");

    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir)?;
    }

    Ok(cache_dir)
}

/// Clone a remote repository using subprocess with filter=blob:none, falling back to git2 if needed
fn clone_remote_repo(owner: &str, repo: &str, target_path: &Path) -> WtgResult<()> {
    // Create parent directory
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let repo_url = format!("https://github.com/{owner}/{repo}.git");

    eprintln!("🔄 Cloning remote repository {repo_url}...");

    // Try subprocess with --filter=blob:none first (requires Git 2.17+)
    match clone_with_filter(&repo_url, target_path) {
        Ok(()) => {
            eprintln!("✅ Repository cloned successfully (using filter)");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠️  Filter clone failed ({e}), falling back to bare clone...");
            // Fall back to git2 bare clone
            clone_bare_with_git2(&repo_url, target_path)
        }
    }
}

/// Clone with --filter=blob:none using subprocess
fn clone_with_filter(repo_url: &str, target_path: &Path) -> WtgResult<()> {
    let output = Command::new("git")
        .args([
            "clone",
            "--filter=blob:none", // Don't download blobs until needed (Git 2.17+)
            "--bare",             // Bare repository (no working directory)
            repo_url,
            target_path.to_str().ok_or_else(|| {
                WtgError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid path",
                ))
            })?,
        ])
        .output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(WtgError::Io(std::io::Error::other(format!(
            "Failed to clone with filter: {error}"
        ))));
    }

    Ok(())
}

/// Clone bare repository using git2 (fallback)
fn clone_bare_with_git2(repo_url: &str, target_path: &Path) -> WtgResult<()> {
    // Clone without progress output for cleaner UX
    let callbacks = RemoteCallbacks::new();

    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(callbacks);

    // Build the repository with options
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_options);
    builder.bare(true); // Bare repository - no working directory, only git metadata

    // Clone the repository as bare
    // This gets all commits, branches, and tags without checking out files
    builder.clone(repo_url, target_path)?;

    eprintln!("✅ Repository cloned successfully (using bare clone)");

    Ok(())
}

/// Update an existing cloned remote repository
fn update_remote_repo(repo_path: &PathBuf) -> WtgResult<()> {
    eprintln!("🔄 Updating cached repository...");

    // Try subprocess fetch first (works for both filter and non-filter repos)
    match fetch_with_subprocess(repo_path) {
        Ok(()) => {
            eprintln!("✅ Repository updated");
            Ok(())
        }
        Err(_) => {
            // Fall back to git2
            fetch_with_git2(repo_path)
        }
    }
}

/// Fetch updates using subprocess
fn fetch_with_subprocess(repo_path: &Path) -> WtgResult<()> {
    let args = build_fetch_args(repo_path)?;

    let output = Command::new("git").args(&args).output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(WtgError::Io(std::io::Error::other(format!(
            "Failed to fetch: {error}"
        ))));
    }

    Ok(())
}

/// Build the arguments passed to `git fetch` when refreshing cached repos.
///
/// Keeping this logic isolated lets us sanity-check the flags in unit tests so
/// we don't regress on rejected tag updates again.
fn build_fetch_args(repo_path: &Path) -> WtgResult<Vec<String>> {
    let repo_path = repo_path.to_str().ok_or_else(|| {
        WtgError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid path",
        ))
    })?;

    Ok(vec![
        "-C".to_string(),
        repo_path.to_string(),
        "fetch".to_string(),
        "--all".to_string(),
        "--tags".to_string(),
        "--force".to_string(),
        "--prune".to_string(),
    ])
}

/// Fetch updates using git2 (fallback)
fn fetch_with_git2(repo_path: &PathBuf) -> WtgResult<()> {
    let repo = Repository::open(repo_path)?;

    // Find the origin remote
    let mut remote = repo
        .find_remote("origin")
        .or_else(|_| repo.find_remote("upstream"))
        .map_err(WtgError::Git)?;

    // Fetch without progress output for cleaner UX
    let callbacks = RemoteCallbacks::new();
    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(callbacks);

    // Fetch all refs
    remote.fetch(
        &["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"],
        Some(&mut fetch_options),
        None,
    )?;

    eprintln!("✅ Repository updated");

    Ok(())
}

// ========================================
// On-demand fetch utilities
// ========================================

/// Check if a ref exists on remote without fetching (git ls-remote).
///
/// This is a lightweight check that doesn't download any data.
/// Returns true if the ref exists, false otherwise.
fn ls_remote_ref_exists(remote_url: &str, ref_spec: &str) -> WtgResult<bool> {
    let output = Command::new("git")
        .args(["ls-remote", "--exit-code", remote_url, ref_spec])
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status();

    match output {
        Ok(status) => Ok(status.success()),
        Err(e) => Err(WtgError::Io(e)),
    }
}

/// Fetch a specific commit by hash.
///
/// This fetches only the specified commit, not all refs.
fn fetch_commit(repo_path: &Path, remote_url: &str, hash: &str) -> WtgResult<()> {
    let repo_path_str = repo_path.to_str().ok_or_else(|| {
        WtgError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid path",
        ))
    })?;

    let output = Command::new("git")
        .args(["-C", repo_path_str, "fetch", "--depth=1", remote_url, hash])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(WtgError::Io(std::io::Error::other(format!(
            "Failed to fetch commit {hash}: {stderr}"
        ))))
    }
}

/// Fetch all tags from remote.
fn fetch_tags(repo_path: &Path, remote_url: &str) -> WtgResult<()> {
    let repo_path_str = repo_path.to_str().ok_or_else(|| {
        WtgError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid path",
        ))
    })?;

    let output = Command::new("git")
        .args([
            "-C",
            repo_path_str,
            "fetch",
            "--tags",
            "--force",
            remote_url,
        ])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(WtgError::Io(std::io::Error::other(format!(
            "Failed to fetch tags: {stderr}"
        ))))
    }
}
