use crossterm::style::Stylize;
use std::fmt;

pub type Result<T> = std::result::Result<T, WtgError>;

#[derive(Debug)]
pub enum WtgError {
    NotInGitRepo,
    NotFound(String),
    Git(git2::Error),
    GitHub(octocrab::Error),
    #[allow(dead_code)] // Will be used for network error handling
    NetworkUnavailable,
    MultipleMatches(Vec<String>),
    Io(std::io::Error),
}

impl fmt::Display for WtgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInGitRepo => {
                writeln!(
                    f,
                    "{}",
                    "❌ What the git are you asking me to do?".red().bold()
                )?;
                writeln!(f, "   {}", "This isn't even a git repository! 😱".red())
            }
            Self::NotFound(input) => {
                writeln!(
                    f,
                    "{}",
                    "🤔 Couldn't find this anywhere - are you sure you didn't make it up?"
                        .yellow()
                        .bold()
                )?;
                writeln!(f)?;
                writeln!(f, "   {}", "Tried:".yellow())?;
                writeln!(f, "   {} Commit hash (local + remote)", "❌".red())?;
                writeln!(f, "   {} GitHub issue/PR", "❌".red())?;
                writeln!(f, "   {} File in repo", "❌".red())?;
                writeln!(f, "   {} Git tag", "❌".red())?;
                writeln!(f)?;
                writeln!(f, "   {}: {}", "Input was".yellow(), input.as_str().cyan())
            }
            Self::Git(e) => write!(f, "Git error: {e}"),
            Self::GitHub(e) => write!(f, "GitHub error: {e}"),
            Self::NetworkUnavailable => {
                writeln!(
                    f,
                    "{}",
                    "🌐 Network is MIA - this might be an issue, might be your imagination."
                        .yellow()
                )?;
                writeln!(f, "   {}", "Can't reach GitHub to confirm.".yellow())
            }
            Self::MultipleMatches(types) => {
                writeln!(f, "{}", "💥 OH MY, YOU BLEW ME UP!".red().bold())?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "This matches EVERYTHING and I don't know what to do! 🤯".red()
                )?;
                writeln!(f)?;
                writeln!(f, "   {}", "Matches:".yellow())?;
                for t in types {
                    writeln!(f, "   {} {}", "✓".green(), t)?;
                }
                panic!("💥 BOOM! You broke me!");
            }
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for WtgError {}

impl From<git2::Error> for WtgError {
    fn from(err: git2::Error) -> Self {
        Self::Git(err)
    }
}

impl From<octocrab::Error> for WtgError {
    fn from(err: octocrab::Error) -> Self {
        Self::GitHub(err)
    }
}

impl From<std::io::Error> for WtgError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}
