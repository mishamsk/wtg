mod common;

use common::{TestRepoFixture, test_repo};
use rstest::rstest;
use wtg_cli::identifier::{EntryPoint, IdentifiedThing, identify};

/// Test identifying a commit by its hash
#[rstest]
#[tokio::test]
async fn test_identify_commit_by_hash(test_repo: TestRepoFixture) {
    let commit_hash = &test_repo.commits.commit0_initial;

    // Identify the commit
    let result = identify(commit_hash, test_repo.repo.clone())
        .await
        .expect("Failed to identify commit");

    // Verify it's an enriched result
    match result {
        IdentifiedThing::Enriched(info) => {
            // Check entry point
            assert!(matches!(info.entry_point, EntryPoint::Commit(hash) if hash == *commit_hash));

            // Check commit info exists
            assert!(info.commit.is_some());
            let commit = info.commit.unwrap();

            // Verify commit details
            assert_eq!(commit.hash, test_repo.commits.commit0_initial);
            assert_eq!(commit.message, "Initial commit");
            assert_eq!(commit.author_name, "Test User");
            assert_eq!(commit.author_email, "test@example.com");
        }
        _ => panic!("Expected Enriched result, got something else"),
    }
}

/// Test identifying a commit with short hash
#[rstest]
#[tokio::test]
async fn test_identify_commit_by_short_hash(test_repo: TestRepoFixture) {
    // Use short hash of commit 1 (which has v1.0.0 tag)
    let short_hash = &test_repo.commits.commit1_add_file[..7];

    // Identify the commit using short hash
    let result = identify(short_hash, test_repo.repo.clone())
        .await
        .expect("Failed to identify commit");

    // Verify it's an enriched result with correct commit
    match result {
        IdentifiedThing::Enriched(info) => {
            assert!(info.commit.is_some());
            let commit = info.commit.unwrap();

            assert_eq!(commit.hash, test_repo.commits.commit1_add_file);
            assert_eq!(commit.message, "Add test.txt file");
            assert_eq!(commit.author_name, "Test User");

            // Verify it has the v1.0.0 tag (since tag is on commit 1)
            assert!(info.release.is_some());
            let release = info.release.unwrap();
            assert_eq!(release.name, "v1.0.0");
            assert!(release.is_semver);
        }
        _ => panic!("Expected Enriched result, got something else"),
    }
}

/// Test identifying a file
#[rstest]
#[tokio::test]
async fn test_identify_file(test_repo: TestRepoFixture) {
    // Identify test.txt
    let result = identify("test.txt", test_repo.repo.clone())
        .await
        .expect("Failed to identify file");

    // Verify it's a file result
    match result {
        IdentifiedThing::File(file_result) => {
            // Check file info
            assert_eq!(file_result.file_info.path, "test.txt");

            // Check last commit (should be commit 2)
            let last_commit = &file_result.file_info.last_commit;
            assert_eq!(last_commit.hash, test_repo.commits.commit2_update_file);
            assert_eq!(last_commit.message, "Update test.txt with new content");
            assert_eq!(last_commit.author_name, "Another Author");

            // Check previous authors (should have at least one - the original author)
            assert!(!file_result.file_info.previous_authors.is_empty());
            let prev_author = &file_result.file_info.previous_authors[0];
            assert_eq!(prev_author.1, "Test User"); // name

            // Should have beta-release tag since that's on commit 2
            assert!(file_result.release.is_some());
            let release = file_result.release.unwrap();
            assert_eq!(release.name, "beta-release");
            assert!(!release.is_semver);
        }
        _ => panic!("Expected File result, got something else"),
    }
}

/// Test identifying a tag
#[rstest]
#[tokio::test]
async fn test_identify_tag(test_repo: TestRepoFixture) {
    // Identify v1.0.0 tag
    let result = identify("v1.0.0", test_repo.repo.clone())
        .await
        .expect("Failed to identify tag");

    // Verify it's a tag-only result
    match result {
        IdentifiedThing::TagOnly(tag_info, _github_url) => {
            assert_eq!(tag_info.name, "v1.0.0");
            assert_eq!(tag_info.commit_hash, test_repo.commits.commit1_add_file);
            assert!(tag_info.is_semver);

            let semver = tag_info.semver_info.expect("Should have semver info");
            assert_eq!(semver.major, 1);
            assert_eq!(semver.minor, 0);
            assert_eq!(semver.patch, Some(0));
        }
        _ => panic!("Expected TagOnly result, got something else"),
    }
}

/// Test that nonexistent input returns error
#[rstest]
#[tokio::test]
async fn test_identify_nonexistent(test_repo: TestRepoFixture) {
    let result = identify("nonexistent-thing", test_repo.repo.clone()).await;

    assert!(result.is_err());
}
