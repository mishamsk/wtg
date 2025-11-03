use crate::error::{Result, WtgError};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo};
use crate::github::{GitHubClient, IssueInfo, PullRequestInfo, ReleaseInfo};
use std::collections::HashMap;
use std::sync::Arc;

/// What the user entered to search for
#[derive(Debug, Clone)]
pub enum EntryPoint {
    Commit(String),         // Hash they entered
    IssueNumber(u64),       // Issue # they entered
    PullRequestNumber(u64), // PR # they entered
    FilePath(String),       // File path they entered
    Tag(String),            // Tag they entered
}

/// The enriched result of identification - progressively accumulates data
#[derive(Debug, Clone)]
pub struct EnrichedInfo {
    pub entry_point: EntryPoint,

    // Core - the commit (always present for complete results)
    pub commit: Option<CommitInfo>,
    pub commit_url: Option<String>,
    pub commit_author_github_url: Option<String>,

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

#[derive(Debug, Clone)]
pub enum IdentifiedThing {
    Enriched(Box<EnrichedInfo>),
    File(Box<FileResult>),
    TagOnly(Box<TagInfo>, Option<String>), // Just a tag, no commit yet
}

pub async fn identify(input: &str, git: GitRepo) -> Result<IdentifiedThing> {
    let github = git
        .github_remote()
        .map(|(owner, repo)| Arc::new(GitHubClient::new(owner, repo)));

    // Try as commit hash first
    if let Some(commit_info) = git.find_commit(input) {
        return Ok(resolve_commit(
            EntryPoint::Commit(input.to_string()),
            commit_info,
            &git,
            github.as_deref(),
        )
        .await);
    }

    // Try as issue/PR number (if it's all digits or starts with #)
    let number_str = input.strip_prefix('#').unwrap_or(input);
    if let Ok(number) = number_str.parse::<u64>()
        && let Some(result) = Box::pin(resolve_number(number, &git, github.as_deref())).await
    {
        return Ok(result);
    }

    // Try as file path
    if let Some(file_info) = git.find_file(input) {
        return Ok(resolve_file(file_info, &git, github.as_deref()).await);
    }

    // Try as tag
    let tags = git.get_tags();
    if let Some(tag_info) = tags.iter().find(|t| t.name == input) {
        let github_url = github.as_deref().map(|gh| gh.tag_url(&tag_info.name));
        return Ok(IdentifiedThing::TagOnly(
            Box::new(tag_info.clone()),
            github_url,
        ));
    }

    // Nothing found
    Err(WtgError::NotFound(input.to_string()))
}

/// Resolve a commit to enriched info
async fn resolve_commit(
    entry_point: EntryPoint,
    commit_info: CommitInfo,
    git: &GitRepo,
    github: Option<&GitHubClient>,
) -> IdentifiedThing {
    let (commit_url, commit_author_github_url) =
        resolve_commit_urls(github, &commit_info.author_email, &commit_info.hash).await;

    let commit_date = commit_info.date_rfc3339();
    let release =
        resolve_release_for_commit(git, github, &commit_info.hash, Some(&commit_date)).await;

    IdentifiedThing::Enriched(Box::new(EnrichedInfo {
        entry_point,
        commit: Some(commit_info),
        commit_url,
        commit_author_github_url,
        pr: None,
        issue: None,
        release,
    }))
}

/// Resolve an issue/PR number
async fn resolve_number(
    number: u64,
    git: &GitRepo,
    github: Option<&GitHubClient>,
) -> Option<IdentifiedThing> {
    let gh = github?;

    if let Some(pr_info) = gh.fetch_pr(number).await {
        if let Some(merge_sha) = &pr_info.merge_commit_sha
            && let Some(commit_info) = git.find_commit(merge_sha)
        {
            let (commit_url, commit_author_github_url) =
                resolve_commit_urls(Some(gh), &commit_info.author_email, &commit_info.hash).await;

            let commit_date = commit_info.date_rfc3339();
            let release =
                resolve_release_for_commit(git, Some(gh), merge_sha, Some(&commit_date)).await;

            return Some(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
                entry_point: EntryPoint::PullRequestNumber(number),
                commit: Some(commit_info),
                commit_url,
                commit_author_github_url,
                pr: Some(pr_info),
                issue: None,
                release,
            })));
        }

        return Some(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
            entry_point: EntryPoint::PullRequestNumber(number),
            commit: None,
            commit_url: None,
            commit_author_github_url: None,
            pr: Some(pr_info),
            issue: None,
            release: None,
        })));
    }

    if let Some(issue_info) = gh.fetch_issue(number).await {
        if let Some(&first_pr_number) = issue_info.closing_prs.first()
            && let Some(pr_info) = gh.fetch_pr(first_pr_number).await
        {
            if let Some(merge_sha) = &pr_info.merge_commit_sha
                && let Some(commit_info) = git.find_commit(merge_sha)
            {
                let (commit_url, commit_author_github_url) =
                    resolve_commit_urls(Some(gh), &commit_info.author_email, &commit_info.hash)
                        .await;

                let commit_date = commit_info.date_rfc3339();
                let release =
                    resolve_release_for_commit(git, Some(gh), merge_sha, Some(&commit_date)).await;

                return Some(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
                    entry_point: EntryPoint::IssueNumber(number),
                    commit: Some(commit_info),
                    commit_url,
                    commit_author_github_url,
                    pr: Some(pr_info),
                    issue: Some(issue_info),
                    release,
                })));
            }

            return Some(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
                entry_point: EntryPoint::IssueNumber(number),
                commit: None,
                commit_url: None,
                commit_author_github_url: None,
                pr: Some(pr_info),
                issue: Some(issue_info),
                release: None,
            })));
        }

        return Some(IdentifiedThing::Enriched(Box::new(EnrichedInfo {
            entry_point: EntryPoint::IssueNumber(number),
            commit: None,
            commit_url: None,
            commit_author_github_url: None,
            pr: None,
            issue: Some(issue_info),
            release: None,
        })));
    }

    None
}

