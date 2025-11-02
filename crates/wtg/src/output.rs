use crate::error::Result;
use crate::identifier::IdentifiedThing;
use crossterm::style::Stylize;

pub fn display(thing: IdentifiedThing) -> Result<()> {
    match thing {
        IdentifiedThing::Commit {
            info,
            release,
            github_url,
            author_url,
        } => display_commit(info, release, github_url, author_url),

        IdentifiedThing::File {
            info,
            release,
            github_url,
            author_urls,
        } => display_file(info, release, github_url, author_urls),

        IdentifiedThing::Issue { info, release } => display_issue(info, release),

        IdentifiedThing::Tag { info, github_url } => display_tag(info, github_url),
    }

    Ok(())
}

// Helper functions for consistent formatting

/// Print a clickable URL with consistent styling
fn print_link(url: &str) {
    println!("   {} {}", "🔗".blue(), url.blue().underlined());
}

/// Print author information with optional profile URL
fn print_author_with_profile(name: &str, email: &str, profile_url: Option<&str>) {
    println!(
        "   {} {} ({})",
        "👤".yellow(),
        name.cyan(),
        email.dark_grey()
    );

    if let Some(url) = profile_url {
        print_link(url);
    }
}

/// Print a single commit summary line with optional URL
fn print_commit_summary(
    short_hash: &str,
    author: &str,
    date: &str,
    message: &str,
    commit_url: Option<&str>,
) {
    println!(
        "   {} by {} on {}",
        short_hash.cyan(),
        author.cyan(),
        date.dark_grey()
    );
    println!("   {} {}", "📝".yellow(), message.white());

    if let Some(url) = commit_url {
        print_link(url);
    }
}

fn display_commit(
    info: crate::git::CommitInfo,
    release: Option<crate::git::TagInfo>,
    github_url: Option<String>,
    author_url: Option<String>,
) {
    println!(
        "{} {}",
        "🔍 Found commit:".green().bold(),
        info.short_hash.cyan()
    );
    println!();

    // Commit message
    println!("{} {}", "📝".yellow(), info.message.white().bold());
    println!("   {} {}", "📅".yellow(), info.date.dark_grey());

    // Snarky comment if multi-line commit message
    if info.message_lines > 1 {
        let extra_lines = info.message_lines - 1;
        println!(
            "   {} {}",
            "📚".yellow(),
            format!(
                "Someone likes to write essays... {} more line{}",
                extra_lines,
                if extra_lines == 1 { "" } else { "s" }
            )
            .dark_grey()
            .italic()
        );
    }

    println!();

    // Who to blame
    println!("{}", "👎 Who's to blame for this pesky bug:".red().bold());
    print_author_with_profile(&info.author_name, &info.author_email, author_url.as_deref());

    println!();

    // Release info
    display_release_info(release, github_url.as_deref());

    // Commit link
    if let Some(url) = github_url {
        println!();
        print_link(&url);
    }
}

fn display_file(
    info: crate::git::FileInfo,
    release: Option<crate::git::TagInfo>,
    github_url: Option<String>,
    author_urls: Vec<Option<String>>,
) {
    println!("{} {}", "📄 Found file:".green().bold(), info.path.cyan());
    println!();

    // Last touched
    println!("{}", "🕐 Last touched:".yellow().bold());
    print_commit_summary(
        &info.last_commit.short_hash,
        &info.last_commit.author_name,
        &info.last_commit.date,
        &info.last_commit.message,
        github_url.as_deref(),
    );

    println!();

    // Previous authors
    if !info.previous_authors.is_empty() {
        println!("{}", "📜 Previous blame (up to 4):".yellow().bold());

        for (idx, (hash, name, _email)) in info.previous_authors.iter().enumerate() {
            print!(
                "   {}. {} - {}",
                idx + 1,
                hash.as_str().cyan(),
                name.as_str().cyan()
            );

            if let Some(Some(url)) = author_urls.get(idx) {
                print!(" {}", format!("({url})").blue().underlined());
            }

            println!();
        }

        println!();
    }

    // Release info
    display_release_info(release, None);
}

