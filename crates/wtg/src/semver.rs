//! Semantic version parsing for git tags.

use std::sync::LazyLock;

use regex::Regex;

/// Parsed semantic version information from a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemverInfo {
    pub major: u32,
    pub minor: u32,
    pub patch: Option<u32>,
    pub build: Option<u32>,
    pub pre_release: Option<String>,
    pub build_metadata: Option<String>,
}

impl Ord for SemverInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.major.cmp(&other.major) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.minor.cmp(&other.minor) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.patch.cmp(&other.patch) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.build.cmp(&other.build) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        // Pre-release: None (stable) > Some (pre-release)
        match (&self.pre_release, &other.pre_release) {
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
            (None, None) => std::cmp::Ordering::Equal,
        }
    }
}

impl PartialOrd for SemverInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Regex for parsing semantic version tags.
/// Supports:
/// - Optional prefix: py-, rust-, python-, etc.
/// - Optional 'v' prefix
/// - Version: X.Y, X.Y.Z, X.Y.Z.W
/// - Pre-release: -alpha, -beta.1, -rc.1 (dash style) OR a1, b1, rc1 (Python style)
/// - Build metadata: +build.123
static SEMVER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[a-z]+-)?v?(\d+)\.(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:(?:-([a-zA-Z0-9.-]+))|(?:([a-z]+)(\d+)))?(?:\+(.+))?$"
    )
    .expect("Invalid semver regex")
});

