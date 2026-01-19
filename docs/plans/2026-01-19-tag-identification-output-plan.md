# Tag Identification Output Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the placeholder "cowboy" message with meaningful tag output showing metadata and changes from the best available source (GitHub release, CHANGELOG, or commit diff).

**Architecture:** New `changelog` module for parsing. New Backend trait methods for `find_previous_tag` and `commits_between_tags`. Resolution gathers data sources, output renders with selection logic. 20-line truncation applied at display time.

**Tech Stack:** Rust, regex for CHANGELOG parsing, existing Backend trait pattern, existing TagInfo/CommitInfo types.

---

## Task 1: Add `body` field to ReleaseInfo

**Files:**
- Modify: `crates/wtg/src/github.rs:196-204` (ReleaseInfo struct)
- Modify: `crates/wtg/src/github.rs:528-536` (fetch_releases_since conversion)
- Modify: `crates/wtg/src/github.rs:586-593` (fetch_release_by_tag conversion)

**Step 1: Add body field to ReleaseInfo**

```rust
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,  // Add this field
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub prerelease: bool,
}
```

**Step 2: Update fetch_releases_since conversion**

In the loop where `ReleaseInfo` is created (~line 528-536):

```rust
releases.push(ReleaseInfo {
    tag_name: release.tag_name,
    name: release.name,
    body: release.body,  // Add this line
    url: release.html_url.to_string(),
    published_at: release.published_at,
    created_at: release.created_at,
    prerelease: release.prerelease,
});
```

**Step 3: Update fetch_release_by_tag conversion**

In `fetch_release_by_tag` (~line 586-593):

```rust
Some(ReleaseInfo {
    tag_name: release.tag_name,
    name: release.name,
    body: release.body,  // Add this line
    url: release.html_url.to_string(),
    published_at: release.published_at,
    created_at: release.created_at,
    prerelease: release.prerelease,
})
```

**Step 4: Run checks**

Run: `just fmt && just lint`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/wtg/src/github.rs
git commit -m "Add body field to ReleaseInfo for release descriptions"
```

---

## Task 2: Create changelog module with parsing logic

**Files:**
- Create: `crates/wtg/src/changelog.rs`
- Modify: `crates/wtg/src/lib.rs` (add `mod changelog;`)

**Step 1: Write the changelog module with tests**

Create `crates/wtg/src/changelog.rs`:

```rust
//! CHANGELOG.md parsing for Keep a Changelog format.
//!
//! Supports strict Keep a Changelog format with `## [version]` headers.
//! See <https://keepachangelog.com> for format specification.

use std::fs;
use std::path::Path;

use regex::Regex;

/// Maximum number of lines to include in changelog output before truncation.
pub const MAX_LINES: usize = 20;

/// Extract the changelog section for a specific version.
///
/// Looks for CHANGELOG.md (case-insensitive) at the given path and extracts
/// the section matching the version. Returns None if file doesn't exist,
/// version not found, or format is invalid.
///
/// # Arguments
/// * `repo_root` - Path to the repository root
/// * `version` - Version to find (with or without 'v' prefix)
///
/// # Returns
/// The changelog section content, or None if not found.
pub fn parse_changelog_for_version(repo_root: &Path, version: &str) -> Option<String> {
    let changelog_path = find_changelog_file(repo_root)?;
    let content = fs::read_to_string(changelog_path).ok()?;
    extract_version_section(&content, version)
}

/// Find CHANGELOG.md file (case-insensitive) at repo root.
fn find_changelog_file(repo_root: &Path) -> Option<std::path::PathBuf> {
    let entries = fs::read_dir(repo_root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("changelog.md") {
            return Some(entry.path());
        }
    }
    None
}

