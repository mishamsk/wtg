/// Integration tests that run against the actual wtg repository.
/// These tests are excluded from the default test run and should be run explicitly.
///
/// To run these tests:
/// - Locally: `just test-integration`
/// - CI: automatically included in the `ci` profile
use wtg_cli::git::GitRepo;
use wtg_cli::identifier::{IdentifiedThing, identify};

/// Test identifying a recent commit from the actual wtg repository
#[tokio::test]
async fn integration_identify_recent_commit() {
    // Open the actual wtg repository
    let repo = GitRepo::open().expect("Failed to open wtg repository");

    // Identify a known commit (from git log)
    let result = identify("6146f62054c1eb14792be673275f8bc9a2e223f3", repo)
        .await
        .expect("Failed to identify commit");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Test identifying a tag from the actual wtg repository
#[tokio::test]
async fn integration_identify_tag() {
    const TAG_NAME: &str = "v0.1.0";

    let repo = GitRepo::open().expect("Failed to open wtg repository");

    // Identify the first tag
    let result = identify(TAG_NAME, repo)
        .await
        .expect("Failed to identify tag");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Test identifying a file from the actual wtg repository
#[tokio::test]
async fn integration_identify_file() {
    let repo = GitRepo::open().expect("Failed to open wtg repository");

    // Identify LICENSE (which should not change)
    let result = identify("LICENSE", repo)
        .await
        .expect("Failed to identify LICENSE");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Convert `IdentifiedThing` to a consistent snapshot structure
fn to_snapshot(result: &IdentifiedThing) -> IntegrationSnapshot {
    match result {
        IdentifiedThing::Enriched(info) => IntegrationSnapshot {
            result_type: "enriched".to_string(),
            entry_point: Some(format!("{:?}", info.entry_point)),
            commit_message: info.commit.as_ref().map(|c| c.message.clone()),
            commit_author: info.commit.as_ref().map(|c| c.author_name.clone()),
            has_commit_url: info.commit_url.is_some(),
            has_pr: info.pr.is_some(),
            has_issue: info.issue.is_some(),
            release_name: info.release.as_ref().map(|r| r.name.clone()),
            release_is_semver: info.release.as_ref().map(|r| r.is_semver),
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
            release_is_semver: Some(tag_info.is_semver),
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
            release_is_semver: file_result.release.as_ref().map(|r| r.is_semver),
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
