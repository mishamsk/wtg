use crossterm::style::Stylize;

use crate::constants;

/// Display custom help message when no input is provided
pub fn display_help() {
    let version = env!("CARGO_PKG_VERSION");

    println!(
        r"{title}
{version}

{tagline}

{usage_header}
  {cmd} {examples}

{what_header}
  {bullet} Throw anything at me: commits, issues, PRs, files, or tags
  {bullet} I'll figure out what you mean and show you the juicy details
  {bullet} Including who to blame and which release shipped it

{examples_header}
  {cmd} c62bbcc         {dim}# Find commit info
  {cmd} 123             {dim}# Look up issue or PR
  {cmd} Cargo.toml      {dim}# Check file history
  {cmd} v1.2.3          {dim}# Inspect a release tag
",
        title = format!("{} What The Git?! {}", "🔍", "🔍").green().bold(),
        version = format!("v{version}").dark_grey(),
        tagline = constants::DESCRIPTION.to_string().dark_grey().italic(),
        usage_header = "USAGE".cyan().bold(),
        cmd = "wtg".cyan(),
        examples = "<COMMIT|ISSUE|FILE|TAG>".yellow(),
        what_header = "WHAT I DO".cyan().bold(),
        bullet = "→",
        examples_header = "EXAMPLES".cyan().bold(),
        dim = "".dark_grey(),
    );
}