/// Extract a version section from changelog content.
///
/// Matches Keep a Changelog format: `## [version]` or `## [version] - date`
/// Version matching is flexible: strips 'v' prefix from both sides for comparison.
fn extract_version_section(content: &str, version: &str) -> Option<String> {
    // Normalize version by stripping 'v' prefix
    let normalized_version = version.strip_prefix('v').unwrap_or(version);

    // Pattern: ## [version] or ## [vVersion] optionally followed by - date
    // Captures the version inside brackets
    let header_pattern = Regex::new(r"(?m)^## \[v?([^\]]+)\]").ok()?;

    let mut section_start: Option<usize> = None;
    let mut section_end: Option<usize> = None;

    for caps in header_pattern.captures_iter(content) {
        let full_match = caps.get(0)?;
        let captured_version = caps.get(1)?.as_str();

        // Normalize captured version too
        let normalized_captured = captured_version.strip_prefix('v').unwrap_or(captured_version);

        if section_start.is_some() {
            // We found the next section header, mark end
            section_end = Some(full_match.start());
            break;
        }

        if normalized_captured == normalized_version {
            // Found our version, start after the header line
            let line_end = content[full_match.end()..].find('\n')
                .map(|i| full_match.end() + i + 1)
                .unwrap_or(full_match.end());
            section_start = Some(line_end);
        }
    }

    let start = section_start?;
    let end = section_end.unwrap_or(content.len());

    let section = content[start..end].trim();
    if section.is_empty() {
        return None;
    }

    Some(section.to_string())
}

/// Truncate content to MAX_LINES, returning (content, remaining_lines).
///
/// If content exceeds MAX_LINES, returns truncated content and count of remaining lines.
/// Otherwise returns original content and 0.
pub fn truncate_content(content: &str) -> (&str, usize) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= MAX_LINES {
        return (content, 0);
    }

    let truncated_end = lines[..MAX_LINES]
        .iter()
        .map(|l| l.len() + 1) // +1 for newline
        .sum::<usize>();

    // Find the actual byte position (handle last line without newline)
    let truncated_end = truncated_end.min(content.len());
    let truncated = &content[..truncated_end].trim_end();

    (truncated, lines.len() - MAX_LINES)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CHANGELOG: &str = r#"# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Something new

## [1.2.0] - 2024-01-15

### Added
- Feature X
- Feature Y

### Fixed
- Bug in authentication

## [1.1.0] - 2024-01-01

