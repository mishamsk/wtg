use std::fs::File;
use std::io;
use std::path::PathBuf;
use tempfile::TempDir;
use wtg_cli::git::GitRepo;
use zip::ZipArchive;

/// Test fixture data containing the repo and expected commit hashes
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct TestRepoFixture {
    pub repo: GitRepo,
    pub commits: TestCommits,
    #[allow(dead_code)]
    temp_dir: TempDir, // Kept alive to prevent cleanup
}

/// Expected commit hashes in the test repository
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct TestCommits {
    pub commit0_initial: String,
    pub commit1_add_file: String,
    pub commit2_update_file: String,
}

/// Fixture that extracts the test repo zip to a temp directory.
/// Each test gets its own independent copy of the repository.
#[rstest::fixture]
pub fn test_repo() -> TestRepoFixture {
    // Get path to the zip file in fixtures
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let zip_path = manifest_dir.join("tests/fixtures/test-repo.zip");

    // Create temp directory
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let extract_path = temp_dir.path();

    // Extract zip
    extract_zip(&zip_path, extract_path).expect("Failed to extract test repo");

    // Open the repository
    let repo = GitRepo::from_path(extract_path).expect("Failed to open test repo");

    // Known commit hashes from the test repository
    let commits = TestCommits {
        commit0_initial: "1701b19f169012a64d194efa3b9ec2a3538c7964".to_string(),
        commit1_add_file: "e7d0328fcad38176b00995b7f763ef1e7c8cf365".to_string(),
        commit2_update_file: "f6f335876c56b42d0c7cecec8727244f9e5183fa".to_string(),
    };

    TestRepoFixture {
        repo,
        commits,
        temp_dir,
    }
}

/// Extract a zip archive to a directory
fn extract_zip(zip_path: &PathBuf, target_dir: &std::path::Path) -> io::Result<()> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = target_dir.join(file.name());

        if file.is_dir() {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;

            // Preserve Unix permissions if available
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = file.unix_mode() {
                    std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
    }

    Ok(())
}
