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
  {cmd} {examples} {release_arg}
  {cmd} -r {repo_url} {examples_with_r}

{options_header}
  {opt_r}              GitHub repository (e.g., owner/repo)
  {opt_fetch}             Fetch missing refs from remote when not found locally
  {opt_skip_pre}  Skip pre-release versions (nightlies, RCs, etc.)

{what_header}
  {bullet} Throw anything at me: commits, issues, PRs, files, or tags
  {bullet} I'll figure out what you mean and show you the juicy details
  {bullet} Including who to blame and which release shipped it
  {bullet} Works with local repos OR any GitHub repo (via -r flag)

{examples_header}
  {dim}# Local repository
  {cmd} c62bbcc                              {dim2}# Find commit info
  {cmd} 123                                  {dim2}# Look up issue or PR
  {cmd} Cargo.toml                           {dim2}# Check file history
  {cmd} v1.2.3                               {dim2}# Inspect a release tag

  {dim}# Check specific release
  {cmd} c62bbcc v2.0.0                       {dim2}# Is commit in v2.0.0?
  {cmd} 123 v2.0.0                           {dim2}# Is PR/issue fix in v2.0.0?
  {cmd} c62bbcc -S                           {dim2}# Skip pre-releases

  {dim}# Remote repository
  {cmd} -r owner/repo c62bbcc                {dim2}# Check commit in remote repo
  {cmd} -r https://github.com/owner/repo 123 {dim2}# Look up remote issue/PR

  {dim}# GitHub URLs (auto-detected)
  {cmd} https://github.com/owner/repo/commit/abc123
  {cmd} https://github.com/owner/repo/issues/42
  {cmd} https://github.com/owner/repo/pull/123
  {cmd} https://github.com/owner/repo/blob/main/src/file.rs
",
        title = format!("{} What The Git?! {}", "🔍", "🔍").green().bold(),
        version = format!("v{version}").dark_grey(),
        tagline = constants::DESCRIPTION.to_string().dark_grey().italic(),
        usage_header = "USAGE".cyan().bold(),
        cmd = "wtg".cyan(),
        examples = "<COMMIT|ISSUE|FILE|TAG|URL>".yellow(),
        examples_with_r = "<COMMIT|ISSUE|FILE|TAG>".yellow(),
        release_arg = "[RELEASE]".yellow(),
        repo_url = "<REPO_URL>".yellow(),
        options_header = "OPTIONS".cyan().bold(),
        opt_r = "-r, --repo".green(),
        opt_fetch = "    --fetch".green(),
        opt_skip_pre = "-S, --skip-prereleases".green(),
        what_header = "WHAT I DO".cyan().bold(),
        bullet = "→",
        examples_header = "EXAMPLES".cyan().bold(),
        dim = "".dark_grey(),
        dim2 = "".dark_grey(),
    );
}