### Added
- Initial feature
"#;

    #[test]
    fn extracts_version_section() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "1.2.0");
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("Feature X"));
        assert!(content.contains("Bug in authentication"));
        assert!(!content.contains("Initial feature"));
    }

    #[test]
    fn extracts_version_with_v_prefix() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "v1.2.0");
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("Feature X"));
    }

    #[test]
    fn handles_changelog_with_v_prefix_in_header() {
        let changelog = r#"# Changelog

## [v2.0.0] - 2024-02-01

### Changed
- Major update
"#;
        let result = extract_version_section(changelog, "2.0.0");
        assert!(result.is_some());
        assert!(result.unwrap().contains("Major update"));

        let result2 = extract_version_section(changelog, "v2.0.0");
        assert!(result2.is_some());
    }

    #[test]
    fn returns_none_for_missing_version() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "9.9.9");
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_unreleased() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "Unreleased");
        assert!(result.is_some()); // It exists but content is minimal
    }

    #[test]
    fn returns_none_for_empty_section() {
        let changelog = r#"# Changelog

## [1.0.0]

## [0.9.0]

### Added
- Something
"#;
        let result = extract_version_section(changelog, "1.0.0");
        assert!(result.is_none());
    }

    #[test]
    fn truncates_long_content() {
        let long_content = (0..30).map(|i| format!("Line {i}")).collect::<Vec<_>>().join("\n");
        let (truncated, remaining) = truncate_content(&long_content);

        assert_eq!(remaining, 10);
        assert!(truncated.lines().count() <= MAX_LINES);
        assert!(truncated.contains("Line 0"));
        assert!(truncated.contains("Line 19"));
        assert!(!truncated.contains("Line 20"));
    }

    #[test]
    fn does_not_truncate_short_content() {
        let short_content = "Line 1\nLine 2\nLine 3";
        let (result, remaining) = truncate_content(short_content);

        assert_eq!(remaining, 0);
        assert_eq!(result, short_content);
    }
}
```

**Step 2: Add module to lib.rs**

In `crates/wtg/src/lib.rs`, add after line 16 (after `pub mod git;`):

```rust
pub mod changelog;
```

**Step 3: Add regex dependency**

Run: `cargo add regex --package wtg-cli`

**Step 4: Run tests**

Run: `just test`
Expected: PASS (including new changelog tests)

**Step 5: Commit**

```bash
git add crates/wtg/src/changelog.rs crates/wtg/src/lib.rs crates/wtg/Cargo.toml Cargo.lock
git commit -m "Add changelog module for Keep a Changelog parsing"
```

---

## Task 3: Add `find_previous_tag` to Backend trait

**Files:**
- Modify: `crates/wtg/src/backend/mod.rs` (add trait method)
- Modify: `crates/wtg/src/backend/git_backend.rs` (implement)
- Modify: `crates/wtg/src/backend/github_backend.rs` (implement)
- Modify: `crates/wtg/src/backend/combined_backend.rs` (delegate)

**Step 1: Add trait method to Backend**

In `crates/wtg/src/backend/mod.rs`, add after `find_tag` method (~line 84):

```rust
    /// Find the previous tag before the given tag.
    ///
    /// For semver tags, returns the immediately preceding version by semver ordering.
    /// For non-semver tags, returns the most recent tag pointing to an earlier commit.
    async fn find_previous_tag(&self, _tag_name: &str) -> WtgResult<Option<TagInfo>> {
        Err(WtgError::Unsupported("find previous tag".into()))
    }
```

**Step 2: Implement in GitBackend**

In `crates/wtg/src/backend/git_backend.rs`, add after `find_tag` implementation (~line 173):

```rust
    async fn find_previous_tag(&self, tag_name: &str) -> WtgResult<Option<TagInfo>> {
        let tags = self.repo.get_tags();
        let current_tag = tags.iter().find(|t| t.name == tag_name);

        let Some(current) = current_tag else {
            return Ok(None);
        };

        // If current is semver, find previous by semver ordering
        if current.is_semver() {
            let mut semver_tags: Vec<_> = tags.iter()
                .filter(|t| t.is_semver())
                .collect();

            // Sort by semver (ascending)
            semver_tags.sort_by(|a, b| {
                let a_semver = a.semver_info.as_ref().unwrap();
                let b_semver = b.semver_info.as_ref().unwrap();
                a_semver.cmp(b_semver)
            });

            // Find current position and return previous
            if let Some(pos) = semver_tags.iter().position(|t| t.name == tag_name) {
                if pos > 0 {
                    return Ok(Some(semver_tags[pos - 1].clone()));
                }
            }
            return Ok(None);
        }

        // Non-semver: find most recent tag on an earlier commit
        let current_timestamp = self.repo.get_commit_timestamp(&current.commit_hash);

        let mut candidates: Vec<_> = tags.iter()
            .filter(|t| t.name != tag_name)
            .filter(|t| t.commit_hash != current.commit_hash)
            .filter(|t| self.repo.get_commit_timestamp(&t.commit_hash) < current_timestamp)
            .collect();

        // Sort by timestamp descending (most recent first)
        candidates.sort_by(|a, b| {
            let a_ts = self.repo.get_commit_timestamp(&a.commit_hash);
            let b_ts = self.repo.get_commit_timestamp(&b.commit_hash);
            b_ts.cmp(&a_ts)
        });

        Ok(candidates.first().map(|t| (*t).clone()))
    }
