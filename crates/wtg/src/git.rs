use crate::error::{Result, WtgError};
use git2::{Repository, Oid, Commit, Time};
use std::path::Path;

pub struct GitRepo {
    repo: Repository,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub message_lines: usize,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
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
    pub is_semver: bool,
}

impl GitRepo {
    /// Open the git repository from the current directory
    pub fn open() -> Result<Self> {
        let repo = Repository::discover(".").map_err(|_| WtgError::NotInGitRepo)?;
        Ok(Self { repo })
    }

    /// Get the repository path
    pub fn path(&self) -> &Path {
        self.repo.path()
    }

    /// Try to find a commit by hash (can be short or full)
    pub fn find_commit(&self, hash_str: &str) -> Option<CommitInfo> {
        // Try to parse as OID
        if let Ok(oid) = Oid::from_str(hash_str) {
            if let Ok(commit) = self.repo.find_commit(oid) {
                return Some(self.commit_to_info(&commit));
            }
        }

        // Try as short hash - iterate through all commits
        if hash_str.len() >= 7 {
            if let Ok(obj) = self.repo.revparse_single(hash_str) {
                if let Ok(commit) = obj.peel_to_commit() {
                    return Some(self.commit_to_info(&commit));
                }
            }
        }

        None
    }

    /// Find a file in the repository
    pub fn find_file(&self, path: &str) -> Option<FileInfo> {
        // Get the last commit that touched this file
        // (checks both worktree and git history)
        let mut revwalk = self.repo.revwalk().ok()?;
        revwalk.push_head().ok()?;

        for oid in revwalk {
            let oid = oid.ok()?;
            let commit = self.repo.find_commit(oid).ok()?;

            // Check if this commit touched the file
            if self.commit_touches_file(&commit, path) {
                let commit_info = self.commit_to_info(&commit);

                // Get previous authors (up to 4 more)
                let previous_authors = self.get_previous_authors(path, &commit, 4);

                return Some(FileInfo {
                    path: path.to_string(),
                    last_commit: commit_info,
                    previous_authors,
                });
            }
        }

        None
    }

    /// Check if a commit touches a specific file
    fn commit_touches_file(&self, commit: &Commit, path: &str) -> bool {
        let tree = match commit.tree() {
            Ok(t) => t,
            Err(_) => return false,
        };

        // Check if the file exists in this commit's tree
        tree.get_path(Path::new(path)).is_ok()
    }

