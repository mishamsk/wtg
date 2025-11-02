use crate::error::Result;
use crate::identifier::IdentifiedThing;
use colored::*;

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

fn display_commit(
    info: crate::git::CommitInfo,
    release: Option<crate::git::TagInfo>,
    github_url: Option<String>,
    author_url: Option<String>,
) {
    println!("{} {}", "🔍 Found commit:".green().bold(), info.short_hash.cyan());
    println!();

    // Commit message
    println!("{} {}", "📝".yellow(), info.message.white().bold());
    println!("   {} {}", "📅".yellow(), info.date.bright_black());

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
            .bright_black()
            .italic()
        );
    }

    println!();

    // Who to blame
    println!("{}", "👎 Who's to blame for this pesky bug:".red().bold());
    println!(
        "   {} {} ({})",
        "👤".yellow(),
        info.author_name.cyan(),
        info.author_email.bright_black()
    );

    if let Some(url) = author_url {
        println!("   {} {}", "🔗".blue(), url.blue().underline());
    }

    println!();

    // Release info
    display_release_info(release, github_url.as_deref());

    // Commit link
    if let Some(url) = github_url {
        println!();
        println!("{} {}", "🔗".blue(), url.blue().underline());
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
    println!(
        "   {} by {} on {}",
        info.last_commit.short_hash.cyan(),
        info.last_commit.author_name.cyan(),
        info.last_commit.date.bright_black()
    );
    println!("   {} {}", "📝".yellow(), info.last_commit.message.white());

    if let Some(url) = github_url {
        println!("   {} {}", "🔗".blue(), url.blue().underline());
    }

    println!();

    // Previous authors
    if !info.previous_authors.is_empty() {
        println!("{}", "📜 Previous blame (up to 4):".yellow().bold());

        for (idx, (hash, name, _email)) in info.previous_authors.iter().enumerate() {
            print!("   {}. {} - {}", idx + 1, hash.cyan(), name.cyan());

            if let Some(Some(url)) = author_urls.get(idx) {
                print!(" {}", format!("({})", url).blue().underline());
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
        format!("{} Found {}", emoji, type_str).green().bold(),
        info.number.to_string().cyan(),
        info.title.white().bold()
    );
    println!();

    // Author info
    if let Some(author) = &info.author {
        print!("   {} {}", "👤".yellow(), author.cyan());
        if let Some(url) = &info.author_url {
            print!(" {}", format!("({})", url).blue().underline());
        }
        println!();
    }

    println!("{} {}", "🔗".blue(), info.url.blue().underline());
    println!();

    // For PRs, show merge commit
    if info.is_pr {
        if let Some(merge_sha) = &info.merge_commit_sha {
            println!("{}", "✅ Merged:".green().bold());
            println!("   {} {}", "Merge commit:".yellow(), merge_sha[..7].cyan());
        } else {
            println!(
                "{}",
                "❌ Not merged yet - still open or closed without merging".yellow().italic()
            );
        }
        println!();
    } else {
        // For issues, show closing commits if any
        if info.closing_commits.is_empty() {
            println!(
                "{}",
                "🤷 No commits claimed to fix this... suspicious!".yellow().italic()
            );
        } else {
            println!("{}", "✅ Closed by:".green().bold());
            for commit in &info.closing_commits {
                println!("   {} {}", "•".yellow(), commit.cyan());
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
        println!("   {} {}", "🔗".blue(), url.blue().underline());
    }
}

fn display_release_info(release: Option<crate::git::TagInfo>, commit_url: Option<&str>) {
    println!("{}", "📦 First shipped in:".magenta().bold());

    match release {
        Some(tag) => {
            println!("   {} {}", "🏷️ ".yellow(), tag.name.cyan().bold());

            // Build GitHub URLs if we have a commit URL
            if let Some(url) = commit_url {
                // Extract owner/repo from commit URL
                if let Some((base_url, _)) = url.rsplit_once("/commit/") {
                    let release_url = format!("{}/releases/tag/{}", base_url, tag.name);
                    println!("   {} {}", "🔗".blue(), release_url.blue().underline());
                }
            }
        }
        None => {
            println!(
                "   {}",
                "🔥 Not shipped yet, still cooking in main!".yellow().italic()
            );
        }
    }
}