```

**Step 3: Implement in GitHubBackend**

In `crates/wtg/src/backend/github_backend.rs`, add after `find_tag` implementation (~line 147):

```rust
    async fn find_previous_tag(&self, tag_name: &str) -> WtgResult<Option<TagInfo>> {
        // Fetch the current tag first
        let current = self.find_tag(tag_name).await?;

        // For GitHub, we need to list releases/tags and find the previous one
        // This is a simplified implementation - fetch recent releases
        let since = current.created_at - chrono::Duration::days(365);
        let releases = self.client.fetch_releases_since(&self.gh_repo_info, since).await;

        if current.is_semver() {
            // Find previous by semver
            let mut semver_releases: Vec<_> = releases.iter()
                .filter(|r| crate::git::parse_semver(&r.tag_name).is_some())
                .collect();

            semver_releases.sort_by(|a, b| {
                let a_semver = crate::git::parse_semver(&a.tag_name).unwrap();
                let b_semver = crate::git::parse_semver(&b.tag_name).unwrap();
                a_semver.cmp(&b_semver)
            });

            if let Some(pos) = semver_releases.iter().position(|r| r.tag_name == tag_name) {
                if pos > 0 {
                    let prev = &semver_releases[pos - 1];
                    return self.find_tag(&prev.tag_name).await.map(Some);
                }
            }
            return Ok(None);
        }

        // Non-semver: find by date
        let mut candidates: Vec<_> = releases.iter()
            .filter(|r| r.tag_name != tag_name)
            .filter(|r| r.created_at.map(|d| d < current.created_at).unwrap_or(false))
            .collect();

        candidates.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        if let Some(prev) = candidates.first() {
            return self.find_tag(&prev.tag_name).await.map(Some);
        }

        Ok(None)
    }
```

**Step 4: Delegate in CombinedBackend**

In `crates/wtg/src/backend/combined_backend.rs`, add after `find_tag` (~line 308):

```rust
    async fn find_previous_tag(&self, tag_name: &str) -> WtgResult<Option<TagInfo>> {
        // Use git backend for local tag lookup (faster)
        self.git.find_previous_tag(tag_name).await
    }
```

**Step 5: Add Ord implementation to SemverInfo**

In `crates/wtg/src/semver.rs`, add after the struct definition:

```rust
impl Ord for SemverInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.major.cmp(&other.major) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.minor.cmp(&other.minor) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.patch.cmp(&other.patch) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        // Pre-release: None (stable) > Some (pre-release)
        match (&self.pre_release, &other.pre_release) {
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
            (None, None) => std::cmp::Ordering::Equal,
        }
    }
}

impl PartialOrd for SemverInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
```

**Step 6: Run checks**

Run: `just fmt && just lint && just test`
Expected: PASS

**Step 7: Commit**

```bash
git add crates/wtg/src/backend/ crates/wtg/src/semver.rs
git commit -m "Add find_previous_tag to Backend trait with semver ordering"
```

---

## Task 4: Add `commits_between_tags` to Backend trait

**Files:**
- Modify: `crates/wtg/src/backend/mod.rs` (add trait method)
- Modify: `crates/wtg/src/backend/git_backend.rs` (implement)
- Modify: `crates/wtg/src/backend/github_backend.rs` (implement)
- Modify: `crates/wtg/src/backend/combined_backend.rs` (delegate)
- Modify: `crates/wtg/src/git.rs` (add helper method to GitRepo)

**Step 1: Add trait method to Backend**

In `crates/wtg/src/backend/mod.rs`, add after `find_previous_tag`:

```rust
    /// Get commits between two tags (from_tag exclusive, to_tag inclusive).
    ///
    /// Returns up to `limit` commits, most recent first.
    async fn commits_between_tags(
        &self,
        _from_tag: &str,
        _to_tag: &str,
        _limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        Err(WtgError::Unsupported("commits between tags".into()))
    }
