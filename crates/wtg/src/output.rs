use crate::error::Result;
use crate::identifier::{EnrichedInfo, EntryPoint, FileResult, IdentifiedThing};
use crossterm::style::Stylize;

pub fn display(thing: IdentifiedThing) -> Result<()> {
    match thing {
        IdentifiedThing::Enriched(info) => display_enriched(*info),
        IdentifiedThing::File(file_result) => display_file(*file_result),
        IdentifiedThing::TagOnly(tag_info, github_url) => {
            display_tag_warning(*tag_info, github_url);
        }
    }

    Ok(())
}

/// Display tag with humor - tags aren't supported yet
fn display_tag_warning(tag_info: crate::git::TagInfo, github_url: Option<String>) {
    println!(
        "{} {}",
        "🏷️  Found tag:".green().bold(),
        tag_info.name.cyan()
    );
    println!();
    println!("{}", "🐎 Whoa there, slow down cowboy!".yellow().bold());
    println!();
    println!(
        "   {}",
        "Tags aren't fully baked yet. I found it, but can't tell you much about it.".white()
    );
    println!(
        "   {}",
        "Come back when you have a commit hash, PR, or issue to look up!".white()
    );

    if let Some(url) = github_url {
        println!();
        print_link(&url);
    }
}

/// Display enriched info - the main display logic
/// Order depends on what the user searched for
fn display_enriched(info: EnrichedInfo) {
    match &info.entry_point {
        EntryPoint::IssueNumber(_) => {
            // User searched for issue - lead with issue
            display_identification(&info.entry_point);
            println!();

            if let Some(issue) = &info.issue {
                display_issue_section(issue);
                println!();
            }

            if let Some(pr) = &info.pr {
                display_pr_section(pr, true); // true = show as "the fix"
                println!();
            }

            if let Some(commit) = &info.commit {
                display_commit_section(
                    commit,
                    info.commit_url.as_ref(),
                    info.commit_author_github_url.as_ref(),
                    info.pr.as_ref(),
                );
                println!();
            }

            display_missing_info(&info);

            if info.commit.is_some() {
                display_release_info(info.release, info.commit_url.as_deref());
            }
        }
        EntryPoint::PullRequestNumber(_) => {
            // User searched for PR - lead with PR
            display_identification(&info.entry_point);
            println!();

            if let Some(pr) = &info.pr {
                display_pr_section(pr, false); // false = not a fix, just a PR
                println!();
            }

            if let Some(commit) = &info.commit {
                display_commit_section(
                    commit,
                    info.commit_url.as_ref(),
                    info.commit_author_github_url.as_ref(),
                    info.pr.as_ref(),
                );
                println!();
            }

            display_missing_info(&info);

            if info.commit.is_some() {
                display_release_info(info.release, info.commit_url.as_deref());
            }
        }
        _ => {
            // User searched for commit or something else - lead with commit
            display_identification(&info.entry_point);
            println!();

            if let Some(commit) = &info.commit {
                display_commit_section(
                    commit,
                    info.commit_url.as_ref(),
                    info.commit_author_github_url.as_ref(),
                    info.pr.as_ref(),
                );
                println!();
            }

            if let Some(pr) = &info.pr {
                display_pr_section(pr, false);
                println!();
            }

            if let Some(issue) = &info.issue {
                display_issue_section(issue);
                println!();
            }

            display_missing_info(&info);

            if info.commit.is_some() {
                display_release_info(info.release, info.commit_url.as_deref());
            }
        }
    }
}

/// Display what the user searched for
fn display_identification(entry_point: &EntryPoint) {
    match entry_point {
        EntryPoint::Commit(hash) => {
            println!(
                "{} {}",
                "🔍 Found commit:".green().bold(),
                hash.as_str().cyan()
            );
        }
        EntryPoint::PullRequestNumber(num) => {
            println!(
                "{} #{}",
                "🔀 Found PR:".green().bold(),
                num.to_string().cyan()
            );
        }
        EntryPoint::IssueNumber(num) => {
            println!(
                "{} #{}",
                "🐛 Found issue:".green().bold(),
                num.to_string().cyan()
            );
        }
        EntryPoint::FilePath(path) => {
            println!(
                "{} {}",
                "📄 Found file:".green().bold(),
                path.as_str().cyan()
            );
        }
        EntryPoint::Tag(tag) => {
            println!(
                "{} {}",
                "🏷️  Found tag:".green().bold(),
                tag.as_str().cyan()
            );
        }
    }
}

