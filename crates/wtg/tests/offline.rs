mod common;

use common::{TestRepoFixture, test_repo};
use rstest::rstest;
use std::path::{Path, PathBuf};
use wtg_cli::backend::{Backend, GitBackend};
use wtg_cli::parse_input::{ParsedQuery, Query};
use wtg_cli::release_filter::ReleaseFilter;
use wtg_cli::resolution::resolve;
use wtg_cli::resolution::{EntryPoint, IdentifiedThing};

/// Test identifying a commit by its hash
#[rstest]
#[tokio::test]
async fn test_identify_commit_by_hash(test_repo: TestRepoFixture) {
    let commit_hash = &test_repo.commits.commit0_initial;
    let backend = GitBackend::new(test_repo.repo);
    let query = Query::GitCommit(commit_hash.clone());

    let result = resolve(&backend, &query, &ReleaseFilter::Unrestricted)
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
            assert_eq!(commit.author_email.as_deref(), Some("test@example.com"));
            assert!(
                commit.author_login.is_none(),
                "Expected no GitHub login for test user"
            );
            assert!(
                commit.author_url.is_none(),
                "Expected no GitHub author url for test user"
            );
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
    let backend = GitBackend::new(test_repo.repo);
    let query = Query::GitCommit(short_hash.to_string());

    let result = resolve(&backend, &query, &ReleaseFilter::Unrestricted)
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
            assert!(release.is_semver());
        }
        _ => panic!("Expected Enriched result, got something else"),
    }
}

/// Test identifying a file
#[rstest]
#[tokio::test]
async fn test_identify_file(test_repo: TestRepoFixture) {
    let backend = GitBackend::new(test_repo.repo);
    let query = Query::FilePath {
        branch: "HEAD".to_string(),
        path: PathBuf::from("test.txt"),
    };

    let result = resolve(&backend, &query, &ReleaseFilter::Unrestricted)
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
            assert!(!release.is_semver());
        }
        _ => panic!("Expected File result, got something else"),
    }
}

/// Test identifying a tag
#[rstest]
#[tokio::test]
async fn test_identify_tag(test_repo: TestRepoFixture) {
    let backend = GitBackend::new(test_repo.repo);
    let query = backend
        .disambiguate_query(&ParsedQuery::Unknown("v1.0.0".to_string()))
        .await
        .expect("Failed to disambiguate tag");

    let result = resolve(&backend, &query, &ReleaseFilter::Unrestricted)
        .await
        .expect("Failed to identify tag");

    // Verify it's a Tag result
    match result {
        IdentifiedThing::Tag(tag_result) => {
            assert_eq!(tag_result.tag_info.name, "v1.0.0");
            assert_eq!(
                tag_result.tag_info.commit_hash,
                test_repo.commits.commit1_add_file
            );
            assert!(tag_result.tag_info.is_semver());

            let semver = tag_result
                .tag_info
                .semver_info
                .expect("Should have semver info");
            assert_eq!(semver.major, 1);
            assert_eq!(semver.minor, 0);
            assert_eq!(semver.patch, Some(0));
        }
        _ => panic!("Expected Tag result, got something else"),
    }
}

/// Test that nonexistent input returns error
#[rstest]
#[tokio::test]
async fn test_identify_nonexistent(test_repo: TestRepoFixture) {
    let backend = GitBackend::new(test_repo.repo);
    let result = backend
        .disambiguate_query(&ParsedQuery::Unknown("nonexistent-thing".to_string()))
        .await;
    assert!(result.is_err());
}