```

**Step 2: Add helper to GitRepo**

In `crates/wtg/src/git.rs`, add after `tags_containing_commit` method (~line 567):

```rust
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
                let Ok(commit) = repo.find_commit(oid) else { continue };
                result.push(Self::commit_to_info(&commit));
            }

            result
        })
    }
```

**Step 3: Implement in GitBackend**

In `crates/wtg/src/backend/git_backend.rs`:

```rust
    async fn commits_between_tags(
        &self,
        from_tag: &str,
        to_tag: &str,
        limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        Ok(self.repo.commits_between(from_tag, to_tag, limit))
    }
```

**Step 4: Implement in GitHubBackend**

In `crates/wtg/src/backend/github_backend.rs`:

```rust
    async fn commits_between_tags(
        &self,
        from_tag: &str,
        to_tag: &str,
        limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        // Use GitHub compare API
        let compare = self
            .call_client_api_with_fallback(move |client| {
                let from = from_tag.to_string();
                let to = to_tag.to_string();
                let repo_info = self.gh_repo_info.clone();
                Box::pin(async move {
                    client
                        .commits(repo_info.owner(), repo_info.repo())
                        .compare(&from, &to)
                        .per_page(limit.try_into().unwrap_or(100))
                        .send()
                        .await
                })
            })
            .await?;

        Ok(compare.commits
            .into_iter()
            .take(limit)
            .map(CommitInfo::from)
            .collect())
    }
```

Note: This requires adding a helper method or using the existing conversion. Check if RepoCommit is the right type.

**Step 5: Delegate in CombinedBackend**

In `crates/wtg/src/backend/combined_backend.rs`:

```rust
    async fn commits_between_tags(
        &self,
        from_tag: &str,
        to_tag: &str,
        limit: usize,
    ) -> WtgResult<Vec<CommitInfo>> {
        // Try git first, fall back to GitHub
        match self.git.commits_between_tags(from_tag, to_tag, limit).await {
            Ok(commits) if !commits.is_empty() => Ok(commits),
            _ => self.github.commits_between_tags(from_tag, to_tag, limit).await,
        }
    }
```

**Step 6: Run checks**

Run: `just fmt && just lint && just test`
Expected: PASS

**Step 7: Commit**

```bash
git add crates/wtg/src/backend/ crates/wtg/src/git.rs
git commit -m "Add commits_between_tags to Backend trait"
```

---

## Task 5: Create enriched TagResult type

**Files:**
- Modify: `crates/wtg/src/resolution.rs` (add TagResult, modify IdentifiedThing)

**Step 1: Add TagResult struct and ChangesSource enum**

In `crates/wtg/src/resolution.rs`, add after `FileResult` struct (~line 78):

```rust
/// Source of changes information for a tag
#[derive(Debug, Clone)]
pub enum ChangesSource {
    /// From GitHub release description
    GitHubRelease,
    /// From CHANGELOG.md
    Changelog,
    /// From commits since previous tag
    Commits { previous_tag: String },
}