    /// Get previous authors for a file (excluding the last commit)
    fn get_previous_authors(&self, path: &str, last_commit: &Commit, limit: usize) -> Vec<(String, String, String)> {
        let mut authors = Vec::new();
        let mut revwalk = match self.repo.revwalk() {
            Ok(rw) => rw,
            Err(_) => return authors,
        };

        if revwalk.push_head().is_err() {
            return authors;
        }

        let mut found_last = false;

        for oid in revwalk {
            if authors.len() >= limit {
                break;
            }

            let oid = match oid {
                Ok(o) => o,
                Err(_) => continue,
            };

            let commit = match self.repo.find_commit(oid) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Skip until we pass the last commit
            if commit.id() == last_commit.id() {
                found_last = true;
                continue;
            }

            if !found_last {
                continue;
            }

            // Check if this commit touched the file
            if self.commit_touches_file(&commit, path) {
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
    pub fn get_tags(&self) -> Vec<TagInfo> {
        let mut tags = Vec::new();

        if let Ok(tag_names) = self.repo.tag_names(None) {
            for tag_name in tag_names.iter().flatten() {
                if let Ok(obj) = self.repo.revparse_single(tag_name) {
                    if let Ok(commit) = obj.peel_to_commit() {
                        let is_semver = is_semver_tag(tag_name);
                        tags.push(TagInfo {
                            name: tag_name.to_string(),
                            commit_hash: commit.id().to_string(),
                            is_semver,
                        });
                    }
                }
            }
        }

        tags
    }

    /// Find the closest release that contains a given commit
    pub fn find_closest_release(&self, commit_hash: &str) -> Option<TagInfo> {
        let commit_oid = Oid::from_str(commit_hash).ok()?;
        let tags = self.get_tags();

        // Filter to only semver tags for releases
        let release_tags: Vec<_> = tags.into_iter()
            .filter(|t| t.is_semver)
            .collect();

        // Find tags that contain this commit
        let mut containing_tags = Vec::new();

        for tag in release_tags {
            let tag_oid = Oid::from_str(&tag.commit_hash).ok()?;

            // Check if commit is ancestor of tag (i.e., tag contains commit)
            if self.is_ancestor(commit_oid, tag_oid) {
                containing_tags.push(tag);
            }
        }

        if containing_tags.is_empty() {
            return None;
        }

        // Sort by commit date (oldest first) and return the first one
        containing_tags.sort_by_key(|t| {
            Oid::from_str(&t.commit_hash)
                .and_then(|oid| self.repo.find_commit(oid))
                .map(|c| c.time().seconds())
                .unwrap_or(0)
        });

        containing_tags.into_iter().next()
    }

    /// Check if commit1 is an ancestor of commit2
    fn is_ancestor(&self, ancestor: Oid, descendant: Oid) -> bool {
        self.repo.graph_descendant_of(descendant, ancestor).unwrap_or(false)
    }

    /// Get the GitHub remote URL if it exists (checks all remotes)
    pub fn github_remote(&self) -> Option<(String, String)> {
        // Try common remote names first (origin, upstream)
        for remote_name in ["origin", "upstream"] {
            if let Ok(remote) = self.repo.find_remote(remote_name) {
                if let Some(url) = remote.url() {
                    if let Some(github_info) = parse_github_url(url) {
                        return Some(github_info);
                    }
                }
            }
        }

        // If not found in common names, check all remotes
        if let Ok(remotes) = self.repo.remotes() {
            for remote_name in remotes.iter().flatten() {
                if let Ok(remote) = self.repo.find_remote(remote_name) {
                    if let Some(url) = remote.url() {
                        if let Some(github_info) = parse_github_url(url) {
                            return Some(github_info);
                        }
                    }
                }
            }
        }

        None
    }

    /// Convert a git2::Commit to CommitInfo
    fn commit_to_info(&self, commit: &Commit) -> CommitInfo {
        let message = commit.message().unwrap_or("").to_string();
        let lines: Vec<&str> = message.lines().collect();
        let message_lines = lines.len();

        CommitInfo {
            hash: commit.id().to_string(),
            short_hash: commit.id().to_string()[..7].to_string(),
            message: lines.first().unwrap_or(&"").to_string(),
            message_lines,
            author_name: commit.author().name().unwrap_or("Unknown").to_string(),
            author_email: commit.author().email().unwrap_or("").to_string(),
            date: format_git_time(&commit.time()),
        }
    }
}

/// Check if a tag name is a semantic version
fn is_semver_tag(tag: &str) -> bool {
    let tag = tag.strip_prefix('v').unwrap_or(tag);

    // Simple semver check: X.Y.Z pattern
    let parts: Vec<&str> = tag.split('.').collect();
    if parts.len() != 3 {
        return false;
    }

    parts.iter().all(|p| p.parse::<u32>().is_ok())
}

/// Parse a GitHub URL to extract owner and repo
fn parse_github_url(url: &str) -> Option<(String, String)> {
    // Handle both HTTPS and SSH URLs
    // HTTPS: https://github.com/owner/repo.git
    // SSH: git@github.com:owner/repo.git

    if url.contains("github.com") {
        let parts: Vec<&str> = if url.starts_with("git@") {
            url.split(':').collect()
        } else {
            url.split("github.com/").collect()
        };

        if let Some(path) = parts.last() {
            let path = path.trim_end_matches(".git");
            let repo_parts: Vec<&str> = path.split('/').collect();
            if repo_parts.len() >= 2 {
                return Some((repo_parts[0].to_string(), repo_parts[1].to_string()));
            }
        }
    }

    None
}

/// Format git time to a human-readable string
fn format_git_time(time: &Time) -> String {
    use chrono::{DateTime, Utc, TimeZone};

    let datetime: DateTime<Utc> = Utc.timestamp_opt(time.seconds(), 0).unwrap();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}
