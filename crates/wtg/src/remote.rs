use crossterm::style::Stylize;

/// Check git remote status and print snarky messages
pub fn check_remote_and_snark(remote_info: Option<(String, String)>, repo_path: &std::path::Path) {
    match remote_info {
        Some((_owner, _repo)) => {
            // We have GitHub! All good
        }
        None => {
            // Check if there's any remote at all
            if let Ok(repo) = git2::Repository::open(repo_path)
                && let Ok(remotes) = repo.remotes()
            {
                if remotes.is_empty() {
                    println!(
                        "{}",
                        "🤐 No remotes configured - what are you hiding?"
                            .yellow()
                            .italic()
                    );
                    println!(
                        "{}",
                        "   (Or maybe... go do some OSS? 👀)".yellow().italic()
                    );
                    println!();
                } else {
                    // Has remotes but not GitHub
                    for remote_name in remotes.iter().flatten() {
                        if let Ok(remote) = repo.find_remote(remote_name)
                            && let Some(url) = remote.url()
                        {
                            if url.contains("gitlab") {
                                println!(
                                    "{}",
                                    "🦊 GitLab spotted! Living that self-hosted life, I see..."
                                        .yellow()
                                        .italic()
                                );
                            } else if url.contains("bitbucket") {
                                println!(
                                    "{}",
                                    "🪣 Bitbucket, eh? Taking the scenic route!"
                                        .yellow()
                                        .italic()
                                );
                            } else if !url.contains("github") {
                                println!(
                                    "{}",
                                    "🌐 A custom git remote? Look at you being all independent!"
                                        .yellow()
                                        .italic()
                                );
                            }

                            println!(
                                        "{}",
                                        "   (I can only do GitHub API stuff, but let me show you local git info...)"
                                            .yellow()
                                            .italic()
                                    );
                            println!();
                            break;
                        }
                    }
                }
            }
        }
    }
}