/// Display commit information (the core section, always present when resolved)
fn display_commit_section(
    commit: &crate::git::CommitInfo,
    commit_url: Option<&String>,
    author_url: Option<&String>,
    pr: Option<&crate::github::PullRequestInfo>,
) {
    println!("{}", "💻 The Commit:".cyan().bold());
    println!(
        "   {} {}",
        "Hash:".yellow(),
        commit.short_hash.as_str().cyan()
    );

    // Show commit author
    print_author_subsection(
        "Who wrote this gem:",
        &commit.author_name,
        &commit.author_email,
        author_url.map(String::as_str),
    );

    // Show commit message if not a PR
    if pr.is_none() {
        print_message_with_essay_joke(&commit.message, None, commit.message_lines);
    }

    println!("   {} {}", "📅".yellow(), commit.date.as_str().dark_grey());

    if let Some(url) = commit_url {
        print_link(url);
    }
}

/// Display PR information (enrichment layer 1)
fn display_pr_section(pr: &crate::github::PullRequestInfo, is_fix: bool) {
    println!("{}", "🔀 The Pull Request:".magenta().bold());
    println!(
        "   {} #{}",
        "Number:".yellow(),
        pr.number.to_string().cyan()
    );

    // PR author - different wording if this is shown as "the fix" for an issue
    if let Some(author) = &pr.author {
        let header = if is_fix {
            "Who's brave:"
        } else {
            "Who merged this beauty:"
        };
        print_author_subsection(header, author, "", pr.author_url.as_deref());
    }

    // PR description (overrides commit message)
    print_message_with_essay_joke(&pr.title, pr.body.as_deref(), pr.title.lines().count());

    // Merge status
    if let Some(merge_sha) = &pr.merge_commit_sha {
        println!("   {} {}", "✅ Merged:".green(), merge_sha[..7].cyan());
    } else {
        println!("   {}", "❌ Not merged yet".yellow().italic());
    }

    print_link(&pr.url);
}

/// Display issue information (enrichment layer 2)
fn display_issue_section(issue: &crate::github::IssueInfo) {
    println!("{}", "🐛 The Issue:".red().bold());
    println!(
        "   {} #{}",
        "Number:".yellow(),
        issue.number.to_string().cyan()
    );

    // Issue author
    if let Some(author) = &issue.author {
        print_author_subsection(
            "Who spotted the trouble:",
            author,
            "",
            issue.author_url.as_deref(),
        );
    }

    // Issue description
    print_message_with_essay_joke(
        &issue.title,
        issue.body.as_deref(),
        issue.title.lines().count(),
    );

    print_link(&issue.url);
}

/// Display missing information (graceful degradation)
fn display_missing_info(info: &EnrichedInfo) {
    // Issue without PR
    if let Some(issue) = info.issue.as_ref()
        && info.pr.is_none()
    {
        let message = if info.commit.is_none() {
            if issue.state == octocrab::models::IssueState::Closed {
                "🔍 Issue closed, but the trail's cold. Some stealthy hero dropped a fix and vanished without a PR."
            } else {
                "🔍 Couldn't trace this issue, still open. Waiting for a brave soul to pick it up..."
            }
        } else if issue.state == octocrab::models::IssueState::Closed {
            "🤷 Issue closed, but no PR found... Some stealthy hero dropped a fix and vanished without a PR."
        } else {
            "🤷 No PR found for this issue... still hunting for the fix!"
        };
        println!("{}", message.yellow().italic());
        println!();
    }

    // PR without commit (not merged)
    if info.pr.is_some() && info.commit.is_none() {
        println!(
            "{}",
            "⏳ This PR hasn't been merged yet, too scared to commit!"
                .yellow()
                .italic()
        );
        println!();
    }
}

// Helper functions for consistent formatting

/// Print a clickable URL with consistent styling
fn print_link(url: &str) {
    println!("   {} {}", "🔗".blue(), url.blue().underlined());
}

/// Print author information as a subsection (indented)
fn print_author_subsection(
    header: &str,
    name: &str,
    email_or_username: &str,
    profile_url: Option<&str>,
) {
    println!("   {} {}", "👤".yellow(), header.dark_grey());

    if email_or_username.is_empty() {
        println!("      {}", name.cyan());
    } else {
        println!("      {} ({})", name.cyan(), email_or_username.dark_grey());
    }

    if let Some(url) = profile_url {
        println!("      {} {}", "🔗".blue(), url.blue().underlined());
    }
}