/// Parse a semantic version string
/// Supports:
/// - 2-part: 1.0
/// - 3-part: 1.2.3
/// - 4-part: 1.2.3.4
/// - Pre-release: 1.0.0-alpha, 1.0.0-rc.1, 1.0.0-beta.1
/// - Python-style pre-release: 1.2.3a1, 1.2.3b1, 1.2.3rc1
/// - Build metadata: 1.0.0+build.123
/// - With or without 'v' prefix (e.g., v1.0.0)
/// - With custom prefixes (e.g., py-v1.0.0, rust-v1.0.0, python-1.0.0)
pub fn parse_semver(tag: &str) -> Option<SemverInfo> {
    let caps = SEMVER_REGEX.captures(tag)?;

    let major = caps.get(1)?.as_str().parse::<u32>().ok()?;
    let minor = caps.get(2)?.as_str().parse::<u32>().ok()?;
    let patch = caps.get(3).and_then(|m| m.as_str().parse::<u32>().ok());
    let build = caps.get(4).and_then(|m| m.as_str().parse::<u32>().ok());

    // Pre-release can be either:
    // - Group 5: dash-style (-alpha, -beta.1, -rc.1)
    // - Groups 6+7: Python-style (a1, b1, rc1)
    let pre_release = caps.get(5).map_or_else(
        || {
            caps.get(6).map(|py_pre| {
                let py_num = caps
                    .get(7)
                    .map_or(String::new(), |m| m.as_str().to_string());
                format!("{}{}", py_pre.as_str(), py_num)
            })
        },
        |dash_pre| Some(dash_pre.as_str().to_string()),
    );

    let build_metadata = caps.get(8).map(|m| m.as_str().to_string());

    Some(SemverInfo {
        major,
        minor,
        patch,
        build,
        pre_release,
        build_metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check if a tag name is a semantic version
    fn is_semver_tag(tag: &str) -> bool {
        parse_semver(tag).is_some()
    }

    #[test]
    fn test_parse_semver_2_part() {
        let result = parse_semver("1.0");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, None);
        assert_eq!(semver.build, None);
    }

    #[test]
    fn test_parse_semver_2_part_with_v_prefix() {
        let result = parse_semver("v2.1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 1);
    }

    #[test]
    fn test_parse_semver_3_part() {
        let result = parse_semver("1.2.3");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.build, None);
    }

    #[test]
    fn test_parse_semver_3_part_with_v_prefix() {
        let result = parse_semver("v1.2.3");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
    }

    #[test]
    fn test_parse_semver_4_part() {
        let result = parse_semver("1.2.3.4");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.build, Some(4));
    }

    #[test]
    fn test_parse_semver_with_pre_release() {
        let result = parse_semver("1.0.0-alpha");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("alpha".to_string()));
    }

    #[test]
    fn test_parse_semver_with_pre_release_numeric() {
        let result = parse_semver("v2.0.0-rc.1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("rc.1".to_string()));
    }

    #[test]
    fn test_parse_semver_with_build_metadata() {
        let result = parse_semver("1.0.0+build.123");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.build_metadata, Some("build.123".to_string()));
    }

    #[test]
    fn test_parse_semver_with_pre_release_and_build() {
        let result = parse_semver("v1.0.0-beta.2+20130313144700");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("beta.2".to_string()));
        assert_eq!(semver.build_metadata, Some("20130313144700".to_string()));
    }

    #[test]
    fn test_parse_semver_2_part_with_pre_release() {
        let result = parse_semver("2.0-alpha");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, None);
        assert_eq!(semver.pre_release, Some("alpha".to_string()));
    }

    #[test]
    fn test_parse_semver_invalid_single_part() {
        assert!(parse_semver("1").is_none());
    }

    #[test]
    fn test_parse_semver_invalid_non_numeric() {
        assert!(parse_semver("abc.def").is_none());
        assert!(parse_semver("1.x.3").is_none());
    }

    #[test]
    fn test_parse_semver_invalid_too_many_parts() {
        assert!(parse_semver("1.2.3.4.5").is_none());
    }

    #[test]
    fn test_is_semver_tag() {
        // Basic versions
        assert!(is_semver_tag("1.0"));
        assert!(is_semver_tag("v1.0"));
        assert!(is_semver_tag("1.2.3"));
        assert!(is_semver_tag("v1.2.3"));
        assert!(is_semver_tag("1.2.3.4"));

        // Pre-release versions
        assert!(is_semver_tag("1.0.0-alpha"));
        assert!(is_semver_tag("v2.0.0-rc.1"));
        assert!(is_semver_tag("1.2.3-beta.2"));

        // Python-style pre-release
        assert!(is_semver_tag("1.2.3a1"));
        assert!(is_semver_tag("1.2.3b1"));
        assert!(is_semver_tag("1.2.3rc1"));

        // Build metadata
        assert!(is_semver_tag("1.0.0+build"));

        // Custom prefixes
        assert!(is_semver_tag("py-v1.0.0"));
        assert!(is_semver_tag("rust-v1.2.3-beta.1"));
        assert!(is_semver_tag("python-1.2.3b1"));

        // Invalid
        assert!(!is_semver_tag("v1"));
        assert!(!is_semver_tag("abc"));
        assert!(!is_semver_tag("1.2.3.4.5"));
        assert!(!is_semver_tag("server-v-1.0.0")); // Double dash should fail
    }

    #[test]
    fn test_parse_semver_with_custom_prefix() {
        // Test py-v prefix
        let result = parse_semver("py-v1.0.0-beta.1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("beta.1".to_string()));

        // Test rust-v prefix
        let result = parse_semver("rust-v1.0.0-beta.2");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("beta.2".to_string()));

        // Test prefix without v
        let result = parse_semver("python-2.1.0");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 1);
        assert_eq!(semver.patch, Some(0));
    }

    #[test]
    fn test_parse_semver_python_style() {
        // Alpha
        let result = parse_semver("1.2.3a1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.pre_release, Some("a1".to_string()));

        // Beta
        let result = parse_semver("v1.2.3b2");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 2);
        assert_eq!(semver.patch, Some(3));
        assert_eq!(semver.pre_release, Some("b2".to_string()));

        // Release candidate
        let result = parse_semver("2.0.0rc1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 2);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("rc1".to_string()));

        // With prefix
        let result = parse_semver("py-v1.0.0b1");
        assert!(result.is_some());
        let semver = result.unwrap();
        assert_eq!(semver.major, 1);
        assert_eq!(semver.minor, 0);
        assert_eq!(semver.patch, Some(0));
        assert_eq!(semver.pre_release, Some("b1".to_string()));
    }

    #[test]
    fn test_parse_semver_rejects_garbage() {
        // Should reject random strings with -v in them
        assert!(parse_semver("server-v-config").is_none());
        assert!(parse_semver("whatever-v-something").is_none());

        // Should reject malformed versions
        assert!(parse_semver("v1").is_none());
        assert!(parse_semver("1").is_none());
        assert!(parse_semver("1.2.3.4.5").is_none());
        assert!(parse_semver("abc.def").is_none());
    }

    #[test]
    fn test_semver_ordering() {
        let v1_0_0 = parse_semver("1.0.0").unwrap();
        let v1_0_1 = parse_semver("1.0.1").unwrap();
        let v1_0_0_alpha = parse_semver("1.0.0-alpha").unwrap();
        let v1_0_0_beta = parse_semver("1.0.0-beta").unwrap();

        // Pre-release versions compare lexicographically
        assert!(v1_0_0_alpha < v1_0_0_beta);
        // Pre-release versions are less than stable versions
        assert!(v1_0_0_beta < v1_0_0);
        // Patch versions compare correctly
        assert!(v1_0_0 < v1_0_1);
    }

    #[test]
    fn test_semver_ordering_with_build() {
        let v1_2_3_4 = parse_semver("1.2.3.4").unwrap();
        let v1_2_3_5 = parse_semver("1.2.3.5").unwrap();
        let v1_2_3 = parse_semver("1.2.3").unwrap();

        // Build numbers compare correctly
        assert!(v1_2_3_4 < v1_2_3_5);
        // None build is less than Some build (None < Some in Option ordering)
        assert!(v1_2_3 < v1_2_3_4);
    }
}
