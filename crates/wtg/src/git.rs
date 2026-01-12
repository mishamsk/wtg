use crate::error::{WtgError, WtgResult};
use crate::github::{GhRepoInfo, ReleaseInfo};
use crate::parse_input::parse_github_repo_url;
use chrono::{DateTime, TimeZone, Utc};
use git2::{Commit, FetchOptions, Oid, RemoteCallbacks, Repository};
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};

/// Tracks what data has been synchronized from remote.
///
/// This helps avoid redundant network calls:
/// - If `full_metadata_synced`, we've done a filter clone or full fetch, so all refs are known
/// - If a commit is in `fetched_commits`, we've already fetched it individually
/// - If `tags_synced`, we've fetched all tags
#[derive(Default)]
struct FetchState {
    /// True if we did a full metadata fetch (filter clone or fetch --all)
    full_metadata_synced: bool,
    /// Specific commits we've fetched individually
    fetched_commits: HashSet<String>,
    /// True if we've fetched all tags
    tags_synced: bool,
}

pub struct GitRepo {
    repo: Arc<Mutex<Repository>>,
    path: PathBuf,
    /// Remote URL for fetching
    remote_url: Option<String>,
    /// Repository info (owner/repo) if explicitly set
    repo_info: Option<GhRepoInfo>,
    /// Whether fetching is allowed
    allow_fetch: bool,
    /// Tracks what's been synced from remote
    fetch_state: Mutex<FetchState>,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub message_lines: usize,
    pub commit_url: Option<String>,
    pub author_name: String,
    pub author_email: Option<String>,
    pub author_login: Option<String>,
    pub author_url: Option<String>,
    pub date: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub last_commit: CommitInfo,
    pub previous_authors: Vec<(String, String, String)>, // (hash, name, email)
}

#[derive(Debug, Clone)]
pub struct TagInfo {
    pub name: String,
    pub commit_hash: String,
    pub semver_info: Option<SemverInfo>,
    pub created_at: DateTime<Utc>, // Timestamp of the commit the tag points to
    pub is_release: bool,          // Whether this is a GitHub release
    pub release_name: Option<String>, // GitHub release name (if is_release)
    pub release_url: Option<String>, // GitHub release URL (if is_release)
    pub published_at: Option<DateTime<Utc>>, // GitHub release published date (if is_release)
}

impl TagInfo {
    /// Whether this is a semver tag
    #[must_use]
    pub const fn is_semver(&self) -> bool {
        self.semver_info.is_some()
    }