/// Enriched tag result with changes information
#[derive(Debug, Clone)]
pub struct TagResult {
    pub tag_info: TagInfo,
    pub github_url: Option<String>,
    /// Changes content (release notes, changelog section, or commit list)
    pub changes: Option<String>,
    /// Where the changes came from
    pub changes_source: Option<ChangesSource>,
    /// Number of lines truncated (0 if not truncated)
    pub truncated_lines: usize,
    /// Commits between this tag and previous (when source is Commits)
    pub commits: Vec<CommitInfo>,
}
```

**Step 2: Update IdentifiedThing enum**

Change the `TagOnly` variant:

```rust
#[derive(Debug, Clone)]
pub enum IdentifiedThing {
    Enriched(Box<EnrichedInfo>),
    File(Box<FileResult>),
    Tag(Box<TagResult>),  // Changed from TagOnly
}
```

**Step 3: Run checks (will fail - need to update usages)**

Run: `just check`
Expected: FAIL (compilation errors for TagOnly usages)

**Step 4: Commit partial progress**

```bash
git add crates/wtg/src/resolution.rs
git commit -m "Add TagResult type for enriched tag output (WIP - breaks compilation)"
```

---

## Task 6: Update resolution to gather tag data

**Files:**
- Modify: `crates/wtg/src/resolution.rs` (update resolve_tag function)

**Step 1: Update resolve_tag function**

Replace the existing `resolve_tag` function (~line 244-248):

```rust
/// Resolve a tag name to `IdentifiedThing`.
async fn resolve_tag(backend: &dyn Backend, name: &str) -> WtgResult<IdentifiedThing> {
    use crate::changelog;

    let tag = backend.find_tag(name).await?;
    let url = backend.tag_url(name);

    // Try to get release info with body
    let release_body = if tag.is_release {
        // Tag is a GitHub release - body should be available via API
        // For now, we don't have direct access to body through TagInfo
        // This will be enhanced when we add release body fetching
        None
    } else {
        None
    };

    // Try to get changelog section
    let changelog_content = if let Some(repo_path) = get_repo_path(backend) {
        changelog::parse_changelog_for_version(&repo_path, name)
    } else {
        None
    };

    // Determine best source and get commits if needed
    let (changes, source, truncated, commits) = select_best_changes(
        backend,
        name,
        release_body.as_deref(),
        changelog_content.as_deref(),
    ).await;

    Ok(IdentifiedThing::Tag(Box::new(TagResult {
        tag_info: tag,
        github_url: url,
        changes,
        changes_source: source,
        truncated_lines: truncated,
        commits,
    })))
}

/// Get repository path from backend (if available).
fn get_repo_path(backend: &dyn Backend) -> Option<std::path::PathBuf> {
    // For now, try current directory. In future, backend could expose this.
    std::env::current_dir().ok()
}

