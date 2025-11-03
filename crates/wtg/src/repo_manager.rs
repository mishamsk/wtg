use crate::error::{Result, WtgError};
use crate::git::GitRepo;
use git2::{FetchOptions, RemoteCallbacks, Repository};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Manages repository access for both local and remote repositories
pub struct RepoManager {
    local_path: PathBuf,
    is_remote: bool,
    owner: Option<String>,
    repo_name: Option<String>,
}

impl RepoManager {
    /// Create a repo manager for the current local repository
    pub fn local() -> Result<Self> {
        let repo = Repository::discover(".").map_err(|_| WtgError::NotInGitRepo)?;
        let path = repo.workdir().ok_or(WtgError::NotInGitRepo)?.to_path_buf();

        Ok(Self {
            local_path: path,
            is_remote: false,
            owner: None,
            repo_name: None,
        })
    }

    /// Create a repo manager for a remote GitHub repository
    /// This will clone the repo to a cache directory if needed
    pub fn remote(owner: String, repo: String) -> Result<Self> {
        let cache_dir = get_cache_dir()?;
        let repo_cache_path = cache_dir.join(format!("{owner}/{repo}"));

        // Check if already cloned
        if repo_cache_path.exists() && Repository::open(&repo_cache_path).is_ok() {
            // Try to update it
            if let Err(e) = update_remote_repo(&repo_cache_path) {
                eprintln!("Warning: Failed to update cached repo: {e}");
                // Continue anyway - use the cached version
            }
        } else {
            // Clone it
            clone_remote_repo(&owner, &repo, &repo_cache_path)?;
        }

        Ok(Self {
            local_path: repo_cache_path,
            is_remote: true,
            owner: Some(owner),
            repo_name: Some(repo),
        })
    }

    /// Get the `GitRepo` instance for this managed repository
    pub fn git_repo(&self) -> Result<GitRepo> {
        GitRepo::from_path(&self.local_path)
    }

    /// Get the repository path
    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        &self.local_path
    }

    /// Check if this is a remote repository
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        self.is_remote
    }

    /// Get the owner/repo info (only for remote repos)
    #[must_use]
    pub fn remote_info(&self) -> Option<(String, String)> {
        if self.is_remote {
            Some((self.owner.clone()?, self.repo_name.clone()?))
        } else {
            None
        }
    }
}

/// Get the cache directory for remote repositories
fn get_cache_dir() -> Result<PathBuf> {
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
fn clone_remote_repo(owner: &str, repo: &str, target_path: &Path) -> Result<()> {
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
fn clone_with_filter(repo_url: &str, target_path: &Path) -> Result<()> {
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
fn clone_bare_with_git2(repo_url: &str, target_path: &Path) -> Result<()> {
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
fn update_remote_repo(repo_path: &PathBuf) -> Result<()> {
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
fn fetch_with_subprocess(repo_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().ok_or_else(|| {
                WtgError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid path",
                ))
            })?,
            "fetch",
            "--all",
            "--tags",
        ])
        .output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(WtgError::Io(std::io::Error::other(format!(
            "Failed to fetch: {error}"
        ))));
    }

    Ok(())
}

/// Fetch updates using git2 (fallback)
fn fetch_with_git2(repo_path: &PathBuf) -> Result<()> {
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
