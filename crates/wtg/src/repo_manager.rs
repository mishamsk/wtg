use crate::error::{Result, WtgError};
use crate::git::GitRepo;
use git2::{FetchOptions, RemoteCallbacks, Repository};
use std::path::PathBuf;

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
        let repo_cache_path = cache_dir.join(format!("{}/{}", owner, repo));

        // Check if already cloned
        if repo_cache_path.exists() && Repository::open(&repo_cache_path).is_ok() {
            // Try to update it
            if let Err(e) = update_remote_repo(&repo_cache_path) {
                eprintln!("Warning: Failed to update cached repo: {}", e);
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

    /// Get the GitRepo instance for this managed repository
    pub fn git_repo(&self) -> Result<GitRepo> {
        GitRepo::from_path(&self.local_path)
    }

    /// Get the repository path
    pub fn path(&self) -> &PathBuf {
        &self.local_path
    }

    /// Check if this is a remote repository
    pub const fn is_remote(&self) -> bool {
        self.is_remote
    }

    /// Get the owner/repo info (only for remote repos)
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

/// Clone a remote repository using git2
fn clone_remote_repo(owner: &str, repo: &str, target_path: &PathBuf) -> Result<()> {
    // Create parent directory
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let repo_url = format!("https://github.com/{}/{}.git", owner, repo);

    eprintln!("🔄 Cloning remote repository {}...", repo_url);

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
    builder.clone(&repo_url, target_path)?;

    eprintln!("✅ Repository cloned successfully");

    Ok(())
}

/// Update an existing cloned remote repository
fn update_remote_repo(repo_path: &PathBuf) -> Result<()> {
    eprintln!("🔄 Updating cached repository...");

    let repo = Repository::open(repo_path)?;

    // Find the origin remote
    let mut remote = repo
        .find_remote("origin")
        .or_else(|_| repo.find_remote("upstream"))
        .map_err(|e| WtgError::Git(e))?;

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
