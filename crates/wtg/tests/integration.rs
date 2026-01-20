/// Integration tests that run against the actual wtg repository.
/// These tests are excluded from the default test run and should be run explicitly.
///
/// To run these tests:
/// - Locally: `just test-integration`
/// - CI: automatically included in the `ci` profile
use std::path::PathBuf;
use wtg_cli::backend::resolve_backend;
use wtg_cli::parse_input::{ParsedInput, ParsedQuery, Query};
use wtg_cli::release_filter::ReleaseFilter;
use wtg_cli::resolution::IdentifiedThing;
use wtg_cli::resolution::resolve;

/// Test identifying a recent commit from the actual wtg repository
#[tokio::test]
async fn integration_identify_recent_commit() {
    // Identify a known commit (from git log)
    let parsed_input = ParsedInput::new_local_query(ParsedQuery::Resolved(Query::GitCommit(
        "6146f62054c1eb14792be673275f8bc9a2e223f3".to_string(),
    )));
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate commit");

    let result = resolve(backend.as_ref(), &query, &ReleaseFilter::Unrestricted)
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
    let parsed_input = ParsedInput::new_local_query(ParsedQuery::Unknown(TAG_NAME.to_string()));
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate tag");

    let result = resolve(backend.as_ref(), &query, &ReleaseFilter::Unrestricted)
        .await
        .expect("Failed to identify tag");

    let snapshot = to_snapshot(&result);
    insta::assert_yaml_snapshot!(snapshot);
}

