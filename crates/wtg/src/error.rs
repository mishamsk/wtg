use crossterm::style::Stylize;
use http::StatusCode;
use octocrab::Error as OctoError;
use std::fmt;

pub type WtgResult<T> = std::result::Result<T, WtgError>;

#[derive(Debug, strum::EnumIs)]
pub enum WtgError {
    EmptyInput,
    NotInGitRepo,
    NotFound(String),
    TagNotFound(String),
    Unsupported(String),
    Git(git2::Error),
    GhConnectionLost,
    GhRateLimit(OctoError),
    GhSaml(OctoError),
    GitHub(OctoError),
    MultipleMatches(Vec<String>),
    Io(std::io::Error),
    Cli { message: String, code: i32 },
    Timeout,
    NotGitHubUrl(String),
    MalformedGitHubUrl(String),
    SecurityRejection(String),
    GitHubClientFailed,
}

impl fmt::Display for WtgError {
    #[allow(clippy::too_many_lines)]
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
            Self::TagNotFound(tag_name) => {
                writeln!(
                    f,
                    "{}",
                    "🏷️  Tag not found! Never heard of it.".yellow().bold()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}: {}",
                    "Looking for".yellow(),
                    tag_name.as_str().cyan()
                )?;
                writeln!(f)?;
                writeln!(f, "   {}", "Check your spelling! 🔍".yellow())
            }
            Self::Unsupported(operation) => {
                writeln!(f, "{}", "🚫 Can't do that here!".yellow().bold())?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {} is not supported with the current backend.",
                    operation.as_str().cyan()
                )
            }
            Self::Git(e) => write!(f, "Git error: {e}"),
            Self::GhConnectionLost => {
                writeln!(
                    f,
                    "{}",
                    "📡 Houston, we have a problem! Connection lost mid-flight!"
                        .red()
                        .bold()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "GitHub was there a second ago, now it's playing hide and seek. 👻".red()
                )?;
                writeln!(
                    f,
                    "   {}",
                    "Check your internet connection and try again!".yellow()
                )
            }
            Self::GhRateLimit(_) => {
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
            }
            Self::GhSaml(_) => {
                writeln!(
                    f,
                    "{}",
                    "🔐 Halt! Who goes there? Your GitHub org wants to see some ID!"
                        .red()
                        .bold()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "Looks like SAML SSO is standing between you and your data. 🚧".red()
                )?;
                writeln!(
                    f,
                    "   {}",
                    "Try authenticating your GITHUB_TOKEN with SAML first!".red()
                )
            }
            Self::GitHub(e) => write!(f, "GitHub error: {e}"),
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
            Self::Timeout => {
                writeln!(
                    f,
                    "{}",
                    "⏰ Time's up! The internet took a nap.".red().bold()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "Did you forget to pay your internet bill? 💸".red()
                )
            }
            Self::NotGitHubUrl(url) => {
                writeln!(
                    f,
                    "{}",
                    "🤨 That's a URL alright, but it's not GitHub!"
                        .yellow()
                        .bold()
                )?;
                writeln!(f)?;
                writeln!(f, "   {}: {}", "You gave me".yellow(), url.clone().cyan())?;
                writeln!(f)?;
                writeln!(f, "   {}", "I only speak GitHub URLs, buddy! 🐙".yellow())
            }
            Self::MalformedGitHubUrl(url) => {
                writeln!(
                    f,
                    "{}",
                    "😵 That GitHub URL is more broken than my ex's promises!"
                        .red()
                        .bold()
                )?;
                writeln!(f)?;
                writeln!(f, "   {}: {}", "You gave me".red(), url.clone().cyan())?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "Expected something like: https://github.com/owner/repo/issues/123".yellow()
                )?;
                writeln!(f, "   {}", "But this? This is just sad. 😢".red())
            }
            Self::SecurityRejection(reason) => {
                writeln!(f, "{}", "🚨 Whoa there! Security alert!".red().bold())?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "I can't process that input for personal reasons. 🛡️".red()
                )?;
                writeln!(f)?;
                writeln!(f, "   {}: {}", "Reason".yellow(), reason.clone())?;
                writeln!(f)?;
                writeln!(f, "   {}", "Please, try something safer? 🙏".yellow())
            }
            Self::EmptyInput => {
                writeln!(
                    f,
                    "{}",
                    "🫥 Excuse me, but I can't read minds!".yellow().bold()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "You gave me... nothing. Nada. Zilch. The void! 👻".yellow()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "Try giving me something to work with, please!".yellow()
                )
            }
            Self::GitHubClientFailed => {
                writeln!(
                    f,
                    "{}",
                    "🔑 Can't connect to GitHub! Something's blocking the path..."
                        .red()
                        .bold()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "You explicitly asked for GitHub data, but I can't reach it. 😞".red()
                )?;
                writeln!(f)?;
                writeln!(
                    f,
                    "   {}",
                    "Check your GITHUB_TOKEN and network connection!".yellow()
                )
            }
        }
    }
}

impl std::error::Error for WtgError {}

impl From<git2::Error> for WtgError {
    fn from(err: git2::Error) -> Self {
        Self::Git(err)
    }
}

impl From<OctoError> for WtgError {
    fn from(err: OctoError) -> Self {
        if let OctoError::GitHub { ref source, .. } = err {
            match source.status_code {
                StatusCode::TOO_MANY_REQUESTS => return Self::GhRateLimit(err),
                StatusCode::FORBIDDEN => {
                    let msg_lower = source.message.to_ascii_lowercase();

                    if msg_lower.to_ascii_lowercase().contains("saml") {
                        return Self::GhSaml(err);
                    }

                    if msg_lower.contains("rate limit") {
                        return Self::GhRateLimit(err);
                    }

                    return Self::GitHub(err);
                }
                _ => {
                    return Self::GitHub(err);
                }
            }
        }

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

/// Extension trait for logging errors before discarding them.
pub trait LogError<T> {
    /// Log the error at debug level and convert to Option.
    fn log_err(self, context: &str) -> Option<T>;
}

impl<T> LogError<T> for WtgResult<T> {
    fn log_err(self, context: &str) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                log::debug!("{context}: {e:?}");
                None
            }
        }
    }
}
