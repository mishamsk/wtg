//! Query resolution logic.
//!
//! This module contains the orchestration layer that resolves user queries
//! to identified information using backend implementations.

use crate::backend::Backend;
use crate::error::{WtgError, WtgResult};
use crate::identifier::{EnrichedInfo, EntryPoint, FileResult, IdentifiedThing};
use crate::parse_input::Query;

/// Resolve a query to identified information using the provided backend.
pub(crate) async fn resolve(backend: &dyn Backend, query: &Query) -> WtgResult<IdentifiedThing> {
    match query {
        Query::GitCommit(hash) => resolve_commit(backend, hash).await,
        Query::Pr(number) => resolve_pr(backend, *number).await,
        Query::Issue(number) => resolve_issue(backend, *number).await,
        Query::IssueOrPr(number) => {
            // Try PR first, then issue
            if let Ok(result) = resolve_pr(backend, *number).await {
                return Ok(result);
            }
            if let Ok(result) = resolve_issue(backend, *number).await {
                return Ok(result);
            }
            Err(WtgError::NotFound(format!("#{number}")))
        }
        Query::FilePath(path) => resolve_file(backend, &path.to_string_lossy()).await,
        Query::Unknown(input) => resolve_unknown(backend, input).await,
    }
}

/// Resolve a commit hash to `IdentifiedThing`.
async fn resolve_commit(backend: &dyn Backend, hash: &str) -> WtgResult<IdentifiedThing> {
    let commit = backend.find_commit(hash).await?;
    let commit = backend.enrich_commit(commit).await;
    let release = backend
        .find_release_for_commit(&commit.hash, Some(commit.date))
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
async fn resolve_pr(backend: &dyn Backend, number: u64) -> WtgResult<IdentifiedThing> {
    let pr = backend.fetch_pr(number).await?;

    let commit = backend.find_commit_for_pr(&pr).await.ok();
    let commit = match commit {
        Some(c) => Some(backend.enrich_commit(c).await),
        None => None,
    };

    let release = if let Some(ref c) = commit {
        backend.find_release_for_commit(&c.hash, Some(c.date)).await
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
async fn resolve_issue(backend: &dyn Backend, number: u64) -> WtgResult<IdentifiedThing> {
    let ext_issue = backend.fetch_issue(number).await?;
    let display_issue = (&ext_issue).into();

    // Try to find closing PR info
    let closing_pr = ext_issue.closing_prs.into_iter().next();

    let (commit, release) = if let Some(ref pr) = closing_pr {
        if let Some(merge_sha) = &pr.merge_commit_sha {
            // Check if PR is from a different repo (cross-project)
            let is_cross_repo = pr.repo_info.as_ref().is_some_and(|pr_repo| {
                backend
                    .repo_info()
                    .is_some_and(|ri| pr_repo.owner() != ri.owner() || pr_repo.repo() != ri.repo())
            });

            if is_cross_repo {
                // Fetch from PR's repo using cross-project backend
                if let Some(pr_repo) = &pr.repo_info
                    && let Some(cross_backend) = backend.for_repo(pr_repo).await
                {
                    let commit = cross_backend.find_commit(merge_sha).await.ok();
                    let commit = match commit {
                        Some(c) => Some(cross_backend.enrich_commit(c).await),
                        None => None,
                    };

                    // Try issue's repo first, fall back to PR's repo
                    let release = if let Some(ref c) = commit {
                        let hash = &c.hash;
                        let date = Some(c.date);
                        match backend.find_release_for_commit(hash, date).await {
                            Some(r) => Some(r),
                            None => cross_backend.find_release_for_commit(hash, date).await,
                        }
                    } else {
                        None
                    };

                    (commit, release)
                } else {
                    (None, None)
                }
            } else {
                // Same repo - use provided backend
                let commit = backend.find_commit(merge_sha).await.ok();
                let commit = match commit {
                    Some(c) => Some(backend.enrich_commit(c).await),
                    None => None,
                };
                let release = if let Some(ref c) = commit {
                    backend.find_release_for_commit(&c.hash, Some(c.date)).await
                } else {
                    None
                };
                (commit, release)
            }
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
async fn resolve_file(backend: &dyn Backend, path: &str) -> WtgResult<IdentifiedThing> {
    let file_info = backend.find_file(path).await?;
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
        )
        .await;

    Ok(IdentifiedThing::File(Box::new(FileResult {
        file_info,
        commit_url,
        author_urls,
        release,
    })))
}

/// Resolve a tag name to `IdentifiedThing`.
async fn resolve_tag(backend: &dyn Backend, name: &str) -> WtgResult<IdentifiedThing> {
    let tag = backend.find_tag(name).await?;
    let url = backend.tag_url(name);
    Ok(IdentifiedThing::TagOnly(Box::new(tag), url))
}

/// Resolve unknown input by trying each possibility.
async fn resolve_unknown(backend: &dyn Backend, input: &str) -> WtgResult<IdentifiedThing> {
    // Try as commit hash
    if let Ok(result) = resolve_commit(backend, input).await {
        return Ok(result);
    }

    // Try as PR/issue number (if numeric)
    if let Ok(number) = input.parse::<u64>() {
        if let Ok(result) = resolve_pr(backend, number).await {
            return Ok(result);
        }
        if let Ok(result) = resolve_issue(backend, number).await {
            return Ok(result);
        }
    }

    // Try as file path
    if let Ok(result) = resolve_file(backend, input).await {
        return Ok(result);
    }

    // Try as tag
    if let Ok(result) = resolve_tag(backend, input).await {
        return Ok(result);
    }

    Err(WtgError::NotFound(input.to_string()))
}
