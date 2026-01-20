//! Query resolution logic.
//!
//! This module contains the orchestration layer that resolves user queries
//! to identified information using backend implementations. It also defines
//! the types for representing resolved information.

use crate::backend::Backend;
use crate::changelog;
use crate::error::{WtgError, WtgResult};
use crate::git::{CommitInfo, FileInfo, TagInfo};
use crate::github::{ExtendedIssueInfo, PullRequestInfo};
use crate::parse_input::Query;
use crate::release_filter::ReleaseFilter;

// ============================================
// Result types
// ============================================

/// What the user entered to search for
#[derive(Debug, Clone)]
pub enum EntryPoint {
    Commit(String),                            // Hash they entered
    IssueNumber(u64),                          // Issue # they entered
    PullRequestNumber(u64),                    // PR # they entered
    FilePath { branch: String, path: String }, // File path they entered
    Tag(String),                               // Tag they entered
}

/// Information about an Issue
#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: octocrab::models::IssueState,
    pub url: String,
    pub author: Option<String>,
    pub author_url: Option<String>,
}

impl From<&ExtendedIssueInfo> for IssueInfo {
    fn from(ext_info: &ExtendedIssueInfo) -> Self {
        Self {
            number: ext_info.number,
            title: ext_info.title.clone(),
            body: ext_info.body.clone(),
            state: ext_info.state.clone(),
            url: ext_info.url.clone(),
            author: ext_info.author.clone(),
            author_url: ext_info.author_url.clone(),
        }
    }
}

/// The enriched result of identification - progressively accumulates data
#[derive(Debug, Clone)]
pub struct EnrichedInfo {
    pub entry_point: EntryPoint,

    // Core - the commit (always present for complete results)
    pub commit: Option<CommitInfo>,

    // Enrichment Layer 1: PR (if this commit came from a PR)
    pub pr: Option<PullRequestInfo>,

    // Enrichment Layer 2: Issue (if this PR was fixing an issue)
    pub issue: Option<IssueInfo>,

    // Metadata
    pub release: Option<TagInfo>,
}

/// For file results (special case with blame history)
#[derive(Debug, Clone)]
pub struct FileResult {
    pub file_info: FileInfo,
    pub commit_url: Option<String>,
    pub author_urls: Vec<Option<String>>,
    pub release: Option<TagInfo>,
}

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

#[derive(Debug, Clone)]
pub enum IdentifiedThing {
    Enriched(Box<EnrichedInfo>),
    File(Box<FileResult>),
    Tag(Box<TagResult>),
}

// ============================================
// Resolution logic
// ============================================

/// Resolve a query to identified information using the provided backend.
///
/// The `filter` parameter controls which releases are considered when finding
/// the release that contains a commit.
pub async fn resolve(
    backend: &dyn Backend,
    query: &Query,
    filter: &ReleaseFilter,
) -> WtgResult<IdentifiedThing> {
    // Validate specific tag exists before doing any work
    if let Some(tag_name) = filter.specific_tag()
        && backend.find_tag(tag_name).await.is_err()
    {
        return Err(WtgError::TagNotFound(tag_name.to_string()));
    }

    match query {
        Query::GitCommit(hash) => resolve_commit(backend, hash, filter).await,
        Query::Pr(number) => resolve_pr(backend, *number, filter).await,
        Query::Issue(number) => resolve_issue(backend, *number, filter).await,
        Query::IssueOrPr(number) => {
            // Try PR first, then issue
            if let Ok(result) = resolve_pr(backend, *number, filter).await {
                return Ok(result);
            }
            if let Ok(result) = resolve_issue(backend, *number, filter).await {
                return Ok(result);
            }
            Err(WtgError::NotFound(format!("#{number}")))
        }
        Query::FilePath { branch, path } => {
            resolve_file(backend, branch, &path.to_string_lossy(), filter).await
        }
        Query::Tag(tag) => resolve_tag(backend, tag).await,
    }
}

/// Resolve a commit hash to `IdentifiedThing`.
async fn resolve_commit(
    backend: &dyn Backend,
    hash: &str,
    filter: &ReleaseFilter,
) -> WtgResult<IdentifiedThing> {
    let commit = backend.find_commit(hash).await?;
    let commit = backend.enrich_commit(commit).await;
    let release = backend
        .find_release_for_commit(&commit.hash, Some(commit.date), filter)
        .await;

    Ok(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point: EntryPoint::Commit(hash.to_string()),
        commit: Some(commit),
        pr: None,
        issue: None,
        release,
    })))
}

/// Resolve a PR number to `IdentifiedThing`.
async fn resolve_pr(
    backend: &dyn Backend,
    number: u64,
    filter: &ReleaseFilter,
) -> WtgResult<IdentifiedThing> {
    let pr = backend.fetch_pr(number).await?;

    let commit = backend.find_commit_for_pr(&pr).await.ok();
    let commit = match commit {
        Some(c) => Some(backend.enrich_commit(c).await),
        None => None,
    };

    let release = if let Some(ref c) = commit {
        backend
            .find_release_for_commit(&c.hash, Some(c.date), filter)
            .await
    } else {
        None
    };

    Ok(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point: EntryPoint::PullRequestNumber(number),
        commit,
        pr: Some(pr),
        issue: None,
        release,
    })))
}

