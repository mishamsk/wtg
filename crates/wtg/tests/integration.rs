/// Integration tests that run against the actual wtg repository.
/// These tests are excluded from the default test run and should be run explicitly.
///
/// To run these tests:
/// - Locally: `just test-integration`
/// - CI: automatically included in the `ci` profile
use std::path::PathBuf;
use wtg_cli::backend::resolve_backend;
use wtg_cli::parse_input::{ParsedInput, Query};
use wtg_cli::resolution::IdentifiedThing;
use wtg_cli::resolution::resolve;

/// Test identifying a recent commit from the actual wtg repository
#[tokio::test]
async fn integration_identify_recent_commit() {
    // Identify a known commit (from git log)
    let query = Query::GitCommit("6146f62054c1eb14792be673275f8bc9a2e223f3".to_string());
    let parsed_input = ParsedInput::new_local_query(query.clone());
    let resolved = resolve_backend(&parsed_input, false).expect("Failed to create backend");

    let result = resolve(resolved.backend.as_ref(), &query)
        .await
        .expect("Failed to identify commit");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Test identifying a tag from the actual wtg repository
#[tokio::test]
async fn integration_identify_tag() {
    const TAG_NAME: &str = "v0.1.0";

    // Identify the first tag
    let query = Query::Unknown(TAG_NAME.to_string());
    let parsed_input = ParsedInput::new_local_query(query.clone());
    let resolved = resolve_backend(&parsed_input, false).expect("Failed to create backend");

    let result = resolve(resolved.backend.as_ref(), &query)
        .await
        .expect("Failed to identify tag");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Test identifying a file from the actual wtg repository
#[tokio::test]
async fn integration_identify_file() {
    // Identify LICENSE (which should not change)
    let query = Query::FilePath(PathBuf::from("LICENSE"));
    let parsed_input = ParsedInput::new_local_query(query.clone());
    let resolved = resolve_backend(&parsed_input, false).expect("Failed to create backend");

    let result = resolve(resolved.backend.as_ref(), &query)
        .await
        .expect("Failed to identify LICENSE");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Test finding closing PRs for a GitHub issue
/// This tests the ability to find PRs that close issues, specifically
/// testing that we prioritize Closed events with `commit_id` and only
/// consider merged PRs.
/// <https://github.com/ghostty-org/ghostty/issues/4800>
#[tokio::test]
async fn integration_identify_ghostty_issue_4800() {
    use wtg_cli::github::{GhRepoInfo, GitHubClient};

    // Create a GitHub client for the ghostty repository
    let repo_info = GhRepoInfo::new("ghostty-org".to_string(), "ghostty".to_string());
    let client = GitHubClient::new().expect("Failed to create GitHub client");

    // Fetch the issue
    let issue = client
        .fetch_issue(&repo_info, 4800)
        .await
        .expect("Failed to fetch ghostty issue #4800");

    assert_eq!(
        issue.closing_prs.len(),
        1,
        "Expected exactly one closing PR"
    );

    assert_eq!(issue.closing_prs[0].number, 7704);
}

/// Convert `IdentifiedThing` to a consistent snapshot structure
fn to_snapshot(result: &IdentifiedThing) -> IntegrationSnapshot {
    match result {
        IdentifiedThing::Enriched(info) => IntegrationSnapshot {
            result_type: "enriched".to_string(),
            entry_point: Some(format!("{:?}", info.entry_point)),
            commit_message: info.commit.as_ref().map(|c| c.message.clone()),
            commit_author: info.commit.as_ref().map(|c| c.author_name.clone()),
            has_commit_url: info
                .commit
                .as_ref()
                .and_then(|ci| ci.commit_url.as_deref())
                .is_some(),
            has_pr: info.pr.is_some(),
            has_issue: info.issue.is_some(),
            release_name: info.release.as_ref().map(|r| r.name.clone()),
            release_is_semver: info.release.as_ref().map(wtg_cli::git::TagInfo::is_semver),
            tag_name: None,
            file_path: None,
            previous_authors_count: None,
        },
        IdentifiedThing::TagOnly(tag_info, github_url) => IntegrationSnapshot {
            result_type: "tag_only".to_string(),
            entry_point: None,
            commit_message: None,
            commit_author: None,
            has_commit_url: github_url.is_some(),
            has_pr: false,
            has_issue: false,
            release_name: if tag_info.is_release {
                Some(tag_info.name.clone())
            } else {
                None
            },
            release_is_semver: Some(tag_info.is_semver()),
            tag_name: Some(tag_info.name.clone()),
            file_path: None,
            previous_authors_count: None,
        },
        IdentifiedThing::File(file_result) => IntegrationSnapshot {
            result_type: "file".to_string(),
            entry_point: None,
            commit_message: Some(file_result.file_info.last_commit.message.clone()),
            commit_author: Some(file_result.file_info.last_commit.author_name.clone()),
            has_commit_url: file_result.commit_url.is_some(),
            has_pr: false,
            has_issue: false,
            release_name: file_result.release.as_ref().map(|r| r.name.clone()),
            release_is_semver: file_result
                .release
                .as_ref()
                .map(wtg_cli::git::TagInfo::is_semver),
            tag_name: None,
            file_path: Some(file_result.file_info.path.clone()),
            previous_authors_count: Some(file_result.file_info.previous_authors.len()),
        },
    }
}

/// Unified snapshot structure for all integration tests
/// Captures common elements (commit, release) plus type-specific fields
#[derive(serde::Serialize)]
struct IntegrationSnapshot {
    result_type: String,
    // Entry point (for commits)
    entry_point: Option<String>,
    // Commit information (common to all types)
    commit_message: Option<String>,
    commit_author: Option<String>,
    has_commit_url: bool,
    // PR/Issue (for commits)
    has_pr: bool,
    has_issue: bool,
    // Release information (common to all types)
    release_name: Option<String>,
    release_is_semver: Option<bool>,
    tag_name: Option<String>,
    // File-specific
    file_path: Option<String>,
    previous_authors_count: Option<usize>,
}
