use crate::error::{Result, WtgError};
use crate::github::ReleaseInfo;
use git2::{Commit, Oid, Repository, Time};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

#[derive(Clone)]
pub struct GitRepo {
    repo: Arc<Mutex<Repository>>,
    path: PathBuf,
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
    pub timestamp: i64, // Unix timestamp for the commit
}

impl CommitInfo {
    /// Get the commit date as an RFC3339 string for GitHub API filtering
    #[must_use]
    pub fn date_rfc3339(&self) -> String {
        use chrono::{DateTime, TimeZone, Utc};
        let datetime: DateTime<Utc> = Utc.timestamp_opt(self.timestamp, 0).unwrap();
        datetime.to_rfc3339()
    }
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
    pub semver_info: Option<SemverInfo>,
    pub is_release: bool,             // Whether this is a GitHub release
    pub release_name: Option<String>, // GitHub release name (if is_release)
    pub release_url: Option<String>,  // GitHub release URL (if is_release)
    pub published_at: Option<String>, // GitHub release published date (if is_release)
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
    /// Open the git repository from the current directory
    pub fn open() -> Result<Self> {
        let repo = Repository::discover(".").map_err(|_| WtgError::NotInGitRepo)?;
        let path = repo.path().to_path_buf();
        Ok(Self {
            repo: Arc::new(Mutex::new(repo)),
            path,
        })
    }

    /// Open the git repository from a specific path
    pub fn from_path(path: &Path) -> Result<Self> {
        let repo = Repository::open(path).map_err(|_| WtgError::NotInGitRepo)?;
        let repo_path = repo.path().to_path_buf();
        Ok(Self {
            repo: Arc::new(Mutex::new(repo)),
            path: repo_path,
        })
    }

    /// Get the repository path
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_repo<T>(&self, f: impl FnOnce(&Repository) -> T) -> T {
        let repo = self.repo.lock().expect("git repository mutex poisoned");
        f(&repo)
    }

    /// Try to find a commit by hash (can be short or full)
    #[must_use]
    pub fn find_commit(&self, hash_str: &str) -> Option<CommitInfo> {
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
                        let is_semver = semver_info.is_some();

                        let (is_release, release_name, release_url, published_at) = release_map
                            .get(tag_name)
                            .map_or((false, None, None, None), |release| {
                                (
                                    true,
                                    release.name.clone(),
                                    Some(release.url.clone()),
                                    release.published_at.clone(),
                                )
                            });

                        tags.push(TagInfo {
                            name: tag_name.to_string(),
                            commit_hash: commit.id().to_string(),
                            is_semver,
                            semver_info,
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
    #[must_use]
    pub fn tags_containing_commit(&self, commit_hash: &str) -> Vec<TagInfo> {
        let Ok(commit_oid) = Oid::from_str(commit_hash) else {
            return Vec::new();
        };

        self.find_tags_containing_commit(commit_oid)
            .unwrap_or_default()
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
                is_semver: semver_info.is_some(),
                semver_info,
                is_release: true,
                release_name: release.name.clone(),
                release_url: Some(release.url.clone()),
                published_at: release.published_at.clone(),
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
                            is_semver: semver_info.is_some(),
                            semver_info,
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

    /// Get the GitHub remote URL if it exists (checks all remotes)
    #[must_use]
    pub fn github_remote(&self) -> Option<(String, String)> {
        self.with_repo(|repo| {
            for remote_name in ["origin", "upstream"] {
                if let Ok(remote) = repo.find_remote(remote_name)
                    && let Some(url) = remote.url()
                    && let Some(github_info) = parse_github_url(url)
                {
                    return Some(github_info);
                }
            }

            if let Ok(remotes) = repo.remotes() {
                for remote_name in remotes.iter().flatten() {
                    if let Ok(remote) = repo.find_remote(remote_name)
                        && let Some(url) = remote.url()
                        && let Some(github_info) = parse_github_url(url)
                    {
                        return Some(github_info);
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
            author_name: commit.author().name().unwrap_or("Unknown").to_string(),
            author_email: commit.author().email().unwrap_or("").to_string(),
            date: format_git_time(&time),
            timestamp: time.seconds(),
        }
    }
}

/// Check if a commit touches a specific file
fn commit_touches_file(commit: &Commit, path: &str) -> bool {
    let Ok(tree) = commit.tree() else {
        return false;
    };

    // Check if the file exists in this commit's tree
    tree.get_path(Path::new(path)).is_ok()
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
fn parse_semver(tag: &str) -> Option<SemverInfo> {
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

/// Check if a tag name is a semantic version
#[cfg(test)]
fn is_semver_tag(tag: &str) -> bool {
    parse_semver(tag).is_some()
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
    use chrono::{DateTime, TimeZone, Utc};

    let datetime: DateTime<Utc> = Utc.timestamp_opt(time.seconds(), 0).unwrap();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