/// Select the best changes source, falling back to commits if needed.
async fn select_best_changes(
    backend: &dyn Backend,
    tag_name: &str,
    release_body: Option<&str>,
    changelog_content: Option<&str>,
) -> (Option<String>, Option<ChangesSource>, usize, Vec<CommitInfo>) {
    use crate::changelog;

    // Compare release body and changelog, pick the more substantial one
    let release_len = release_body.map(|s| s.trim().len()).unwrap_or(0);
    let changelog_len = changelog_content.map(|s| s.trim().len()).unwrap_or(0);

    if release_len > 0 || changelog_len > 0 {
        let (content, source) = if release_len >= changelog_len && release_len > 0 {
            (release_body.unwrap().to_string(), ChangesSource::GitHubRelease)
        } else {
            (changelog_content.unwrap().to_string(), ChangesSource::Changelog)
        };

        let (truncated_content, remaining) = changelog::truncate_content(&content);
        return (
            Some(truncated_content.to_string()),
            Some(source),
            remaining,
            Vec::new(),
        );
    }

    // Fall back to commits
    if let Ok(Some(prev_tag)) = backend.find_previous_tag(tag_name).await {
        if let Ok(commits) = backend.commits_between_tags(&prev_tag.name, tag_name, 5).await {
            if !commits.is_empty() {
                return (
                    None,
                    Some(ChangesSource::Commits { previous_tag: prev_tag.name }),
                    0,
                    commits,
                );
            }
        }
    }

    // No changes available
    (None, None, 0, Vec::new())
}
```

**Step 2: Run checks**

Run: `just check`
Expected: Still FAIL (output.rs needs updating)

**Step 3: Commit**

```bash
git add crates/wtg/src/resolution.rs
git commit -m "Update resolve_tag to gather changes from multiple sources"
```

---

## Task 7: Update output display for tags

**Files:**
- Modify: `crates/wtg/src/output.rs`

**Step 1: Update display function**

Change the match arm for `TagOnly` to `Tag`:

```rust
pub fn display(thing: IdentifiedThing) -> WtgResult<()> {
    match thing {
        IdentifiedThing::Enriched(info) => display_enriched(*info),
        IdentifiedThing::File(file_result) => display_file(*file_result),
        IdentifiedThing::Tag(tag_result) => display_tag(*tag_result),
    }

    Ok(())
}
```

**Step 2: Replace display_tag_warning with display_tag**

Remove the old `display_tag_warning` function and add:

```rust
/// Display tag information with changes from best available source
fn display_tag(result: TagResult) {
    let tag = &result.tag_info;

    // Header
    println!(
        "{} {}",
        "🏷️  Tag:".green().bold(),
        tag.name.as_str().cyan()
    );
    println!(
        "{} {}",
        "📅 Created:".yellow(),
        tag.created_at.format("%Y-%m-%d").to_string().dark_grey()
    );

    // URL - prefer release URL if available
    if let Some(url) = tag.release_url.as_ref().or(result.github_url.as_ref()) {
        println!("{} {}", "🔗 Release:".blue(), url.blue().underlined());
    }

    println!();

    // Changes section
    if let Some(source) = &result.changes_source {
        let source_label = match source {
            ChangesSource::GitHubRelease => "(from GitHub release)".to_string(),
            ChangesSource::Changelog => "(from CHANGELOG)".to_string(),
            ChangesSource::Commits { previous_tag } => {
                format!("(commits since {})", previous_tag)
            }
        };

        println!(
            "{} {}",
            "Changes".magenta().bold(),
            source_label.dark_grey()
        );

        match source {
            ChangesSource::Commits { .. } => {
                // Display commits as bullet list
                for commit in &result.commits {
                    println!(
                        "• {} {}",
                        commit.short_hash.as_str().cyan(),
                        commit.message.as_str().white()
                    );
                }
            }
            _ => {
                // Display text content
                if let Some(content) = &result.changes {
                    for line in content.lines() {
                        println!("{}", line);
                    }
                }
            }
        }

        // Truncation notice
        if result.truncated_lines > 0 {
            if let Some(url) = tag.release_url.as_ref().or(result.github_url.as_ref()) {
                println!(
                    "{}",
                    format!("... {} more lines (see full release at {})", result.truncated_lines, url)
                        .dark_grey()
                        .italic()
                );
            } else {
                println!(
                    "{}",
                    format!("... {} more lines", result.truncated_lines)
                        .dark_grey()
                        .italic()
                );
            }
        }
    } else {
        // No changes available - just show the tag exists
        println!(
            "{}",
            "No release notes, changelog entry, or previous tag found."
                .dark_grey()
                .italic()
        );
    }
}
```

**Step 3: Add import for ChangesSource**

At the top of output.rs, update the import:

```rust
use crate::resolution::{EnrichedInfo, EntryPoint, FileResult, IdentifiedThing, IssueInfo, TagResult, ChangesSource};
```

**Step 4: Run checks**

Run: `just fmt && just lint && just check`
Expected: PASS (or close to it)

**Step 5: Commit**

```bash
git add crates/wtg/src/output.rs
git commit -m "Update output display for enriched tag results"
```

---

## Task 8: Update tests

**Files:**
- Modify: `crates/wtg/tests/offline.rs`
- Modify: `crates/wtg/tests/integration.rs`
- Update: `crates/wtg/tests/snapshots/integration__integration_identify_tag.snap`

**Step 1: Update offline test for tag identification**

In `crates/wtg/tests/offline.rs`, update `test_identify_tag`:

```rust
/// Test identifying a tag
#[rstest]
#[tokio::test]
async fn test_identify_tag(test_repo: TestRepoFixture) {
    let backend = GitBackend::new(test_repo.repo);
    let query = backend
        .disambiguate_query(&ParsedQuery::Unknown("v1.0.0".to_string()))
        .await
        .expect("Failed to disambiguate tag");

    let result = resolve(&backend, &query)
        .await
        .expect("Failed to identify tag");

    // Verify it's a tag result
    match result {
        IdentifiedThing::Tag(tag_result) => {
            assert_eq!(tag_result.tag_info.name, "v1.0.0");
            assert_eq!(tag_result.tag_info.commit_hash, test_repo.commits.commit1_add_file);
            assert!(tag_result.tag_info.is_semver());

            let semver = tag_result.tag_info.semver_info.expect("Should have semver info");
            assert_eq!(semver.major, 1);
            assert_eq!(semver.minor, 0);
            assert_eq!(semver.patch, Some(0));
        }
        _ => panic!("Expected Tag result, got something else"),
    }
}
```

**Step 2: Update integration test snapshot helper**

In `crates/wtg/tests/integration.rs`, update `to_snapshot`:

```rust
IdentifiedThing::Tag(tag_result) => IntegrationSnapshot {
    result_type: "tag".to_string(),
    entry_point: None,
    commit_message: None,
    commit_author: None,
    has_commit_url: tag_result.github_url.is_some(),
    has_pr: false,
    has_issue: false,
    release_name: if tag_result.tag_info.is_release {
        Some(tag_result.tag_info.name.clone())
    } else {
        None
    },
    release_is_semver: Some(tag_result.tag_info.is_semver()),
    tag_name: Some(tag_result.tag_info.name.clone()),
    file_path: None,
    previous_authors_count: None,
},
```

**Step 3: Run tests and update snapshot**

Run: `just test`
Run: `just test-integration`

If snapshot changed, review and accept:
Run: `cargo insta review`

**Step 4: Commit**

```bash
git add crates/wtg/tests/
git commit -m "Update tests for enriched tag output"
```

---

## Task 9: Add release body fetching to resolution

**Files:**
- Modify: `crates/wtg/src/backend/mod.rs` (add fetch_release_body method)
- Modify: `crates/wtg/src/backend/github_backend.rs` (implement)
- Modify: `crates/wtg/src/backend/combined_backend.rs` (delegate)
- Modify: `crates/wtg/src/resolution.rs` (use fetch_release_body)

**Step 1: Add trait method**

In `crates/wtg/src/backend/mod.rs`:

```rust
    /// Fetch the body/description of a GitHub release by tag name.
    async fn fetch_release_body(&self, _tag_name: &str) -> Option<String> {
        None
    }