/// Resolve an issue number to `IdentifiedThing`.
///
/// Handles cross-project PRs by spawning a backend for the PR's repository.
async fn resolve_issue(
    backend: &dyn Backend,
    number: u64,
    filter: &ReleaseFilter,
) -> WtgResult<IdentifiedThing> {
    let ext_issue = backend.fetch_issue(number).await?;
    let display_issue = (&ext_issue).into();

    // Try to find closing PR info
    let closing_pr = ext_issue.closing_prs.into_iter().next();

    let (commit, release) = if let Some(ref pr) = closing_pr {
        if let Some(merge_sha) = &pr.merge_commit_sha {
            // Get backend for PR (returns cross-project backend if needed, None if same repo)
            let cross_backend = backend.backend_for_pr(pr).await;
            let effective_backend: &dyn Backend =
                cross_backend.as_ref().map_or(backend, |b| b.as_ref());

            let commit = effective_backend.find_commit(merge_sha).await.ok();
            let commit = match commit {
                Some(c) => Some(effective_backend.enrich_commit(c).await),
                None => None,
            };

            let release = if let Some(ref c) = commit {
                let hash = &c.hash;
                let date = Some(c.date);
                // Try issue's repo first, fall back to PR's repo for releases
                if cross_backend.is_some() {
                    match backend.find_release_for_commit(hash, date, filter).await {
                        Some(r) => Some(r),
                        None => {
                            effective_backend
                                .find_release_for_commit(hash, date, filter)
                                .await
                        }
                    }
                } else {
                    backend.find_release_for_commit(hash, date, filter).await
                }
            } else {
                None
            };

            (commit, release)
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    Ok(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point: EntryPoint::IssueNumber(number),
        commit,
        pr: closing_pr,
        issue: Some(display_issue),
        release,
    })))
}

/// Resolve a file path to `IdentifiedThing`.
async fn resolve_file(
    backend: &dyn Backend,
    branch: &str,
    path: &str,
    filter: &ReleaseFilter,
) -> WtgResult<IdentifiedThing> {
    let file_info = backend.find_file(branch, path).await?;
    let commit_url = backend.commit_url(&file_info.last_commit.hash);

    // Generate author URLs from emails
    let author_urls: Vec<Option<String>> = file_info
        .previous_authors
        .iter()
        .map(|(_, _, email)| backend.author_url_from_email(email))
        .collect();

    let release = backend
        .find_release_for_commit(
            &file_info.last_commit.hash,
            Some(file_info.last_commit.date),
            filter,
        )
        .await;

    Ok(IdentifiedThing::File(Box::new(FileResult {
        file_info,
        commit_url,
        author_urls,
        release,
    })))
}

/// Select the best changes source, falling back to commits if needed.
async fn select_best_changes(
    backend: &dyn Backend,
    tag_name: &str,
    release_body: Option<&str>,
    changelog_content: Option<&str>,
) -> (
    Option<String>,
    Option<ChangesSource>,
    usize,
    Vec<CommitInfo>,
) {
    // Select the best source: prefer longer content, ties go to release
    let best_source = match (release_body, changelog_content) {
        // Both available: prefer the longer one, tie goes to release
        (Some(release), Some(changelog)) => {
            if release.trim().len() >= changelog.trim().len() {
                Some((release.to_string(), ChangesSource::GitHubRelease))
            } else {
                Some((changelog.to_string(), ChangesSource::Changelog))
            }
        }
        // Only release available
        (Some(release), None) if !release.trim().is_empty() => {
            Some((release.to_string(), ChangesSource::GitHubRelease))
        }
        // Only changelog available
        (None, Some(changelog)) if !changelog.trim().is_empty() => {
            Some((changelog.to_string(), ChangesSource::Changelog))
        }
        // Neither available or both empty
        _ => None,
    };

    if let Some((content, source)) = best_source {
        let (truncated_content, remaining) = changelog::truncate_content(&content);
        return (
            Some(truncated_content.to_string()),
            Some(source),
            remaining,
            Vec::new(),
        );
    }

    // Fall back to commits
    if let Ok(Some(prev_tag)) = backend.find_previous_tag(tag_name).await
        && let Ok(commits) = backend
            .commits_between_tags(&prev_tag.name, tag_name, 5)
            .await
        && !commits.is_empty()
    {
        return (
            None,
            Some(ChangesSource::Commits {
                previous_tag: prev_tag.name,
            }),
            0,
            commits,
        );
    }

    // No changes available
    (None, None, 0, Vec::new())
}

/// Resolve a tag name to `IdentifiedThing`.
async fn resolve_tag(backend: &dyn Backend, name: &str) -> WtgResult<IdentifiedThing> {
    let tag = backend.find_tag(name).await?;

    // Try to get changelog section via backend
    let changelog_content = backend.changelog_for_version(name).await;

    // Try to get release body from GitHub (only if it's a release)
    let release_body = if tag.is_release {
        backend.fetch_release_body(name).await
    } else {
        None
    };

    // Determine best source and get commits if needed
    let (changes, source, truncated, commits) = select_best_changes(
        backend,
        name,
        release_body.as_deref(),
        changelog_content.as_deref(),
    )
    .await;

    Ok(IdentifiedThing::Tag(Box::new(TagResult {
        tag_info: tag.clone(),
        github_url: tag.tag_url,
        changes,
        changes_source: source,
        truncated_lines: truncated,
        commits,
    })))
}
