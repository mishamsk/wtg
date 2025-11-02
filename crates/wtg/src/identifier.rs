use crate::error::{Result, WtgError};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo};
use crate::github::{GitHubClient, IssueInfo, PullRequestInfo};

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
    Enriched(EnrichedInfo),
    File(FileResult),
    TagOnly(TagInfo, Option<String>), // Just a tag, no commit yet
}

pub async fn identify(input: &str, git: GitRepo) -> Result<IdentifiedThing> {
    let github = git
        .github_remote()
        .map(|(owner, repo)| GitHubClient::new(owner, repo));

    // Try as commit hash first
    if let Some(commit_info) = git.find_commit(input) {
        return Ok(resolve_commit(
            EntryPoint::Commit(input.to_string()),
            commit_info,
            &git,
            github.as_ref(),
        )
        .await);
    }

    // Try as issue/PR number (if it's all digits or starts with #)
    let number_str = input.strip_prefix('#').unwrap_or(input);
    if let Ok(number) = number_str.parse::<u64>() {
        if let Some(result) = resolve_number(number, &git, github.as_ref()).await {
            return Ok(result);
        }
    }

    // Try as file path
    if let Some(file_info) = git.find_file(input) {
        return Ok(resolve_file(file_info, &git, github.as_ref()).await);
    }

    // Try as tag
    let tags = git.get_tags();
    if let Some(tag_info) = tags.iter().find(|t| t.name == input) {
        let github_url = github.as_ref().map(|gh| gh.tag_url(&tag_info.name));
        return Ok(IdentifiedThing::TagOnly(tag_info.clone(), github_url));
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
    let commit_url = github.map(|gh| gh.commit_url(&commit_info.hash));

    // Try to get GitHub username: first from email, then from GitHub API
    let commit_author_github_url =
        if let Some(username) = extract_github_username(&commit_info.author_email) {
            Some(GitHubClient::profile_url(&username))
        } else if let Some(gh) = github {
            // Fallback: fetch from GitHub API to get actual username
            gh.fetch_commit_author(&commit_info.hash)
                .await
                .map(|u| GitHubClient::profile_url(&u))
        } else {
            None
        };

    // OPTIMIZED: Use commit date to filter releases (only fetch releases after this commit)
    let github_releases = if let Some(gh) = github {
        let commit_date = commit_info.date_rfc3339();
        gh.fetch_releases_since(Some(&commit_date)).await
    } else {
        Vec::new()
    };
    let release = git.find_closest_release_with_github(&github_releases, &commit_info.hash);

    IdentifiedThing::Enriched(EnrichedInfo {
        entry_point,
        commit: Some(commit_info),
        commit_url,
        commit_author_github_url,
        pr: None,
        issue: None,
        release,
    })
}

/// Resolve an issue/PR number
async fn resolve_number(
    number: u64,
    git: &GitRepo,
    github: Option<&GitHubClient>,
) -> Option<IdentifiedThing> {
    let gh = github?;

    // Try as PR first
    if let Some(pr_info) = gh.fetch_pr(number).await {
        // If PR is merged, resolve to commit and enrich with PR info
        if let Some(merge_sha) = &pr_info.merge_commit_sha {
            if let Some(commit_info) = git.find_commit(merge_sha) {
                let commit_url = Some(gh.commit_url(&commit_info.hash));

                // Try to get GitHub username: first from email, then from GitHub API
                let commit_author_github_url =
                    if let Some(username) = extract_github_username(&commit_info.author_email) {
                        Some(GitHubClient::profile_url(&username))
                    } else {
                        gh.fetch_commit_author(&commit_info.hash)
                            .await
                            .map(|u| GitHubClient::profile_url(&u))
                    };

                // Optimize: only fetch releases since PR creation
                let github_releases = gh.fetch_releases_since(pr_info.created_at.as_deref()).await;
                let release = git.find_closest_release_with_github(&github_releases, merge_sha);

                return Some(IdentifiedThing::Enriched(EnrichedInfo {
                    entry_point: EntryPoint::PullRequestNumber(number),
                    commit: Some(commit_info),
                    commit_url,
                    commit_author_github_url,
                    pr: Some(pr_info),
                    issue: None,
                    release,
                }));
            }
        }

        // PR not merged yet - return PR without commit
        return Some(IdentifiedThing::Enriched(EnrichedInfo {
            entry_point: EntryPoint::PullRequestNumber(number),
            commit: None,
            commit_url: None,
            commit_author_github_url: None,
            pr: Some(pr_info),
            issue: None,
            release: None,
        }));
    }

    // Try as issue
    if let Some(issue_info) = gh.fetch_issue(number).await {
        // If issue has closing PRs, fetch the first one and enrich
        if let Some(&first_pr_number) = issue_info.closing_prs.first() {
            if let Some(pr_info) = gh.fetch_pr(first_pr_number).await {
                // If PR is merged, resolve to commit
                if let Some(merge_sha) = &pr_info.merge_commit_sha {
                    if let Some(commit_info) = git.find_commit(merge_sha) {
                        let commit_url = Some(gh.commit_url(&commit_info.hash));

                        // Try to get GitHub username: first from email, then from GitHub API
                        let commit_author_github_url = if let Some(username) =
                            extract_github_username(&commit_info.author_email)
                        {
                            Some(GitHubClient::profile_url(&username))
                        } else {
                            gh.fetch_commit_author(&commit_info.hash)
                                .await
                                .map(|u| GitHubClient::profile_url(&u))
                        };

                        // Optimize: only fetch releases since issue creation
                        let github_releases = gh
                            .fetch_releases_since(issue_info.created_at.as_deref())
                            .await;
                        let release =
                            git.find_closest_release_with_github(&github_releases, merge_sha);

                        return Some(IdentifiedThing::Enriched(EnrichedInfo {
                            entry_point: EntryPoint::IssueNumber(number),
                            commit: Some(commit_info),
                            commit_url,
                            commit_author_github_url,
                            pr: Some(pr_info),
                            issue: Some(issue_info),
                            release,
                        }));
                    }
                }

                // PR not merged - return issue + PR without commit
                return Some(IdentifiedThing::Enriched(EnrichedInfo {
                    entry_point: EntryPoint::IssueNumber(number),
                    commit: None,
                    commit_url: None,
                    commit_author_github_url: None,
                    pr: Some(pr_info),
                    issue: Some(issue_info),
                    release: None,
                }));
            }
        }

        // Issue without PRs - return just issue
        return Some(IdentifiedThing::Enriched(EnrichedInfo {
            entry_point: EntryPoint::IssueNumber(number),
            commit: None,
            commit_url: None,
            commit_author_github_url: None,
            pr: None,
            issue: Some(issue_info),
            release: None,
        }));
    }

    None
}

/// Resolve a file path
async fn resolve_file(
    file_info: FileInfo,
    git: &GitRepo,
    github: Option<&GitHubClient>,
) -> IdentifiedThing {
    // OPTIMIZED: Use file's last commit date to filter releases
    let github_releases = if let Some(gh) = github {
        let commit_date = file_info.last_commit.date_rfc3339();
        gh.fetch_releases_since(Some(&commit_date)).await
    } else {
        Vec::new()
    };

    let release =
        git.find_closest_release_with_github(&github_releases, &file_info.last_commit.hash);

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

    IdentifiedThing::File(FileResult {
        file_info,
        commit_url,
        author_urls,
        release,
    })
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
