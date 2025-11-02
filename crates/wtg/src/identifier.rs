use crate::error::{Result, WtgError};
use crate::git::{CommitInfo, FileInfo, GitRepo, TagInfo};
use crate::github::{GitHubClient, IssueInfo};

#[derive(Debug, Clone)]
pub enum IdentifiedThing {
    Commit {
        info: CommitInfo,
        release: Option<TagInfo>,
        github_url: Option<String>,
        author_url: Option<String>,
    },
    File {
        info: FileInfo,
        release: Option<TagInfo>,
        github_url: Option<String>,
        author_urls: Vec<Option<String>>,
    },
    Issue {
        info: IssueInfo,
        release: Option<TagInfo>,
    },
    Tag {
        info: TagInfo,
        github_url: Option<String>,
    },
}

pub async fn identify(input: &str) -> Result<IdentifiedThing> {
    // Open git repo
    let git = GitRepo::open()?;

    // Get GitHub client if available
    let github = git
        .github_remote()
        .map(|(owner, repo)| GitHubClient::new(owner, repo));

    // Fetch GitHub releases if available (used to enrich tag info)
    let github_releases = if let Some(gh) = &github {
        gh.fetch_releases().await
    } else {
        Vec::new()
    };

    // Track what we tried
    let mut matches = Vec::new();

    // Try as commit hash first
    if let Some(commit_info) = git.find_commit(input) {
        matches.push("commit");

        let release = git.find_closest_release_with_github(&github_releases, &commit_info.hash);

        let (github_url, author_url) = if let Some(gh) = &github {
            (
                Some(gh.commit_url(&commit_info.hash)),
                extract_github_username(&commit_info.author_email)
                    .map(|u| GitHubClient::profile_url(&u)),
            )
        } else {
            (None, None)
        };

        return Ok(IdentifiedThing::Commit {
            info: commit_info,
            release,
            github_url,
            author_url,
        });
    }

    // Try as issue/PR number (if it's all digits or starts with #)
    let issue_number = input.strip_prefix('#').unwrap_or(input);
    if let Ok(number) = issue_number.parse::<u64>()
        && let Some(gh) = &github
    {
        // Try to fetch, but handle network errors gracefully
        if let Some(issue_info) = gh.fetch_issue(number).await {
            matches.push("issue/PR");

            // For PRs, find release using merge commit SHA
            let release = if let Some(merge_sha) = &issue_info.merge_commit_sha {
                git.find_closest_release_with_github(&github_releases, merge_sha)
            } else {
                // For issues, try to find commits that mention this issue
                // TODO: Parse commit messages for "closes #123" patterns
                None
            };

            return Ok(IdentifiedThing::Issue {
                info: issue_info,
                release,
            });
        }
        // Might be network issue, continue trying other things
    }

    // Try as file path
    if let Some(file_info) = git.find_file(input) {
        matches.push("file");

        let release =
            git.find_closest_release_with_github(&github_releases, &file_info.last_commit.hash);

        let (github_url, author_urls) = if let Some(gh) = &github {
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

        return Ok(IdentifiedThing::File {
            info: file_info,
            release,
            github_url,
            author_urls,
        });
    }

    // Try as tag
    let tags = git.get_tags_with_releases(&github_releases);
    if let Some(tag_info) = tags.iter().find(|t| t.name == input) {
        matches.push("tag");

        let github_url = github.as_ref().map(|gh| gh.tag_url(&tag_info.name));

        return Ok(IdentifiedThing::Tag {
            info: tag_info.clone(),
            github_url,
        });
    }

    // Easter egg: if we somehow matched multiple things (shouldn't happen)
    if matches.len() > 1 {
        return Err(WtgError::MultipleMatches(
            matches.iter().map(|s| (*s).to_string()).collect(),
        ));
    }

    // Nothing found
    Err(WtgError::NotFound(input.to_string()))
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
