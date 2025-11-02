use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "wtg",
    about = "What the git?! - Because sometimes you just need to know what the git is going on.",
    long_about = "A snarky but helpful tool to identify git commits, issues, PRs and files changes, \
                  and tell you which release they shipped in. Because sometimes you just need to know \
                  what the git is going on."
)]
pub struct Cli {
    /// The thing to identify (commit hash, issue number, PR number, file path, or tag)
    #[arg(value_name = "INPUT")]
    pub input: String,
}
