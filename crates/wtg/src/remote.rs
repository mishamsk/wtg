//! Remote type definitions for git repository remotes.

use url::Url;

/// The hosting platform for a git remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RemoteHost {
    GitHub,
    GitLab,
    Bitbucket,
}

impl RemoteHost {
    /// Detect host from a remote URL (HTTP/HTTPS or SSH).
    /// Uses proper URL parsing, not string containment.
    #[must_use]
    pub fn from_url(url: &str) -> Option<Self> {
        // Try SSH format first: git@host:path
        if let Some(host) = Self::parse_ssh_host(url) {
            return Self::from_host_str(&host);
        }

        // Try HTTP/HTTPS URL parsing
        if let Some(host) = Self::parse_http_host(url) {
            return Self::from_host_str(&host);
        }

        None
    }

    /// Parse SSH URL format: `git@host:path` or `ssh://git@host/path`
    fn parse_ssh_host(url: &str) -> Option<String> {
        let trimmed = url.trim();

        // Handle ssh:// scheme
        if trimmed.starts_with("ssh://")
            && let Ok(parsed) = Url::parse(trimmed)
        {
            return parsed.host_str().map(str::to_string);
        }

        // Handle git@host:path format
        if let Some(after_at) = trimmed.strip_prefix("git@") {
            let host = after_at.split(':').next()?;
            return Some(host.to_string());
        }

        None
    }

    /// Parse HTTP/HTTPS URL and extract host
    fn parse_http_host(url: &str) -> Option<String> {
        // Try direct parse
        if let Ok(parsed) = Url::parse(url) {
            return parsed.host_str().map(str::to_string);
        }

        // Try with https:// prefix (handles "github.com/..." format)
        let prefixed = if url.starts_with("//") {
            format!("https:{url}")
        } else if !url.contains("://") {
            format!("https://{url}")
        } else {
            return None;
        };

        Url::parse(&prefixed).ok()?.host_str().map(str::to_string)
    }

    /// Map a host string to a `RemoteHost`
    fn from_host_str(host: &str) -> Option<Self> {
        let normalized = host.trim_start_matches("www.").to_ascii_lowercase();

        if normalized == "github.com" || normalized == "api.github.com" {
            Some(Self::GitHub)
        } else if normalized == "gitlab.com" || normalized.ends_with(".gitlab.com") {
            Some(Self::GitLab)
        } else if normalized == "bitbucket.org" || normalized.ends_with(".bitbucket.org") {
            Some(Self::Bitbucket)
        } else {
            None
        }
    }
}

/// Which named remote this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RemoteKind {
    Upstream, // Highest priority (canonical repo in fork workflows)
    Origin,   // Second priority
    Other,    // Lowest priority
}

impl RemoteKind {
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "origin" => Self::Origin,
            "upstream" => Self::Upstream,
            _ => Self::Other,
        }
    }
}

/// Information about a git remote.
#[derive(Debug, Clone)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
    pub kind: RemoteKind,
    pub host: Option<RemoteHost>,
}

impl RemoteInfo {
    /// Priority for sorting (lower = higher priority).
    /// Upstream < Origin < Other, within same kind: GitHub first.
    #[must_use]
    pub fn priority(&self) -> (RemoteKind, bool) {
        (self.kind, self.host != Some(RemoteHost::GitHub))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_host_from_https_url() {
        assert_eq!(
            RemoteHost::from_url("https://github.com/owner/repo"),
            Some(RemoteHost::GitHub)
        );
        assert_eq!(
            RemoteHost::from_url("https://github.com/owner/repo.git"),
            Some(RemoteHost::GitHub)
        );
        assert_eq!(
            RemoteHost::from_url("https://www.github.com/owner/repo"),
            Some(RemoteHost::GitHub)
        );
        assert_eq!(
            RemoteHost::from_url("https://api.github.com/repos/owner/repo"),
            Some(RemoteHost::GitHub)
        );
    }

    #[test]
    fn test_remote_host_from_ssh_url() {
        assert_eq!(
            RemoteHost::from_url("git@github.com:owner/repo.git"),
            Some(RemoteHost::GitHub)
        );
        assert_eq!(
            RemoteHost::from_url("git@gitlab.com:owner/repo.git"),
            Some(RemoteHost::GitLab)
        );
        assert_eq!(
            RemoteHost::from_url("git@bitbucket.org:owner/repo.git"),
            Some(RemoteHost::Bitbucket)
        );
    }

    #[test]
    fn test_remote_host_from_ssh_scheme_url() {
        assert_eq!(
            RemoteHost::from_url("ssh://git@github.com/owner/repo.git"),
            Some(RemoteHost::GitHub)
        );
    }

    #[test]
    fn test_remote_host_gitlab_variants() {
        assert_eq!(
            RemoteHost::from_url("https://gitlab.com/owner/repo"),
            Some(RemoteHost::GitLab)
        );
        assert_eq!(
            RemoteHost::from_url("https://gitlab.example.gitlab.com/owner/repo"),
            Some(RemoteHost::GitLab)
        );
    }

    #[test]
    fn test_remote_host_bitbucket_variants() {
        assert_eq!(
            RemoteHost::from_url("https://bitbucket.org/owner/repo"),
            Some(RemoteHost::Bitbucket)
        );
    }

    #[test]
    fn test_remote_host_unknown() {
        assert_eq!(RemoteHost::from_url("https://example.com/owner/repo"), None);
        assert_eq!(RemoteHost::from_url("git@example.com:owner/repo.git"), None);
    }

    #[test]
    fn test_remote_kind_from_name() {
        assert_eq!(RemoteKind::from_name("origin"), RemoteKind::Origin);
        assert_eq!(RemoteKind::from_name("upstream"), RemoteKind::Upstream);
        assert_eq!(RemoteKind::from_name("fork"), RemoteKind::Other);
        assert_eq!(RemoteKind::from_name("backup"), RemoteKind::Other);
    }

    #[test]
    fn test_remote_kind_ordering() {
        // Upstream has highest priority (lowest value)
        assert!(RemoteKind::Upstream < RemoteKind::Origin);
        assert!(RemoteKind::Origin < RemoteKind::Other);
    }

    #[test]
    fn test_remote_info_priority() {
        let github_origin = RemoteInfo {
            name: "origin".to_string(),
            url: "https://github.com/owner/repo".to_string(),
            kind: RemoteKind::Origin,
            host: Some(RemoteHost::GitHub),
        };

        let gitlab_origin = RemoteInfo {
            name: "origin".to_string(),
            url: "https://gitlab.com/owner/repo".to_string(),
            kind: RemoteKind::Origin,
            host: Some(RemoteHost::GitLab),
        };

        let github_upstream = RemoteInfo {
            name: "upstream".to_string(),
            url: "https://github.com/owner/repo".to_string(),
            kind: RemoteKind::Upstream,
            host: Some(RemoteHost::GitHub),
        };

        // Upstream beats origin (canonical repo in fork workflows)
        assert!(github_upstream.priority() < github_origin.priority());

        // Within same kind, GitHub beats non-GitHub
        assert!(github_origin.priority() < gitlab_origin.priority());

        // GitHub upstream beats non-GitHub origin
        assert!(github_upstream.priority() < gitlab_origin.priority());
    }
}