/// Test finding previous tag for a semver release
#[rstest]
#[tokio::test]
async fn test_find_previous_tag() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let repo_path = temp_dir.path().to_path_buf();

    // Setup git repo with multiple semver tags
    {
        let repo = git2::Repository::init(&repo_path).expect("init repo");
        let signature = git2::Signature::now("Test User", "test@example.com").expect("signature");

        // Create initial commit with v1.0.0 tag
        let file = repo_path.join("file.txt");
        std::fs::write(&file, "v1").expect("write");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("file.txt")).expect("add");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree lookup");
        let commit1 = repo
            .commit(Some("HEAD"), &signature, &signature, "v1.0.0", &tree, &[])
            .expect("commit");
        let commit1_obj = repo.find_commit(commit1).expect("find commit");
        repo.tag_lightweight("v1.0.0", commit1_obj.as_object(), false)
            .expect("tag");

        // Create second commit with v1.1.0 tag
        std::fs::write(&file, "v1.1").expect("write");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("file.txt")).expect("add");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree lookup");
        let commit2 = repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "v1.1.0",
                &tree,
                &[&commit1_obj],
            )
            .expect("commit");
        let commit2_obj = repo.find_commit(commit2).expect("find commit");
        repo.tag_lightweight("v1.1.0", commit2_obj.as_object(), false)
            .expect("tag");

        // Create third commit with v2.0.0 tag
        std::fs::write(&file, "v2").expect("write");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("file.txt")).expect("add");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree lookup");
        let commit3 = repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "v2.0.0",
                &tree,
                &[&commit2_obj],
            )
            .expect("commit");
        let commit3_obj = repo.find_commit(commit3).expect("find commit");
        repo.tag_lightweight("v2.0.0", commit3_obj.as_object(), false)
            .expect("tag");

        // Create non-semver tag
        repo.tag_lightweight("beta-release", commit3_obj.as_object(), false)
            .expect("tag");
    }

    let repo = wtg_cli::git::GitRepo::from_path(&repo_path).expect("open repo");
    let backend = GitBackend::new(repo);

    // v1.0.0 is the first semver tag, should have no previous
    let prev = backend.find_previous_tag("v1.0.0").await.unwrap();
    assert!(prev.is_none(), "v1.0.0 should have no previous semver tag");

    // v1.1.0 should have v1.0.0 as previous
    let prev = backend.find_previous_tag("v1.1.0").await.unwrap();
    assert_eq!(prev.as_ref().map(|t| t.name.as_str()), Some("v1.0.0"));

    // v2.0.0 should have v1.1.0 as previous
    let prev = backend.find_previous_tag("v2.0.0").await.unwrap();
    assert_eq!(prev.as_ref().map(|t| t.name.as_str()), Some("v1.1.0"));

    // Non-semver tag should return None
    let prev = backend.find_previous_tag("beta-release").await.unwrap();
    assert!(prev.is_none(), "non-semver tag should have no previous");
}

#[rstest]
#[tokio::test]
async fn disambiguates_branch_paths_with_slashes() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let repo_path = temp_dir.path().to_path_buf();

    // Setup git repo in a block so git2 types are dropped before async calls
    {
        let repo = git2::Repository::init(&repo_path).expect("init repo");
        let signature = git2::Signature::now("Test User", "test@example.com").expect("signature");
        let initial_file = repo_path.join("root.txt");
        std::fs::write(&initial_file, "root").expect("write file");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("root.txt")).expect("add path");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree lookup");
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .expect("commit");

        let branch_name = "some/path";
        let target_commit = repo.head().expect("head").peel_to_commit().expect("commit");
        repo.branch(branch_name, &target_commit, true)
            .expect("branch");

        let docs_dir = repo_path.join("docs");
        std::fs::create_dir_all(&docs_dir).expect("create docs dir");
        std::fs::write(docs_dir.join("guide.md"), "docs").expect("write docs");
        let mut index = repo.index().expect("index");
        index
            .add_path(Path::new("docs/guide.md"))
            .expect("add docs");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree lookup");
        let parent = repo.head().expect("head").peel_to_commit().expect("commit");
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "add docs",
            &tree,
            &[&parent],
        )
        .expect("commit docs");
        let updated_commit = repo.head().expect("head").peel_to_commit().expect("commit");
        repo.branch(branch_name, &updated_commit, true)
            .expect("branch update");
    }

    let repo = wtg_cli::git::GitRepo::from_path(&repo_path).expect("open repo");
    let backend = GitBackend::new(repo);
    let query = backend
        .disambiguate_query(&ParsedQuery::UnknownPath {
            segments: vec![
                "some".to_string(),
                "path".to_string(),
                "docs".to_string(),
                "guide.md".to_string(),
            ],
        })
        .await
        .expect("disambiguate path");

    assert!(matches!(
        query,
        Query::FilePath { branch, path }
            if branch == "some/path" && path == Path::new("docs/guide.md")
    ));
}