    /// Whether this tag represents a stable release (no pre-release, no build metadata)
    #[must_use]
    pub const fn is_stable_semver(&self) -> bool {
        if let Some(semver) = &self.semver_info {
            semver.pre_release.is_none()
                && semver.build_metadata.is_none()
                && semver.build.is_none()
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemverInfo {
    pub major: u32,
    pub minor: u32,
    pub patch: Option<u32>,
    pub build: Option<u32>,
    pub pre_release: Option<String>,
    pub build_metadata: Option<String>,
}

impl GitRepo {
    /// Open the git repository from the current directory.
    /// Fetch is disabled by default for local repos.
    pub fn open() -> WtgResult<Self> {
        let repo = Repository::discover(".").map_err(|_| WtgError::NotInGitRepo)?;
        let path = repo.path().to_path_buf();
        let remote_url = Self::extract_remote_url(&repo);
        Ok(Self {
            repo: Arc::new(Mutex::new(repo)),
            path,
            remote_url,
            repo_info: None,
            allow_fetch: false,
            fetch_state: Mutex::new(FetchState::default()),
        })
    }

    /// Open the git repository from a specific path.
    /// Fetch is disabled by default.
    pub fn from_path(path: &Path) -> WtgResult<Self> {
        let repo = Repository::open(path).map_err(|_| WtgError::NotInGitRepo)?;
        let repo_path = repo.path().to_path_buf();
        let remote_url = Self::extract_remote_url(&repo);
        Ok(Self {
            repo: Arc::new(Mutex::new(repo)),
            path: repo_path,
            remote_url,
            repo_info: None,
            allow_fetch: false,
            fetch_state: Mutex::new(FetchState::default()),
        })
    }

    /// Open or clone a remote GitHub repository.
    /// Uses a cache directory (~/.cache/wtg/repos). Fetch is enabled by default.
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

        let repo = Repository::open(&repo_cache_path).map_err(|_| WtgError::NotInGitRepo)?;
        let path = repo.path().to_path_buf();
        let remote_url = Some(format!(
            "https://github.com/{}/{}.git",
            repo_info.owner(),
            repo_info.repo()
        ));

        Ok(Self {
            repo: Arc::new(Mutex::new(repo)),
            path,
            remote_url,
            repo_info: Some(repo_info),
            allow_fetch: true,
            fetch_state: Mutex::new(FetchState {
                full_metadata_synced,
                ..Default::default()
            }),
        })
    }

    /// Extract remote URL from repository (origin or upstream)
    fn extract_remote_url(repo: &Repository) -> Option<String> {
        for remote_name in ["origin", "upstream"] {
            if let Ok(remote) = repo.find_remote(remote_name)
                && let Some(url) = remote.url()
            {
                return Some(url.to_string());
            }
        }
        None
    }

    /// Get the repository path
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Check if this is a shallow repository (internal use only)
    fn is_shallow(&self) -> bool {
        self.with_repo(git2::Repository::is_shallow)
    }

    /// Get the remote URL for fetching
    #[must_use]
    pub fn remote_url(&self) -> Option<&str> {
        self.remote_url.as_deref()
    }

    /// Set whether fetching is allowed.
    /// Use this to enable `--fetch` flag for local repos.
    pub const fn set_allow_fetch(&mut self, allow: bool) {
        self.allow_fetch = allow;
    }

    /// Get a reference to the stored repo info (owner/repo) if explicitly set.
    #[must_use]
    pub const fn repo_info(&self) -> Option<&GhRepoInfo> {
        self.repo_info.as_ref()
    }

    fn with_repo<T>(&self, f: impl FnOnce(&Repository) -> T) -> T {
        let repo = self.repo.lock().expect("git repository mutex poisoned");
        f(&repo)
    }

    /// Find a commit by hash (can be short or full).
    /// If `allow_fetch` is true and the commit isn't found locally, attempts to fetch it.
    pub fn find_commit(&self, hash_str: &str) -> WtgResult<Option<CommitInfo>> {
        // 1. Try local first
        if let Some(commit) = self.find_commit_local(hash_str) {
            return Ok(Some(commit));
        }

        // 2. If we've already synced all metadata, commit doesn't exist
        {
            let state = self.fetch_state.lock().expect("fetch state mutex poisoned");
            if state.full_metadata_synced {
                return Ok(None);
            }
            // Check if we've already tried to fetch this commit
            if state.fetched_commits.contains(hash_str) {
                return Ok(None);
            }
        }

        // 3. If fetch not allowed, return None
        if !self.allow_fetch {
            return Ok(None);
        }

        // 4. For shallow repos, warn and prefer API fallback to avoid huge downloads
        if self.is_shallow() {
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
        if !ls_remote_ref_exists(remote_url, hash_str)? {
            // Mark as fetched (attempted) so we don't retry
            self.fetch_state
                .lock()
                .expect("fetch state mutex poisoned")
                .fetched_commits
                .insert(hash_str.to_string());
            return Ok(None);
        }

        // 7. Fetch the specific commit
        fetch_commit(&self.path, remote_url, hash_str)?;

        // 8. Mark as fetched
        self.fetch_state
            .lock()
            .expect("fetch state mutex poisoned")
            .fetched_commits
            .insert(hash_str.to_string());

        // 9. Retry local lookup
        Ok(self.find_commit_local(hash_str))
    }

    /// Find a commit by hash locally only (no fetch).
    #[must_use]
    fn find_commit_local(&self, hash_str: &str) -> Option<CommitInfo> {
        self.with_repo(|repo| {
            if let Ok(oid) = Oid::from_str(hash_str)
                && let Ok(commit) = repo.find_commit(oid)
            {
                return Some(Self::commit_to_info(&commit));
            }

            if hash_str.len() >= 7
                && let Ok(obj) = repo.revparse_single(hash_str)
                && let Ok(commit) = obj.peel_to_commit()
            {
                return Some(Self::commit_to_info(&commit));
            }

            None
        })
    }

    /// Find a file in the repository
    #[must_use]
    pub fn find_file(&self, path: &str) -> Option<FileInfo> {
        self.with_repo(|repo| {
            let mut revwalk = repo.revwalk().ok()?;
            revwalk.push_head().ok()?;

            for oid in revwalk {
                let oid = oid.ok()?;
                let commit = repo.find_commit(oid).ok()?;

                if commit_touches_file(&commit, path) {
                    let commit_info = Self::commit_to_info(&commit);
                    let previous_authors = Self::get_previous_authors(repo, path, &commit, 4);

                    return Some(FileInfo {
                        path: path.to_string(),
                        last_commit: commit_info,
                        previous_authors,
                    });
                }
            }

            None
        })
    }

    /// Get previous authors for a file (excluding the last commit)
    fn get_previous_authors(
        repo: &Repository,
        path: &str,
        last_commit: &Commit,
        limit: usize,
    ) -> Vec<(String, String, String)> {
        let mut authors = Vec::new();
        let Ok(mut revwalk) = repo.revwalk() else {
            return authors;
        };

        if revwalk.push_head().is_err() {
            return authors;
        }

        let mut found_last = false;

        for oid in revwalk {
            if authors.len() >= limit {
                break;
            }

            let Ok(oid) = oid else { continue };

            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };

            if commit.id() == last_commit.id() {
                found_last = true;
                continue;
            }

            if !found_last {
                continue;
            }

            if commit_touches_file(&commit, path) {
                authors.push((
                    commit.id().to_string()[..7].to_string(),
                    commit.author().name().unwrap_or("Unknown").to_string(),
                    commit.author().email().unwrap_or("").to_string(),
                ));
            }
        }

        authors
    }

    /// Get all tags in the repository
    #[must_use]
    pub fn get_tags(&self) -> Vec<TagInfo> {
        self.get_tags_with_releases(&[])
    }

    /// Get all tags in the repository, enriched with GitHub release info
    #[must_use]
    pub fn get_tags_with_releases(&self, github_releases: &[ReleaseInfo]) -> Vec<TagInfo> {
        let release_map: std::collections::HashMap<String, &ReleaseInfo> = github_releases
            .iter()
            .map(|r| (r.tag_name.clone(), r))
            .collect();

        self.with_repo(|repo| {
            let mut tags = Vec::new();

            if let Ok(tag_names) = repo.tag_names(None) {
                for tag_name in tag_names.iter().flatten() {
                    if let Ok(obj) = repo.revparse_single(tag_name)
                        && let Ok(commit) = obj.peel_to_commit()
                    {
                        let semver_info = parse_semver(tag_name);

                        let (is_release, release_name, release_url, published_at) = release_map
                            .get(tag_name)
                            .map_or((false, None, None, None), |release| {
                                (
                                    true,
                                    release.name.clone(),
                                    Some(release.url.clone()),
                                    release.published_at,
                                )
                            });

                        tags.push(TagInfo {
                            name: tag_name.to_string(),
                            commit_hash: commit.id().to_string(),
                            semver_info,
                            created_at: git_time_to_datetime(commit.time()),
                            is_release,
                            release_name,
                            release_url,
                            published_at,
                        });
                    }
                }
            }

            tags
        })
    }

    /// Expose tags that contain the specified commit.
    /// If `allow_fetch` is true, ensures tags are fetched first.
    pub fn tags_containing_commit(&self, commit_hash: &str) -> Vec<TagInfo> {
        // Ensure tags are available (fetches if needed)
        let _ = self.ensure_tags();

        let Ok(commit_oid) = Oid::from_str(commit_hash) else {
            return Vec::new();
        };

        self.find_tags_containing_commit(commit_oid)
            .unwrap_or_default()
    }

    /// Ensure all tags are available (fetches if needed).
    fn ensure_tags(&self) -> WtgResult<()> {
        {
            let state = self.fetch_state.lock().expect("fetch state mutex poisoned");
            if state.tags_synced || state.full_metadata_synced {
                return Ok(());
            }
        }

        if !self.allow_fetch {
            return Ok(()); // Don't fetch if not allowed
        }

        let Some(remote_url) = &self.remote_url else {
            return Ok(()); // No remote to fetch from
        };

        fetch_tags(&self.path, remote_url)?;

        self.fetch_state
            .lock()
            .expect("fetch state mutex poisoned")
            .tags_synced = true;

        Ok(())
    }

    /// Convert a GitHub release into tag metadata if the tag exists locally.
    #[must_use]
    pub fn tag_from_release(&self, release: &ReleaseInfo) -> Option<TagInfo> {
        self.with_repo(|repo| {
            let obj = repo.revparse_single(&release.tag_name).ok()?;
            let commit = obj.peel_to_commit().ok()?;
            let semver_info = parse_semver(&release.tag_name);

            Some(TagInfo {
                name: release.tag_name.clone(),
                commit_hash: commit.id().to_string(),
                semver_info,
                is_release: true,
                release_name: release.name.clone(),
                release_url: Some(release.url.clone()),
                published_at: release.published_at,
                created_at: git_time_to_datetime(commit.time()),
            })
        })
    }

    /// Check whether a release tag contains the specified commit.
    #[must_use]
    pub fn tag_contains_commit(&self, tag_commit_hash: &str, commit_hash: &str) -> bool {
        let Ok(tag_oid) = Oid::from_str(tag_commit_hash) else {
            return false;
        };
        let Ok(commit_oid) = Oid::from_str(commit_hash) else {
            return false;
        };

        self.is_ancestor(commit_oid, tag_oid)
    }

    /// Find all tags that contain a given commit (git-only, no GitHub enrichment)
    /// Returns None if no tags contain the commit
    /// Performance: Filters by timestamp before doing expensive ancestry checks
    fn find_tags_containing_commit(&self, commit_oid: Oid) -> Option<Vec<TagInfo>> {
        self.with_repo(|repo| {
            let target_commit = repo.find_commit(commit_oid).ok()?;
            let target_timestamp = target_commit.time().seconds();

            let mut containing_tags = Vec::new();
            let tag_names = repo.tag_names(None).ok()?;

            for tag_name in tag_names.iter().flatten() {
                if let Ok(obj) = repo.revparse_single(tag_name)
                    && let Ok(commit) = obj.peel_to_commit()
                {
                    let tag_oid = commit.id();

                    // Performance: Skip tags with commits older than target
                    // (they cannot possibly contain the target commit)
                    if commit.time().seconds() < target_timestamp {
                        continue;
                    }

                    // Check if this tag points to the commit or if the tag is a descendant
                    if tag_oid == commit_oid
                        || repo
                            .graph_descendant_of(tag_oid, commit_oid)
                            .unwrap_or(false)
                    {
                        let semver_info = parse_semver(tag_name);

                        containing_tags.push(TagInfo {
                            name: tag_name.to_string(),
                            commit_hash: tag_oid.to_string(),
                            semver_info,
                            created_at: git_time_to_datetime(commit.time()),
                            is_release: false,
                            release_name: None,
                            release_url: None,
                            published_at: None,
                        });
                    }
                }
            }

            if containing_tags.is_empty() {
                None
            } else {
                Some(containing_tags)
            }
        })
    }

    /// Get commit timestamp for sorting (helper)
    pub(crate) fn get_commit_timestamp(&self, commit_hash: &str) -> i64 {
        self.with_repo(|repo| {
            Oid::from_str(commit_hash)
                .and_then(|oid| repo.find_commit(oid))
                .map(|c| c.time().seconds())
                .unwrap_or(0)
        })
    }

    /// Check if commit1 is an ancestor of commit2
    fn is_ancestor(&self, ancestor: Oid, descendant: Oid) -> bool {
        self.with_repo(|repo| {
            repo.graph_descendant_of(descendant, ancestor)
                .unwrap_or(false)
        })
    }

    /// Get the GitHub remote info.
    /// Returns stored `repo_info` if set, otherwise extracts from git remotes.
    #[must_use]
    pub fn github_remote(&self) -> Option<GhRepoInfo> {
        // Return stored repo_info if explicitly set (e.g., from remote() constructor)
        if let Some(info) = &self.repo_info {
            return Some(info.clone());
        }

        // Otherwise, extract from git remotes
        self.with_repo(|repo| {
            for remote_name in ["upstream", "origin"] {
                if let Ok(remote) = repo.find_remote(remote_name)
                    && let Some(url) = remote.url()
                    && let Some(repo_info) = parse_github_repo_url(url)
                {
                    return Some(repo_info);
                }
            }

            if let Ok(remotes) = repo.remotes() {
                for remote_name in remotes.iter().flatten() {
                    if let Ok(remote) = repo.find_remote(remote_name)
                        && let Some(url) = remote.url()
                        && let Some(repo_info) = parse_github_repo_url(url)
                    {
                        return Some(repo_info);
                    }
                }
            }

            None
        })
    }

    /// Convert a `git2::Commit` to `CommitInfo`
    fn commit_to_info(commit: &Commit) -> CommitInfo {
        let message = commit.message().unwrap_or("").to_string();
        let lines: Vec<&str> = message.lines().collect();
        let message_lines = lines.len();
        let time = commit.time();

        CommitInfo {
            hash: commit.id().to_string(),
            short_hash: commit.id().to_string()[..7].to_string(),
            message: (*lines.first().unwrap_or(&"")).to_string(),
            message_lines,
            commit_url: None,
            author_name: commit.author().name().unwrap_or("Unknown").to_string(),
            author_email: commit.author().email().map(str::to_string),
            author_login: None,
            author_url: None,
            date: Utc.timestamp_opt(time.seconds(), 0).unwrap(),
        }
    }
}

/// Check if a commit touches a specific file
fn commit_touches_file(commit: &Commit, path: &str) -> bool {
    let Ok(tree) = commit.tree() else {
        return false;
    };

    let target_path = Path::new(path);
    let current_entry = tree.get_path(target_path).ok();

    // Root commit: if the file exists now, this commit introduced it
    if commit.parent_count() == 0 {
        return current_entry.is_some();
    }

    for parent in commit.parents() {
        let Ok(parent_tree) = parent.tree() else {
            continue;
        };

        let previous_entry = parent_tree.get_path(target_path).ok();
        if tree_entries_differ(current_entry.as_ref(), previous_entry.as_ref()) {
            return true;
        }
    }

    false
}

fn tree_entries_differ(
    current: Option<&git2::TreeEntry<'_>>,
    previous: Option<&git2::TreeEntry<'_>>,
) -> bool {
    match (current, previous) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(current_entry), Some(previous_entry)) => {
            current_entry.id() != previous_entry.id()
                || current_entry.filemode() != previous_entry.filemode()
        }
    }
}