/// Test identifying a file from the actual wtg repository
#[tokio::test]
async fn integration_identify_file() {
    // Identify LICENSE (which should not change)
    let parsed_input = ParsedInput::new_local_query(ParsedQuery::Resolved(Query::FilePath {
        branch: "HEAD".to_string(),
        path: PathBuf::from("LICENSE"),
    }));
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate file");

    let result = resolve(backend.as_ref(), &query, &ReleaseFilter::Unrestricted)
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

/// Test end-to-end resolution of a Zed issue through PR, commit, and release.
/// This uses the full CLI flow: parse URL -> resolve backend -> disambiguate -> resolve.
/// <https://github.com/zed-industries/zed/issues/41633>
#[tokio::test]
async fn integration_identify_zed_issue_41633() {
    use wtg_cli::backend::resolve_backend;
    use wtg_cli::parse_input::try_parse_input;
    use wtg_cli::resolution::{EntryPoint, IdentifiedThing, resolve};

    // Step 1: Parse the GitHub issue URL (same as CLI)
    let parsed_input = try_parse_input("https://github.com/zed-industries/zed/issues/41633", None)
        .expect("Failed to parse URL");

    // Step 2: Create backend (same as CLI)
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");

    // Step 3: Disambiguate the query (same as CLI)
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate query");

    // Step 4: Resolve (same as CLI)
    let result = resolve(backend.as_ref(), &query, &ReleaseFilter::Unrestricted)
        .await
        .expect("Failed to resolve");

    // Verify the result
    let IdentifiedThing::Enriched(info) = result else {
        panic!("Expected Enriched result, got {result:?}");
    };

    // Entry point should be IssueNumber
    assert!(
        matches!(info.entry_point, EntryPoint::IssueNumber(41633)),
        "Expected IssueNumber(41633) entry point, got {:?}",
        info.entry_point
    );

    // Verify issue
    let issue = info.issue.as_ref().expect("Expected issue info");
    assert_eq!(issue.number, 41633);
    assert_eq!(issue.author.as_deref(), Some("korikhin"));

    // Verify PR
    let pr = info.pr.as_ref().expect("Expected PR info");
    assert_eq!(pr.number, 41639);
    assert_eq!(pr.author.as_deref(), Some("danilo-leal"));
    assert!(pr.merged, "Expected PR to be merged");

    // Verify commit
    let commit = info.commit.as_ref().expect("Expected commit info");
    assert!(
        commit.hash.starts_with("1f938c0"),
        "Expected commit hash to start with 1f938c0, got {}",
        commit.hash
    );

    // Verify release
    let release = info.release.as_ref().expect("Expected release info");
    assert_eq!(release.name, "v0.212.0-pre");
}

/// Test end-to-end resolution of a go-task/task issue through PR, commit, and release.
/// This is a cross-project PR: the issue is in go-task/task but the fixing PR
/// comes from a different author than the issue reporter.
/// <https://github.com/go-task/task/issues/1322>
#[tokio::test]
async fn integration_identify_go_task_issue_1322() {
    use wtg_cli::backend::resolve_backend;
    use wtg_cli::parse_input::try_parse_input;
    use wtg_cli::resolution::{EntryPoint, IdentifiedThing, resolve};

    // Step 1: Parse the GitHub issue URL (same as CLI)
    let parsed_input = try_parse_input("https://github.com/go-task/task/issues/1322", None)
        .expect("Failed to parse URL");

    // Step 2: Create backend (same as CLI)
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");

    // Step 3: Disambiguate the query (same as CLI)
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate query");

    // Step 4: Resolve (same as CLI)
    let result = resolve(backend.as_ref(), &query, &ReleaseFilter::Unrestricted)
        .await
        .expect("Failed to resolve");

    // Verify the result
    let IdentifiedThing::Enriched(info) = result else {
        panic!("Expected Enriched result, got {result:?}");
    };

    // Entry point should be IssueNumber
    assert!(
        matches!(info.entry_point, EntryPoint::IssueNumber(1322)),
        "Expected IssueNumber(1322) entry point, got {:?}",
        info.entry_point
    );

    // Verify issue
    let issue = info.issue.as_ref().expect("Expected issue info");
    assert_eq!(issue.number, 1322);
    assert_eq!(issue.author.as_deref(), Some("StefanBRas"));

    // Verify PR
    let pr = info.pr.as_ref().expect("Expected PR info");
    assert_eq!(pr.number, 2053);
    assert_eq!(pr.author.as_deref(), Some("vmaerten"));
    assert!(pr.merged, "Expected PR to be merged");

    // Verify commit
    let commit = info.commit.as_ref().expect("Expected commit info");
    assert!(
        commit.hash.starts_with("15b7e3c"),
        "Expected commit hash to start with 15b7e3c, got {}",
        commit.hash
    );

    // Verify release
    let release = info.release.as_ref().expect("Expected release info");
    assert_eq!(release.name, "v3.45.5");
}

/// Test that skip-prereleases filter resolves to same tag as unrestricted
/// when no pre-releases are present. Verifies filter doesn't break normal resolution.
#[tokio::test]
async fn integration_skip_prereleases_filter() {
    let parsed_input = ParsedInput::new_local_query(ParsedQuery::Resolved(Query::GitCommit(
        "6146f62054c1eb14792be673275f8bc9a2e223f3".to_string(),
    )));
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate commit");

    // Resolve without filter
    let result_unrestricted = resolve(backend.as_ref(), &query, &ReleaseFilter::Unrestricted)
        .await
        .expect("Failed to resolve without filter");

    // Resolve with SkipPrereleases filter
    let result_filtered = resolve(backend.as_ref(), &query, &ReleaseFilter::SkipPrereleases)
        .await
        .expect("Failed to resolve with skip-prereleases filter");

    // Both should resolve to the same tag (wtg repo has no pre-releases)
    let tag_unrestricted = match &result_unrestricted {
        IdentifiedThing::Enriched(info) => info.release.as_ref().map(|r| r.name.clone()),
        _ => None,
    };
    let tag_filtered = match &result_filtered {
        IdentifiedThing::Enriched(info) => info.release.as_ref().map(|r| r.name.clone()),
        _ => None,
    };
    assert_eq!(
        tag_unrestricted, tag_filtered,
        "Filter should not change result when no pre-releases exist"
    );
}

/// Test that specifying a nonexistent tag returns `TagNotFound` error.
/// Uses a tag name that definitely doesn't exist in the wtg repo.
#[tokio::test]
async fn integration_nonexistent_tag_error() {
    let parsed_input = ParsedInput::new_local_query(ParsedQuery::Resolved(Query::GitCommit(
        "6146f62054c1eb14792be673275f8bc9a2e223f3".to_string(),
    )));
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate commit");

    // Tag that doesn't exist in wtg repo
    let filter = ReleaseFilter::Specific("v999.999.999-never-exists".to_string());

    let result = resolve(backend.as_ref(), &query, &filter).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.is_tag_not_found(),
        "Expected TagNotFound error, got {err:?}"
    );
}

/// Test that specifying a valid tag returns that exact tag when commit is in it.
#[tokio::test]
async fn integration_specific_tag_found() {
    let parsed_input = ParsedInput::new_local_query(ParsedQuery::Resolved(Query::GitCommit(
        "6146f62054c1eb14792be673275f8bc9a2e223f3".to_string(),
    )));
    let backend = resolve_backend(&parsed_input, false).expect("Failed to create backend");
    let query = backend
        .disambiguate_query(parsed_input.query())
        .await
        .expect("Failed to disambiguate commit");

    // Use v0.1.0 which exists and contains this commit
    let filter = ReleaseFilter::Specific("v0.1.0".to_string());

    let result = resolve(backend.as_ref(), &query, &filter)
        .await
        .expect("Failed to resolve with specific tag filter");

    let IdentifiedThing::Enriched(info) = result else {
        panic!("Expected Enriched result");
    };

    // Verify the release is the specific tag we asked for
    let release = info
        .release
        .expect("Expected release info for commit in v0.1.0");
    assert_eq!(
        release.name, "v0.1.0",
        "Release should be the specific tag requested"
    );
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