async fn resolve_release_for_commit(
    git: &GitRepo,
    github: Option<&GitHubClient>,
    commit_hash: &str,
    fallback_since: Option<&str>,
) -> Option<TagInfo> {
    let candidates = collect_tag_candidates(git, commit_hash);
    let has_semver = candidates.iter().any(|candidate| candidate.info.is_semver);

    let targeted_releases = if let Some(gh) = github {
        let target_names: Vec<_> = if has_semver {
            candidates
                .iter()
                .filter(|candidate| candidate.info.is_semver)
                .map(|candidate| candidate.info.name.clone())
                .collect()
        } else {
            candidates
                .iter()
                .map(|candidate| candidate.info.name.clone())
                .collect()
        };

        let mut releases = Vec::new();
        for tag_name in target_names {
            if let Some(release) = gh.fetch_release_by_tag(&tag_name).await {
                releases.push(release);
            }
        }
        releases
    } else {
        Vec::new()
    };

    // Skip GitHub API fallback if we have any local tags (not just semver).
    // This avoids expensive API calls and ancestry checks when we already have the answer locally.
    let fallback_releases = if !candidates.is_empty() {
        Vec::new()
    } else if let (Some(gh), Some(since)) = (github, fallback_since) {
        gh.fetch_releases_since(Some(since)).await
    } else {
        Vec::new()
    };

    resolve_release_from_data(
        git,
        candidates,
        &targeted_releases,
        if fallback_releases.is_empty() {
            None
        } else {
            Some(&fallback_releases)
        },
        commit_hash,
        has_semver,
    )
}

struct TagCandidate {
    info: TagInfo,
    timestamp: i64,
}

fn collect_tag_candidates(git: &GitRepo, commit_hash: &str) -> Vec<TagCandidate> {
    git.tags_containing_commit(commit_hash)
        .into_iter()
        .map(|tag| {
            let timestamp = git.get_commit_timestamp(&tag.commit_hash);
            TagCandidate {
                info: tag,
                timestamp,
            }
        })
        .collect()
}