fn display_issue(info: crate::github::IssueInfo, release: Option<crate::git::TagInfo>) {
    let emoji = if info.is_pr { "🔀" } else { "🐛" };
    let type_str = if info.is_pr { "PR" } else { "Issue" };

    println!(
        "{} #{}: {}",
        format!("{emoji} Found {type_str}").green().bold(),
        info.number.to_string().cyan(),
        info.title.white().bold()
    );
    println!();

    // Author info
    if let Some(author) = &info.author {
        print!("   {} {}", "👤".yellow(), author.as_str().cyan());
        if let Some(url) = &info.author_url {
            print!(" {}", format!("({url})").blue().underlined());
        }
        println!();
    }

    print_link(&info.url);
    println!();

    // For PRs, show merge commit
    if info.is_pr {
        if let Some(merge_sha) = &info.merge_commit_sha {
            println!("{}", "✅ Merged:".green().bold());
            println!("   {} {}", "Merge commit:".yellow(), merge_sha[..7].cyan());
        } else {
            println!(
                "{}",
                "❌ Not merged yet - still open or closed without merging"
                    .yellow()
                    .italic()
            );
        }
        println!();
    } else {
        // For issues, show closing PRs and commits if any
        if info.closing_prs.is_empty() && info.closing_commits.is_empty() {
            println!(
                "{}",
                "🤷 No commits claimed to fix this... suspicious!"
                    .yellow()
                    .italic()
            );
        } else {
            println!("{}", "✅ Closed by:".green().bold());

            // Show closing PRs with links
            if !info.closing_prs.is_empty() {
                // Derive base URL from issue URL
                // Issue URL format: https://github.com/owner/repo/issues/123
                if let Some(base_url) = info.url.rsplit_once("/issues/").map(|(base, _)| base) {
                    for pr_number in &info.closing_prs {
                        println!("   {} PR #{}", "🔀".yellow(), pr_number.to_string().as_str().cyan().bold());
                        let pr_url = format!("{base_url}/pull/{pr_number}");
                        print_link(&pr_url);
                    }
                }
            }

            // Show closing commits
            for commit in &info.closing_commits {
                println!("   {} {}", "•".yellow(), commit[..7].cyan());
            }
        }
        println!();
    }

    // Release info
    display_release_info(release, None);
}

fn display_tag(info: crate::git::TagInfo, github_url: Option<String>) {
    println!("{} {}", "🏷️  Found tag:".green().bold(), info.name.cyan());
    println!();

    if info.is_semver {
        println!("   {} This looks like a release! 🎉", "✓".green());
    } else {
        println!("   {} Not a semver tag", "ℹ".blue());
    }

    println!();
    println!("   {} {}", "Commit:".yellow(), info.commit_hash[..7].cyan());

    if let Some(url) = github_url {
        println!();
        print_link(&url);
    }
}

fn display_release_info(release: Option<crate::git::TagInfo>, commit_url: Option<&str>) {
    println!("{}", "📦 First shipped in:".magenta().bold());

    match release {
        Some(tag) => {
            // Display tag name (or release name if it's a GitHub release)
            if tag.is_release {
                if let Some(release_name) = &tag.release_name {
                    println!(
                        "   {} {} {}",
                        "🎉".yellow(),
                        release_name.as_str().cyan().bold(),
                        format!("({})", tag.name).as_str().dark_grey()
                    );
                } else {
                    println!("   {} {}", "🎉".yellow(), tag.name.as_str().cyan().bold());
                }

                // Show published date if available
                if let Some(published) = &tag.published_at {
                    // Parse and format the date more nicely
                    if let Some(date_part) = published.split('T').next() {
                        println!("   {} {}", "📅".dark_grey(), date_part.dark_grey());
                    }
                }

                // Use the release URL if available
                if let Some(url) = &tag.release_url {
                    print_link(url);
                }
            } else {
                // Plain git tag
                println!("   {} {}", "🏷️ ".yellow(), tag.name.as_str().cyan().bold());

                // Build GitHub URLs if we have a commit URL
                if let Some(url) = commit_url {
                    // Extract owner/repo from commit URL
                    if let Some((base_url, _)) = url.rsplit_once("/commit/") {
                        let tag_url = format!("{}/tree/{}", base_url, tag.name);
                        print_link(&tag_url);
                    }
                }
            }
        }
        None => {
            println!(
                "   {}",
                "🔥 Not shipped yet, still cooking in main!"
                    .yellow()
                    .italic()
            );
        }
    }
}