/// Regex for parsing semantic versions with various formats
/// Supports:
/// - Optional prefix: py-, rust-, python-, etc.
/// - Optional 'v' prefix
/// - Version: X.Y, X.Y.Z, X.Y.Z.W
/// - Pre-release: -alpha, -beta.1, -rc.1 (dash style) OR a1, b1, rc1 (Python style)
/// - Build metadata: +build.123
static SEMVER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[a-z]+-)?v?(\d+)\.(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:(?:-([a-zA-Z0-9.-]+))|(?:([a-z]+)(\d+)))?(?:\+(.+))?$"
    )
    .expect("Invalid semver regex")
});

/// Parse a semantic version string
/// Supports:
/// - 2-part: 1.0
/// - 3-part: 1.2.3
/// - 4-part: 1.2.3.4
/// - Pre-release: 1.0.0-alpha, 1.0.0-rc.1, 1.0.0-beta.1
/// - Python-style pre-release: 1.2.3a1, 1.2.3b1, 1.2.3rc1
/// - Build metadata: 1.0.0+build.123
/// - With or without 'v' prefix (e.g., v1.0.0)
/// - With custom prefixes (e.g., py-v1.0.0, rust-v1.0.0, python-1.0.0)
pub fn parse_semver(tag: &str) -> Option<SemverInfo> {
    let caps = SEMVER_REGEX.captures(tag)?;

    let major = caps.get(1)?.as_str().parse::<u32>().ok()?;
    let minor = caps.get(2)?.as_str().parse::<u32>().ok()?;
    let patch = caps.get(3).and_then(|m| m.as_str().parse::<u32>().ok());
    let build = caps.get(4).and_then(|m| m.as_str().parse::<u32>().ok());

    // Pre-release can be either:
    // - Group 5: dash-style (-alpha, -beta.1, -rc.1)
    // - Groups 6+7: Python-style (a1, b1, rc1)
    let pre_release = caps.get(5).map_or_else(
        || {
            caps.get(6).map(|py_pre| {
                let py_num = caps
                    .get(7)
                    .map_or(String::new(), |m| m.as_str().to_string());
                format!("{}{}", py_pre.as_str(), py_num)
            })
        },
        |dash_pre| Some(dash_pre.as_str().to_string()),
    );

    let build_metadata = caps.get(8).map(|m| m.as_str().to_string());

    Some(SemverInfo {
        major,
        minor,
        patch,
        build,
        pre_release,
        build_metadata,
    })
}