```

**Step 2: Implement in GitHubBackend**

```rust
    async fn fetch_release_body(&self, tag_name: &str) -> Option<String> {
        let release = self.client.fetch_release_by_tag(&self.gh_repo_info, tag_name).await?;
        release.body.filter(|b| !b.trim().is_empty())
    }
```

**Step 3: Delegate in CombinedBackend**

```rust
    async fn fetch_release_body(&self, tag_name: &str) -> Option<String> {
        self.github.fetch_release_body(tag_name).await
    }
```

**Step 4: Update resolve_tag to use it**

In `resolve_tag`, replace the release_body assignment:

```rust
    // Try to get release body from GitHub
    let release_body = backend.fetch_release_body(name).await;
```

**Step 5: Run full test suite**

Run: `just fmt && just lint && just test && just test-integration`

**Step 6: Commit**

```bash
git add crates/wtg/src/
git commit -m "Add fetch_release_body to Backend for GitHub release descriptions"
```

---

## Task 10: Final verification and cleanup

**Step 1: Run full CI locally**

Run: `just ci`
Expected: PASS

**Step 2: Test manually**

```bash
cargo run -- v0.1.0
cargo run -- v0.2.0
```

Verify output shows:
- Tag name, created date, release URL
- Changes section with source indicator
- Truncation notice if applicable

**Step 3: Commit any final fixes**

**Step 4: Final commit message**

```bash
git log --oneline -10
```

Review the commits made during implementation.