/// Print a message/description with essay joke if it's long
fn print_message_with_essay_joke(first_line: &str, full_text: Option<&str>, line_count: usize) {
    println!("   {} {}", "📝".yellow(), first_line.white().bold());

    // Check if we should show the essay joke
    if let Some(text) = full_text {
        let char_count = text.len();

        // Show essay joke if >100 chars or multi-line
        if char_count > 100 || line_count > 1 {
            let extra_lines = line_count.saturating_sub(1);
            let message = if extra_lines > 0 {
                format!(
                    "Someone likes to write essays... {} more line{}",
                    extra_lines,
                    if extra_lines == 1 { "" } else { "s" }
                )
            } else {
                format!("Someone likes to write essays... {char_count} characters")
            };

            println!("      {} {}", "📚".yellow(), message.dark_grey().italic());
        }
    }
}

/// Display file information (special case)
fn display_file(file_result: FileResult) {
    let info = file_result.file_info;

    println!("{} {}", "📄 Found file:".green().bold(), info.path.cyan());
    println!();

    // Get the author URL for the last commit (first in the list)
    let last_commit_author_url = file_result.author_urls.first().and_then(|opt| opt.as_ref());

    // Display the commit section (consistent with PR/issue flow)
    display_commit_section(
        &info.last_commit,
        file_result.commit_url.as_ref(),
        last_commit_author_url,
        None, // Files don't have associated PRs
    );

    // Count how many times the last commit author appears in previous commits
    let last_author_name = &info.last_commit.author_name;
    let repeat_count = info
        .previous_authors
        .iter()
        .filter(|(_, name, _)| name == last_author_name)
        .count();

    // Add snarky comment if they're a repeat offender
    if repeat_count > 0 {
        let joke = match repeat_count {
            1 => format!(
                "   💀 {} can't stop touching this file... {} more time before this!",
                last_author_name.as_str().cyan(),
                repeat_count
            ),
            2 => format!(
                "   💀 {} really loves this file... {} more times before this!",
                last_author_name.as_str().cyan(),
                repeat_count
            ),
            3 => format!(
                "   💀 {} is obsessed... {} more times before this!",
                last_author_name.as_str().cyan(),
                repeat_count
            ),
            _ => format!(
                "   💀 {} REALLY needs to leave this alone... {} more times before this!",
                last_author_name.as_str().cyan(),
                repeat_count
            ),
        };
        println!("{}", joke.dark_grey().italic());
    }

    println!();

    // Previous authors - snarky hall of shame (deduplicated)
    if !info.previous_authors.is_empty() {
        // Deduplicate authors - track who we've seen
        let mut seen_authors = std::collections::HashSet::new();
        seen_authors.insert(last_author_name.clone()); // Skip the last commit author

        let unique_authors: Vec<_> = info
            .previous_authors
            .iter()
            .enumerate()
            .filter(|(_, (_, name, _))| seen_authors.insert(name.clone()))
            .collect();

        if !unique_authors.is_empty() {
            let count = unique_authors.len();
            let header = if count == 1 {
                "👻 One ghost from the past:"
            } else {
                "👻 The usual suspects (who else touched this):"
            };
            println!("{}", header.yellow().bold());

            for (original_idx, (hash, name, _email)) in unique_authors {
                print!("   → {} • {}", hash.as_str().cyan(), name.as_str().cyan());

                if let Some(Some(url)) = file_result.author_urls.get(original_idx) {
                    print!(" {} {}", "🔗".blue(), url.as_str().blue().underlined());
                }

                println!();
            }

            println!();
        }
    }

    // Release info
    display_release_info(file_result.release, file_result.commit_url.as_deref());
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
                if let Some(published) = &tag.published_at
                    && let Some(date_part) = published.split('T').next()
                {
                    println!("   {} {}", "📅".dark_grey(), date_part.dark_grey());
                }

                // Use the release URL if available
                if let Some(url) = &tag.release_url {
                    print_link(url);
                }
            } else {
                // Plain git tag
                println!("   {} {}", "🏷️ ".yellow(), tag.name.as_str().cyan().bold());

                // Build GitHub URLs if we have a commit URL
                if let Some(url) = commit_url
                    && let Some((base_url, _)) = url.rsplit_once("/commit/")
                {
                    let tag_url = format!("{base_url}/tree/{}", tag.name);
                    print_link(&tag_url);
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