fn resolve_release_from_data(
    git: &GitRepo,
    mut candidates: Vec<TagCandidate>,
    targeted_releases: &[ReleaseInfo],
    fallback_releases: Option<&[ReleaseInfo]>,
    commit_hash: &str,
    had_semver: bool,
) -> Option<TagInfo> {
    apply_release_metadata(&mut candidates, targeted_releases);

    let local_best = pick_best_tag(&candidates);

    if had_semver {
        return local_best;
    }

    let fallback_best = fallback_releases.and_then(|releases| {
        let remote_candidates = releases
            .iter()
            .filter_map(|release| {
                git.tag_from_release(release).and_then(|tag| {
                    if git.tag_contains_commit(&tag.commit_hash, commit_hash) {
                        let timestamp = git.get_commit_timestamp(&tag.commit_hash);
                        Some(TagCandidate {
                            info: tag,
                            timestamp,
                        })
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();

        pick_best_tag(&remote_candidates)
    });

    fallback_best.or(local_best)
}

fn apply_release_metadata(candidates: &mut [TagCandidate], releases: &[ReleaseInfo]) {
    let release_map: HashMap<&str, &ReleaseInfo> = releases
        .iter()
        .map(|release| (release.tag_name.as_str(), release))
        .collect();

    for candidate in candidates {
        if let Some(release) = release_map.get(candidate.info.name.as_str()) {
            candidate.info.is_release = true;
            candidate.info.release_name = release.name.clone();
            candidate.info.release_url = Some(release.url.clone());
            candidate.info.published_at = release.published_at.clone();
        }
    }
}

fn pick_best_tag(candidates: &[TagCandidate]) -> Option<TagInfo> {
    fn select_with_pred<F>(candidates: &[TagCandidate], predicate: F) -> Option<TagInfo>
    where
        F: Fn(&TagCandidate) -> bool,
    {
        candidates
            .iter()
            .filter(|candidate| predicate(candidate))
            .min_by_key(|candidate| candidate.timestamp)
            .map(|candidate| candidate.info.clone())
    }

    select_with_pred(candidates, |c| c.info.is_release && c.info.is_semver)
        .or_else(|| select_with_pred(candidates, |c| !c.info.is_release && c.info.is_semver))
        .or_else(|| select_with_pred(candidates, |c| c.info.is_release && !c.info.is_semver))
        .or_else(|| select_with_pred(candidates, |c| !c.info.is_release && !c.info.is_semver))
}

/// Resolve a file path
async fn resolve_file(
    file_info: FileInfo,
    git: &GitRepo,
    github: Option<&GitHubClient>,
) -> IdentifiedThing {
    let commit_date = file_info.last_commit.date_rfc3339();
    let release =
        resolve_release_for_commit(git, github, &file_info.last_commit.hash, Some(&commit_date))
            .await;

    let (commit_url, author_urls) = if let Some(gh) = github {
        let url = Some(gh.commit_url(&file_info.last_commit.hash));
        let urls: Vec<Option<String>> = file_info
            .previous_authors
            .iter()
            .map(|(_, _, email)| {
                extract_github_username(email).map(|u| GitHubClient::profile_url(&u))
            })
            .collect();
        (url, urls)
    } else {
        (None, vec![])
    };

    IdentifiedThing::File(Box::new(FileResult {
        file_info,
        commit_url,
        author_urls,
        release,
    }))
}

/// Resolve commit and author URLs efficiently
/// Returns (`commit_url`, `author_profile_url`)
async fn resolve_commit_urls(
    github: Option<&GitHubClient>,
    email: &str,
    commit_hash: &str,
) -> (Option<String>, Option<String>) {
    // Try to extract username from email first (cheap, no API call)
    if let Some(username) = extract_github_username(email) {
        // We have the username from email, but still need commit URL
        let commit_url = github.map(|gh| gh.commit_url(commit_hash));
        return (commit_url, Some(GitHubClient::profile_url(&username)));
    }

    // Try to fetch both URLs from GitHub API in one call
    if let Some(gh) = github {
        if let Some((_hash, commit_url, author_info)) = gh.fetch_commit_info(commit_hash).await {
            let author_url = author_info.map(|(_login, url)| url);
            return (Some(commit_url), author_url);
        }
        // API call failed, fall back to manual URL building
        return (Some(gh.commit_url(commit_hash)), None);
    }

    (None, None)
}

/// Try to extract GitHub username from email
fn extract_github_username(email: &str) -> Option<String> {
    // GitHub emails are typically in the format: username@users.noreply.github.com
    // Or: id+username@users.noreply.github.com
    if email.ends_with("@users.noreply.github.com") {
        let parts: Vec<&str> = email.split('@').collect();
        if let Some(user_part) = parts.first() {
            // Handle both formats
            if let Some(username) = user_part.split('+').next_back() {
                return Some(username.to_string());
            }
        }
    }

    None
}