/// Convert `git2::Time` to `chrono::DateTime<Utc>`
#[must_use]
pub fn git_time_to_datetime(time: git2::Time) -> DateTime<Utc> {
    Utc.timestamp_opt(time.seconds(), 0).unwrap()
}

// ========================================
// Remote/cache helper functions
// ========================================

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
fn update_remote_repo(repo_path: &Path) -> WtgResult<()> {
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
fn fetch_with_git2(repo_path: &Path) -> WtgResult<()> {
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

/// Check if a ref exists on remote without fetching (git ls-remote).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Check if a tag name is a semantic version
    fn is_semver_tag(tag: &str) -> bool {
        parse_semver(tag).is_some()
    }

    #[test]
    fn test_parse_semver_2_part() {
        let result = parse_semver("1.0");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, None);
        assert_eq!(semver.build, None);
    }

    #[test]
    fn test_parse_semver_2_part_with_v_prefix() {
        let result = parse_semver("v2.1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 1);
    }

    #[test]
    fn test_parse_semver_3_part() {
        let result = parse_semver("1.2.3");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.build, None);
    }

    #[test]
    fn test_parse_semver_3_part_with_v_prefix() {
        let result = parse_semver("v1.2.3");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
    }

    #[test]
    fn test_parse_semver_4_part() {
        let result = parse_semver("1.2.3.4");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.build, Some(4));
    }

    #[test]
    fn test_parse_semver_with_pre_release() {
        let result = parse_semver("1.0.0-alpha");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("alpha".to_string()));
    }

    #[test]
    fn test_parse_semver_with_pre_release_numeric() {
        let result = parse_semver("v2.0.0-rc.1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("rc.1".to_string()));
    }

    #[test]
    fn test_parse_semver_with_build_metadata() {
        let result = parse_semver("1.0.0+build.123");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.build_metadata, Some("build.123".to_string()));
    }

    #[test]
    fn test_parse_semver_with_pre_release_and_build() {
        let result = parse_semver("v1.0.0-beta.2+20130313144700");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("beta.2".to_string()));
        assert_eq!(semver.build_metadata, Some("20130313144700".to_string()));
    }

    #[test]
    fn test_parse_semver_2_part_with_pre_release() {
        let result = parse_semver("2.0-alpha");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, None);
        assert_eq!(semver.pre_release, Some("alpha".to_string()));
    }

    #[test]
    fn test_parse_semver_invalid_single_part() {
        assert!(parse_semver("1").is_none());
    }

    #[test]
    fn test_parse_semver_invalid_non_numeric() {
        assert!(parse_semver("abc.def").is_none());
        assert!(parse_semver("1.x.3").is_none());
    }

    #[test]
    fn test_parse_semver_invalid_too_many_parts() {
        assert!(parse_semver("1.2.3.4.5").is_none());
    }

    #[test]
    fn test_is_semver_tag() {
        // Basic versions
        assert!(is_semver_tag("1.0"));
        assert!(is_semver_tag("v1.0"));
        assert!(is_semver_tag("1.2.3"));
        assert!(is_semver_tag("v1.2.3"));
        assert!(is_semver_tag("1.2.3.4"));

        // Pre-release versions
        assert!(is_semver_tag("1.0.0-alpha"));
        assert!(is_semver_tag("v2.0.0-rc.1"));
        assert!(is_semver_tag("1.2.3-beta.2"));

        // Python-style pre-release
        assert!(is_semver_tag("1.2.3a1"));
        assert!(is_semver_tag("1.2.3b1"));
        assert!(is_semver_tag("1.2.3rc1"));

        // Build metadata
        assert!(is_semver_tag("1.0.0+build"));

        // Custom prefixes
        assert!(is_semver_tag("py-v1.0.0"));
        assert!(is_semver_tag("rust-v1.2.3-beta.1"));
        assert!(is_semver_tag("python-1.2.3b1"));

        // Invalid
        assert!(!is_semver_tag("v1"));
        assert!(!is_semver_tag("abc"));
        assert!(!is_semver_tag("1.2.3.4.5"));
        assert!(!is_semver_tag("server-v-1.0.0")); // Double dash should fail
    }

    #[test]
    fn test_parse_semver_with_custom_prefix() {
        // Test py-v prefix
        let result = parse_semver("py-v1.0.0-beta.1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("beta.1".to_string()));

        // Test rust-v prefix
        let result = parse_semver("rust-v1.0.0-beta.2");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("beta.2".to_string()));

        // Test prefix without v
        let result = parse_semver("python-2.1.0");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 1);
        assert_eq!(semver.patch, Some(0));
    }

    #[test]
    fn test_parse_semver_python_style() {
        // Alpha
        let result = parse_semver("1.2.3a1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.pre_release, Some("a1".to_string()));

        // Beta
        let result = parse_semver("v1.2.3b2");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.pre_release, Some("b2".to_string()));

        // Release candidate
        let result = parse_semver("2.0.0rc1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("rc1".to_string()));

        // With prefix
        let result = parse_semver("py-v1.0.0b1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("b1".to_string()));
    }

    #[test]
    fn test_parse_semver_rejects_garbage() {
        // Should reject random strings with -v in them
        assert!(parse_semver("server-v-config").is_none());
        assert!(parse_semver("whatever-v-something").is_none());

        // Should reject malformed versions
        assert!(parse_semver("v1").is_none());
        assert!(parse_semver("1").is_none());
        assert!(parse_semver("1.2.3.4.5").is_none());
        assert!(parse_semver("abc.def").is_none());
    }

    #[test]
    fn file_history_tracks_content_and_metadata_changes() {
        const ORIGINAL_PATH: &str = "config/policy.json";
        const RENAMED_PATH: &str = "config/policy-renamed.json";
        const EXECUTABLE_PATH: &str = "scripts/run.sh";
        const DELETED_PATH: &str = "docs/legacy.md";
        const DISTRACTION_PATH: &str = "README.md";

        let temp = tempdir().expect("temp dir");
        let repo = Repository::init(temp.path()).expect("git repo");

        commit_file(&repo, DISTRACTION_PATH, "noise", "add distraction");
        commit_file(&repo, ORIGINAL_PATH, "{\"version\":1}", "seed config");
        commit_file(&repo, ORIGINAL_PATH, "{\"version\":2}", "config tweak");
        let rename_commit = rename_file(&repo, ORIGINAL_PATH, RENAMED_PATH, "rename config");
        let post_rename_commit = commit_file(
            &repo,
            RENAMED_PATH,
            "{\"version\":3}",
            "update renamed config",
        );

        commit_file(
            &repo,
            EXECUTABLE_PATH,
            "#!/bin/sh\\nprintf hi\n",
            "add runner",
        );
        let exec_mode_commit = change_file_mode(
            &repo,
            EXECUTABLE_PATH,
            git2::FileMode::BlobExecutable,
            "make runner executable",
        );

        commit_file(&repo, DELETED_PATH, "bye", "add temporary file");
        let delete_commit = delete_file(&repo, DELETED_PATH, "remove temporary file");

        let git_repo = GitRepo::from_path(temp.path()).expect("git repo wrapper");

        let renamed_info = git_repo.find_file(RENAMED_PATH).expect("renamed file info");
        assert_eq!(
            renamed_info.last_commit.hash,
            post_rename_commit.to_string()
        );

        let original_info = git_repo
            .find_file(ORIGINAL_PATH)
            .expect("original file info");
        assert_eq!(original_info.last_commit.hash, rename_commit.to_string());

        let exec_info = git_repo.find_file(EXECUTABLE_PATH).expect("exec file info");
        assert_eq!(exec_info.last_commit.hash, exec_mode_commit.to_string());

        let deleted_info = git_repo.find_file(DELETED_PATH).expect("deleted file info");
        assert_eq!(deleted_info.last_commit.hash, delete_commit.to_string());
    }

    fn commit_file(repo: &Repository, path: &str, contents: &str, message: &str) -> git2::Oid {
        let workdir = repo.workdir().expect("workdir");
        let file_path = workdir.join(path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).expect("create dir");
        }
        fs::write(&file_path, contents).expect("write file");

        let mut index = repo.index().expect("index");
        index.add_path(Path::new(path)).expect("add path");
        write_tree_and_commit(repo, &mut index, message)
    }

    fn rename_file(repo: &Repository, from: &str, to: &str, message: &str) -> git2::Oid {
        let workdir = repo.workdir().expect("workdir");
        let from_path = workdir.join(from);
        let to_path = workdir.join(to);
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent).expect("create dir");
        }
        fs::rename(&from_path, &to_path).expect("rename file");

        let mut index = repo.index().expect("index");
        index.remove_path(Path::new(from)).expect("remove old path");
        index.add_path(Path::new(to)).expect("add new path");
        write_tree_and_commit(repo, &mut index, message)
    }

    fn delete_file(repo: &Repository, path: &str, message: &str) -> git2::Oid {
        let workdir = repo.workdir().expect("workdir");
        let file_path = workdir.join(path);
        if file_path.exists() {
            fs::remove_file(&file_path).expect("remove file");
        }

        let mut index = repo.index().expect("index");
        index.remove_path(Path::new(path)).expect("remove path");
        write_tree_and_commit(repo, &mut index, message)
    }

    fn change_file_mode(
        repo: &Repository,
        path: &str,
        mode: git2::FileMode,
        message: &str,
    ) -> git2::Oid {
        let mut index = repo.index().expect("index");
        index.add_path(Path::new(path)).expect("add path");
        force_index_mode(&mut index, path, mode);
        write_tree_and_commit(repo, &mut index, message)
    }

    fn force_index_mode(index: &mut git2::Index, path: &str, mode: git2::FileMode) {
        if let Some(mut entry) = index.get_path(Path::new(path), 0) {
            entry.mode = u32::try_from(i32::from(mode)).expect("valid file mode");
            index.add(&entry).expect("re-add entry");
        }
    }

    fn write_tree_and_commit(
        repo: &Repository,
        index: &mut git2::Index,
        message: &str,
    ) -> git2::Oid {
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("tree oid");
        let tree = repo.find_tree(tree_oid).expect("tree");
        let sig = test_signature();

        let parents = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok())
            .into_iter()
            .collect::<Vec<_>>();
        let parent_refs = parents.iter().collect::<Vec<_>>();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
    }

    fn test_signature() -> git2::Signature<'static> {
        git2::Signature::now("Test User", "tester@example.com").expect("sig")
    }
}
