use crossterm::style::Stylize;
use std::fmt;

pub type Result<T> = std::result::Result<T, WtgError>;

/// Check if an octocrab error is a rate limit error
fn is_rate_limit_error(err: &octocrab::Error) -> bool {
    if let octocrab::Error::GitHub { source, .. } = err {
        // HTTP 403 with rate limit message, or HTTP 429 (secondary rate limit)
        let status = source.status_code.as_u16();
        (status == 403
            && (source.message.contains("rate limit") || source.message.contains("API rate limit")))
            || status == 429
    } else {
        false
    }
}

#[derive(Debug)]
pub enum WtgError {
    NotInGitRepo,
    NotFound(String),
    Git(git2::Error),
    GitHub(octocrab::Error),
    MultipleMatches(Vec<String>),
    Io(std::io::Error),
    Cli { message: String, code: i32 },
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
            Self::GitHub(e) => {
                if is_rate_limit_error(e) {
                    writeln!(
                        f,
                        "{}",
                        "⏱️  Whoa there, speed demon! GitHub says you're moving too fast."
                            .yellow()
                            .bold()
                    )?;
                    writeln!(f)?;
                    writeln!(
                        f,
                        "   {}",
                        "You've hit the rate limit. Maybe take a coffee break? ☕".yellow()
                    )?;
                    writeln!(
                        f,
                        "   {}",
                        "Or set a GITHUB_TOKEN to get higher limits.".yellow()
                    )
                } else {
                    write!(f, "GitHub error: {e}")
                }
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
            Self::Cli { message, .. } => write!(f, "{message}"),
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

impl WtgError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Cli { code, .. } => *code,
            _ => 1,
        }
    }
}
