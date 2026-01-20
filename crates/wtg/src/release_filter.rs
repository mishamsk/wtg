//! Release filtering configuration.
//!
//! This module provides the `ReleaseFilter` enum which controls which tags/releases
//! are considered when finding releases for a commit.

use crate::git::TagInfo;

/// Controls which tags/releases are considered when finding releases.
#[derive(Debug, Clone, Default)]
pub enum ReleaseFilter {
    /// All tags are considered (default behavior).
    #[default]
    Unrestricted,
    /// Filter out pre-release versions (nightlies, RCs, etc.).
    SkipPrereleases,
    /// Limit to one specific tag by name.
    Specific(String),
}

impl ReleaseFilter {
    /// Filter a list of `TagInfo` candidates.
    ///
    /// Returns a new vector containing only tags that pass the filter.
    #[must_use]
    pub fn filter_tags(&self, tags: Vec<TagInfo>) -> Vec<TagInfo> {
        match self {
            Self::Unrestricted => tags,
            Self::SkipPrereleases => tags
                .into_iter()
                .filter(|t| {
                    // Keep tags that are not semver (can't determine pre-release status)
                    // or are semver but not pre-releases
                    t.semver_info
                        .as_ref()
                        .is_none_or(|s| s.pre_release.is_none())
                })
                .collect(),
            Self::Specific(name) => tags.into_iter().filter(|t| t.name == *name).collect(),
        }
    }

    /// Check if this is a specific release filter and return the tag name if so.
    #[must_use]
    pub fn specific_tag(&self) -> Option<&str> {
        match self {
            Self::Specific(name) => Some(name),
            _ => None,
        }
    }

    /// Returns true if pre-releases should be skipped.
    #[must_use]
    pub const fn skips_prereleases(&self) -> bool {
        matches!(self, Self::SkipPrereleases)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semver::parse_semver;
    use chrono::Utc;

    fn make_tag(name: &str) -> TagInfo {
        TagInfo {
            name: name.to_string(),
            commit_hash: "abc123".to_string(),
            semver_info: parse_semver(name),
            created_at: Utc::now(),
            is_release: false,
            release_name: None,
            release_url: None,
            published_at: None,
            tag_url: None,
        }
    }

    #[test]
    fn unrestricted_keeps_all_tags() {
        let tags = vec![
            make_tag("v1.0.0"),
            make_tag("v1.1.0-beta.1"),
            make_tag("v2.0.0-rc.1"),
            make_tag("release-2024"),
        ];
        let filter = ReleaseFilter::Unrestricted;
        let result = filter.filter_tags(tags);
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn skip_prereleases_filters_correctly() {
        let tags = vec![
            make_tag("v1.0.0"),
            make_tag("v1.1.0-beta.1"),
            make_tag("v2.0.0-rc.1"),
            make_tag("release-2024"), // non-semver, kept
        ];
        let filter = ReleaseFilter::SkipPrereleases;
        let result = filter.filter_tags(tags);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|t| t.name == "v1.0.0"));
        assert!(result.iter().any(|t| t.name == "release-2024"));
    }

    #[test]
    fn specific_filters_to_one_tag() {
        let tags = vec![make_tag("v1.0.0"), make_tag("v1.1.0"), make_tag("v2.0.0")];
        let filter = ReleaseFilter::Specific("v1.1.0".to_string());
        let result = filter.filter_tags(tags);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "v1.1.0");
    }

    #[test]
    fn specific_returns_empty_if_not_found() {
        let tags = vec![make_tag("v1.0.0"), make_tag("v2.0.0")];
        let filter = ReleaseFilter::Specific("v99.0.0".to_string());
        let result = filter.filter_tags(tags);
        assert!(result.is_empty());
    }

    #[test]
    fn specific_tag_returns_name() {
        let filter = ReleaseFilter::Specific("v1.0.0".to_string());
        assert_eq!(filter.specific_tag(), Some("v1.0.0"));

        let filter = ReleaseFilter::Unrestricted;
        assert_eq!(filter.specific_tag(), None);

        let filter = ReleaseFilter::SkipPrereleases;
        assert_eq!(filter.specific_tag(), None);
    }

    #[test]
    fn skips_prereleases_helper() {
        assert!(!ReleaseFilter::Unrestricted.skips_prereleases());
        assert!(ReleaseFilter::SkipPrereleases.skips_prereleases());
        assert!(!ReleaseFilter::Specific("v1.0.0".to_string()).skips_prereleases());
    }
}
