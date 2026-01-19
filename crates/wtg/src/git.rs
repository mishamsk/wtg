use std::{
    collections::HashSet,
    fs,
    io::{Error as IoError, ErrorKind},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, TimeZone, Utc};
use git2::{Commit, FetchOptions, Oid, RemoteCallbacks, Repository};

use crate::error::{WtgError, WtgResult};
use crate::github::{GhRepoInfo, ReleaseInfo};
use crate::notice::{Notice, NoticeCallback, no_notices};
use crate::parse_input::parse_github_repo_url;
use crate::remote::{RemoteHost, RemoteInfo, RemoteKind};
pub use crate::semver::{SemverInfo, parse_semver};

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
    /// GitHub repository info (owner/repo) if explicitly set
    gh_repo_info: Option<GhRepoInfo>,
    /// Whether fetching is allowed
    allow_fetch: bool,
    /// Tracks what's been synced from remote
    fetch_state: Mutex<FetchState>,
    /// Callback for emitting notices
    notice_cb: NoticeCallback,
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
    pub tag_url: Option<String>, // URL to view the tag (tree for plain tags, release page for releases)
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
            gh_repo_info: None,
            allow_fetch: false,
            fetch_state: Mutex::new(FetchState::default()),
            notice_cb: no_notices(),
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
            gh_repo_info: None,
            allow_fetch: false,
            fetch_state: Mutex::new(FetchState::default()),
            notice_cb: no_notices(),
        })
    }

    /// Open or clone a remote GitHub repository.
    /// Uses a cache directory (~/.cache/wtg/repos). Fetch is enabled by default.
    pub fn remote(gh_repo_info: GhRepoInfo) -> WtgResult<Self> {
        Self::remote_with_notices(gh_repo_info, no_notices())
    }

    /// Open or clone a remote GitHub repository with a notice callback.
    /// Uses a cache directory (~/.cache/wtg/repos). Fetch is enabled by default.
    pub fn remote_with_notices(
        gh_repo_info: GhRepoInfo,
        notice_cb: NoticeCallback,
    ) -> WtgResult<Self> {
        let emit = |n: Notice| (notice_cb)(n);

        let cache_dir = get_cache_dir()?;
        let repo_cache_path =
            cache_dir.join(format!("{}/{}", gh_repo_info.owner(), gh_repo_info.repo()));

        // Check if already cloned
        let full_metadata_synced =
            if repo_cache_path.exists() && Repository::open(&repo_cache_path).is_ok() {
                // Cache exists - try to fetch to ensure metadata is fresh
                match update_remote_repo(&repo_cache_path, &emit) {
                    Ok(()) => true,
                    Err(e) => {
                        emit(Notice::CacheUpdateFailed {
                            error: e.to_string(),
                        });
                        false // Continue with stale cache
                    }
                }
            } else {
                // Clone it (with filter=blob:none for efficiency)
                clone_remote_repo(
                    gh_repo_info.owner(),
                    gh_repo_info.repo(),
                    &repo_cache_path,
                    &emit,
                )?;
                true // Fresh clone has all metadata
            };

        let repo = Repository::open(&repo_cache_path).map_err(|_| WtgError::NotInGitRepo)?;
        let path = repo.path().to_path_buf();
        let remote_url = Some(format!(
            "https://github.com/{}/{}.git",
            gh_repo_info.owner(),
            gh_repo_info.repo()
        ));

        Ok(Self {
            repo: Arc::new(Mutex::new(repo)),
            path,
            remote_url,
            gh_repo_info: Some(gh_repo_info),
            allow_fetch: true,
            fetch_state: Mutex::new(FetchState {
                full_metadata_synced,
                ..Default::default()
            }),
            notice_cb,
        })
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

    /// Set the notice callback for emitting operational messages.
    pub fn set_notice_callback(&mut self, cb: NoticeCallback) {
        self.notice_cb = cb;
    }

    /// Emit a notice via the callback.
    fn emit(&self, notice: Notice) {
        (self.notice_cb)(notice);
    }

    /// Get a reference to the stored GitHub repo info (owner/repo) if explicitly set.
    #[must_use]
    pub const fn gh_repo_info(&self) -> Option<&GhRepoInfo> {
        self.gh_repo_info.as_ref()
    }

    fn with_repo<T>(&self, f: impl FnOnce(&Repository) -> T) -> T {
        let repo = self.repo.lock().expect("git repository mutex poisoned");
        f(&repo)
    }

    /// Collect all remotes from a repository as `RemoteInfo` structs.
    fn collect_remotes(repo: &Repository) -> Vec<RemoteInfo> {
        let remote_names: Vec<String> = repo
            .remotes()
            .map(|names| names.iter().flatten().map(str::to_string).collect())
            .unwrap_or_default();

        remote_names
            .into_iter()
            .filter_map(|name| {
                let remote = repo.find_remote(&name).ok()?;
                let url = remote.url()?.to_string();
                Some(RemoteInfo {
                    name: name.clone(),
                    kind: RemoteKind::from_name(&name),
                    host: RemoteHost::from_url(&url),
                    url,
                })
            })
            .collect()
    }

    /// Extract remote URL from repository, preferring upstream over origin.
    fn extract_remote_url(repo: &Repository) -> Option<String> {
        let mut remotes = Self::collect_remotes(repo);
        remotes.sort_by_key(RemoteInfo::priority);
        remotes.into_iter().next().map(|r| r.url)
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
            self.emit(Notice::ShallowRepoDetected);
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
    pub fn find_commit_local(&self, hash_str: &str) -> Option<CommitInfo> {
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

    pub fn has_path_at_head(&self, path: &str) -> bool {
        self.with_repo(|repo| {
            let Ok(head) = repo.head() else {
                return false;
            };
            let Ok(commit) = head.peel_to_commit() else {
                return false;
            };
            let Ok(tree) = commit.tree() else {
                return false;
            };
            tree.get_path(Path::new(path)).is_ok()
        })
    }

    pub fn has_tag_named(&self, name: &str) -> bool {
        self.get_tags().into_iter().any(|tag| tag.name == name)
    }

    pub fn find_branch_path_match(&self, segments: &[String]) -> Option<(String, Vec<String>)> {
        // Collect candidates inside the closure to avoid lifetime issues with References
        let candidates: Vec<(String, Vec<String>)> = self.with_repo(|repo| {
            let refs = repo.references().ok()?;
            let mut candidates = Vec::new();

            for reference in refs.flatten() {
                let Some(name) = reference.name().and_then(|n| n.strip_prefix("refs/heads/"))
                else {
                    continue;
                };
                let branch_segments: Vec<&str> = name.split('/').collect();
                if branch_segments.len() > segments.len() {
                    continue;
                }
                let matches_prefix = branch_segments
                    .iter()
                    .zip(segments.iter())
                    .all(|(branch, segment)| *branch == segment.as_str());
                if matches_prefix {
                    let remainder: Vec<String> = segments[branch_segments.len()..].to_vec();
                    candidates.push((name.to_string(), remainder));
                }
            }
            Some(candidates)
        })?;

        // Filter candidates by checking path existence outside the closure
        let valid: Vec<_> = candidates
            .into_iter()
            .filter(|(branch, remainder)| self.branch_path_exists(branch, remainder))
            .collect();

        if valid.len() == 1 {
            return Some(valid.into_iter().next().unwrap());
        }

        None
    }

    fn branch_path_exists(&self, branch: &str, segments: &[String]) -> bool {
        if segments.is_empty() {
            return false;
        }

        let mut path = PathBuf::new();
        for segment in segments {
            path.push(segment);
        }

        self.with_repo(|repo| {
            let Ok(obj) = repo.revparse_single(branch) else {
                return false;
            };
            let Ok(commit) = obj.peel_to_commit() else {
                return false;
            };
            let Ok(tree) = commit.tree() else {
                return false;
            };
            tree.get_path(&path).is_ok()
        })
    }

    /// Find a file in the repository
    #[must_use]
    pub fn find_file_on_branch(&self, branch: &str, path: &str) -> Option<FileInfo> {
        self.with_repo(|repo| {
            let obj = repo.revparse_single(branch).ok()?;
            let commit = obj.peel_to_commit().ok()?;
            let mut revwalk = repo.revwalk().ok()?;
            revwalk.push(commit.id()).ok()?;

            for oid in revwalk {
                let oid = oid.ok()?;
                let commit = repo.find_commit(oid).ok()?;

                if commit_touches_file(&commit, path) {
                    let commit_info = Self::commit_to_info(&commit);
                    let previous_authors =
                        Self::get_previous_authors_from(repo, path, &commit, 4, |revwalk| {
                            revwalk.push(commit.id())
                        });

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

    fn get_previous_authors_from(
        repo: &Repository,
        path: &str,
        last_commit: &Commit,
        limit: usize,
        seed_revwalk: impl FnOnce(&mut git2::Revwalk) -> Result<(), git2::Error>,
    ) -> Vec<(String, String, String)> {
        let mut authors = Vec::new();
        let Ok(mut revwalk) = repo.revwalk() else {
            return authors;
        };

        if seed_revwalk(&mut revwalk).is_err() {
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

            if !found_last {
                if commit.id() == last_commit.id() {
                    found_last = true;
                }
                continue;
            }

            if !commit_touches_file(&commit, path) {
                continue;
            }

            let author = commit.author();
            let name = author.name().unwrap_or("Unknown").to_string();
            let email = author.email().unwrap_or("").to_string();

            // Skip duplicates
            if !authors.iter().any(|(_, n, e)| *n == name && *e == email) {
                authors.push((commit.id().to_string(), name, email));
            }
        }

        authors
    }

    /// Get all tags in the repository.
    #[must_use]
    pub fn get_tags(&self) -> Vec<TagInfo> {
        self.with_repo(|repo| {
            let mut tags = Vec::new();

            if let Ok(tag_names) = repo.tag_names(None) {
                for tag_name in tag_names.iter().flatten() {
                    if let Ok(obj) = repo.revparse_single(tag_name)
                        && let Ok(commit) = obj.peel_to_commit()
                    {
                        tags.push(TagInfo {
                            name: tag_name.to_string(),
                            commit_hash: commit.id().to_string(),
                            semver_info: parse_semver(tag_name),
                            created_at: git_time_to_datetime(commit.time()),
                            is_release: false,
                            release_name: None,
                            release_url: None,
                            published_at: None,
                            tag_url: None,
                        });
                    }
                }
            }

            tags
        })
    }

    /// Get commits between two refs (from exclusive, to inclusive).
    /// Returns commits in reverse chronological order (most recent first).
    pub fn commits_between(&self, from_ref: &str, to_ref: &str, limit: usize) -> Vec<CommitInfo> {
        self.with_repo(|repo| {
            let mut result = Vec::new();

            let Ok(to_obj) = repo.revparse_single(to_ref) else {
                return result;
            };
            let Ok(to_commit) = to_obj.peel_to_commit() else {
                return result;
            };

            let Ok(from_obj) = repo.revparse_single(from_ref) else {
                return result;
            };
            let Ok(from_commit) = from_obj.peel_to_commit() else {
                return result;
            };

            let Ok(mut revwalk) = repo.revwalk() else {
                return result;
            };

            // Walk from to_ref back, stopping at from_ref
            if revwalk.push(to_commit.id()).is_err() {
                return result;
            }
            if revwalk.hide(from_commit.id()).is_err() {
                return result;
            }

            for oid in revwalk.take(limit) {
                let Ok(oid) = oid else { continue };
                let Ok(commit) = repo.find_commit(oid) else {
                    continue;
                };
                result.push(Self::commit_to_info(&commit));
            }

            result
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
                tag_url: Some(release.url.clone()),
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
                            tag_url: None,
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

    /// Iterate over all remotes in the repository.
    /// Returns an iterator of `RemoteInfo`.
    pub fn remotes(&self) -> impl Iterator<Item = RemoteInfo> {
        self.with_repo(Self::collect_remotes).into_iter()
    }

    /// Get the GitHub remote info.
    /// Returns stored `gh_repo_info` if set, otherwise extracts from git remotes
    /// using the `remotes()` API with priority ordering (upstream > origin > other,
    /// GitHub remotes first within each kind).
    #[must_use]
    pub fn github_remote(&self) -> Option<GhRepoInfo> {
        // Return stored gh_repo_info if explicitly set (e.g., from remote() constructor)
        if let Some(info) = &self.gh_repo_info {
            return Some(info.clone());
        }

        // Use remotes() API to find the best GitHub remote
        let mut remotes: Vec<_> = self.remotes().collect();
        remotes.sort_by_key(RemoteInfo::priority);

        // Find the first GitHub remote and parse its URL
        remotes
            .into_iter()
            .find(|r| r.host == Some(RemoteHost::GitHub))
            .and_then(|r| parse_github_repo_url(&r.url))
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

/// Check if a string looks like a git commit hash (7-40 hex characters).
pub(crate) fn looks_like_commit_hash(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.len() >= 7 && trimmed.len() <= 40 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit())
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
            WtgError::Io(IoError::new(
                ErrorKind::NotFound,
                "Could not determine cache directory",
            ))
        })?
        .join("wtg")
        .join("repos");

    if !cache_dir.exists() {
        fs::create_dir_all(&cache_dir)?;
    }

    Ok(cache_dir)
}

/// Clone a remote repository using subprocess with filter=blob:none, falling back to git2 if needed
fn clone_remote_repo(
    owner: &str,
    repo: &str,
    target_path: &Path,
    emit: &dyn Fn(Notice),
) -> WtgResult<()> {
    // Create parent directory
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let repo_url = format!("https://github.com/{owner}/{repo}.git");

    emit(Notice::CloningRepo {
        url: repo_url.clone(),
    });

    // Try subprocess with --filter=blob:none first (requires Git 2.17+)
    match clone_with_filter(&repo_url, target_path) {
        Ok(()) => {
            emit(Notice::CloneSucceeded { used_filter: true });
            Ok(())
        }
        Err(e) => {
            emit(Notice::CloneFallbackToBare {
                error: e.to_string(),
            });
            // Fall back to git2 bare clone
            clone_bare_with_git2(&repo_url, target_path, emit)
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
                WtgError::Io(IoError::new(ErrorKind::InvalidInput, "Invalid path"))
            })?,
        ])
        .output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(WtgError::Io(IoError::other(format!(
            "Failed to clone with filter: {error}"
        ))));
    }

    Ok(())
}

/// Clone bare repository using git2 (fallback)
fn clone_bare_with_git2(
    repo_url: &str,
    target_path: &Path,
    emit: &dyn Fn(Notice),
) -> WtgResult<()> {
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

    emit(Notice::CloneSucceeded { used_filter: false });

    Ok(())
}

/// Update an existing cloned remote repository
fn update_remote_repo(repo_path: &Path, emit: &dyn Fn(Notice)) -> WtgResult<()> {
    emit(Notice::UpdatingCache);

    // Try subprocess fetch first (works for both filter and non-filter repos)
    match fetch_with_subprocess(repo_path) {
        Ok(()) => {
            emit(Notice::CacheUpdated);
            Ok(())
        }
        Err(_) => {
            // Fall back to git2
            fetch_with_git2(repo_path, emit)
        }
    }
}

/// Fetch updates using subprocess
fn fetch_with_subprocess(repo_path: &Path) -> WtgResult<()> {
    let args = build_fetch_args(repo_path)?;

    let output = Command::new("git").args(&args).output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(WtgError::Io(IoError::other(format!(
            "Failed to fetch: {error}"
        ))));
    }

    Ok(())
}

/// Build the arguments passed to `git fetch` when refreshing cached repos.
fn build_fetch_args(repo_path: &Path) -> WtgResult<Vec<String>> {
    let repo_path = repo_path
        .to_str()
        .ok_or_else(|| WtgError::Io(IoError::new(ErrorKind::InvalidInput, "Invalid path")))?;

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
fn fetch_with_git2(repo_path: &Path, emit: &dyn Fn(Notice)) -> WtgResult<()> {
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

    emit(Notice::CacheUpdated);

    Ok(())
}

/// Check if a ref exists on remote without fetching (git ls-remote).
fn ls_remote_ref_exists(remote_url: &str, ref_spec: &str) -> WtgResult<bool> {
    let output = Command::new("git")
        .args(["ls-remote", "--exit-code", remote_url, ref_spec])
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status();

    match output {
        Ok(status) => Ok(status.success()),
        Err(e) => Err(WtgError::Io(e)),
    }
}

/// Fetch a specific commit by hash.
fn fetch_commit(repo_path: &Path, remote_url: &str, hash: &str) -> WtgResult<()> {
    let repo_path_str = repo_path
        .to_str()
        .ok_or_else(|| WtgError::Io(IoError::new(ErrorKind::InvalidInput, "Invalid path")))?;

    let output = Command::new("git")
        .args(["-C", repo_path_str, "fetch", "--depth=1", remote_url, hash])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(WtgError::Io(IoError::other(format!(
            "Failed to fetch commit {hash}: {stderr}"
        ))))
    }
}

/// Fetch all tags from remote.
fn fetch_tags(repo_path: &Path, remote_url: &str) -> WtgResult<()> {
    let repo_path_str = repo_path
        .to_str()
        .ok_or_else(|| WtgError::Io(IoError::new(ErrorKind::InvalidInput, "Invalid path")))?;

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
        Err(WtgError::Io(IoError::other(format!(
            "Failed to fetch tags: {stderr}"
        ))))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

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

        let renamed_info = git_repo
            .find_file_on_branch("HEAD", RENAMED_PATH)
            .expect("renamed file info");
        assert_eq!(
            renamed_info.last_commit.hash,
            post_rename_commit.to_string()
        );

        let original_info = git_repo
            .find_file_on_branch("HEAD", ORIGINAL_PATH)
            .expect("original file info");
        assert_eq!(original_info.last_commit.hash, rename_commit.to_string());

        let exec_info = git_repo
            .find_file_on_branch("HEAD", EXECUTABLE_PATH)
            .expect("exec file info");
        assert_eq!(exec_info.last_commit.hash, exec_mode_commit.to_string());

        let deleted_info = git_repo
            .find_file_on_branch("HEAD", DELETED_PATH)
            .expect("deleted file info");
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
