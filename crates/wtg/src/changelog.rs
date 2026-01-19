//! CHANGELOG.md parsing for Keep a Changelog format.
//!
//! Supports Keep a Changelog format with version headers at either level 2 (`##`)
//! or level 3 (`###`). The header level is auto-detected from the content - once
//! a version header is found at a specific level, that level is used consistently
//! for section boundaries.
//!
//! See <https://keepachangelog.com> for format specification.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

/// Regex for parsing version headers at level 2: `## [version]` or `## [version - date]`
/// Captures everything inside brackets; date part is stripped in code if present.
static HEADER_REGEX_L2: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^## \[([^\]]+)\]").expect("Invalid changelog header regex"));

/// Regex for parsing version headers at level 3: `### [version]` or `### [version - date]`
/// Captures everything inside brackets; date part is stripped in code if present.
static HEADER_REGEX_L3: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^### \[([^\]]+)\]").expect("Invalid changelog header regex"));

/// Maximum number of lines to include in changelog output before truncation.
pub const MAX_LINES: usize = 20;

/// Extract the changelog section for a specific version.
///
/// Looks for CHANGELOG.md (case-insensitive) at the given path and extracts
/// the section matching the version. Returns None if file doesn't exist,
/// version not found, or format is invalid.
///
/// # Arguments
/// * `repo_root` - Path to the repository root
/// * `version` - Version to find (with or without 'v' prefix)
///
/// # Returns
/// The changelog section content, or None if not found.
#[must_use]
pub fn parse_changelog_for_version(repo_root: &Path, version: &str) -> Option<String> {
    let changelog_path = find_changelog_file(repo_root)?;
    let content = fs::read_to_string(changelog_path).ok()?;
    extract_version_section(&content, version)
}

/// Find CHANGELOG.md file (case-insensitive) at repo root.
fn find_changelog_file(repo_root: &Path) -> Option<std::path::PathBuf> {
    let entries = fs::read_dir(repo_root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("changelog.md") {
            return Some(entry.path());
        }
    }
    None
}

/// Extract a version section from changelog content.
///
/// Matches changelog formats with version headers at either `##` or `###` level.
/// The header level is auto-detected: tries `##` first, then `###` if no matches found.
/// Version matching is flexible: strips 'v' prefix from both sides for comparison.
#[must_use]
pub fn extract_version_section(content: &str, version: &str) -> Option<String> {
    // Try level 2 headers first (## [version])
    if let Some(result) = extract_version_section_with_regex(content, version, &HEADER_REGEX_L2) {
        return Some(result);
    }

    // Fall back to level 3 headers (### [version])
    extract_version_section_with_regex(content, version, &HEADER_REGEX_L3)
}

/// Extract a version section using a specific header regex.
fn extract_version_section_with_regex(
    content: &str,
    version: &str,
    header_regex: &Regex,
) -> Option<String> {
    // Normalize version by stripping 'v' prefix
    let normalized_version = version.strip_prefix('v').unwrap_or(version);

    let mut section_start: Option<usize> = None;
    let mut section_end: Option<usize> = None;

    for caps in header_regex.captures_iter(content) {
        let full_match = caps.get(0)?;
        let captured_version = caps.get(1)?.as_str();

        // Normalize captured version:
        // - Strip 'v' prefix
        // - Strip date suffix (` - 2024-01-15` style)
        let normalized_captured = captured_version
            .strip_prefix('v')
            .unwrap_or(captured_version);
        let normalized_captured = normalized_captured
            .split(" - ")
            .next()
            .unwrap_or(normalized_captured)
            .trim();

        if section_start.is_some() {
            // We found the next section header, mark end
            section_end = Some(full_match.start());
            break;
        }

        if normalized_captured == normalized_version {
            // Found our version, start after the header line
            let line_end = content[full_match.end()..]
                .find('\n')
                .map_or_else(|| full_match.end(), |i| full_match.end() + i + 1);
            section_start = Some(line_end);
        }
    }

    let start = section_start?;
    let end = section_end.unwrap_or(content.len());

    let section = content[start..end].trim();
    if section.is_empty() {
        return None;
    }

    Some(section.to_string())
}

/// Truncate content to `MAX_LINES`, returning (content, `remaining_lines`).
///
/// If content exceeds `MAX_LINES`, returns truncated content and count of remaining lines.
/// Otherwise returns original content and 0.
#[must_use]
pub fn truncate_content(content: &str) -> (&str, usize) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= MAX_LINES {
        return (content, 0);
    }

    let truncated_end = lines[..MAX_LINES]
        .iter()
        .map(|l| l.len() + 1) // +1 for newline
        .sum::<usize>();

    // Find the actual byte position (handle last line without newline)
    let truncated_end = truncated_end.min(content.len());
    let truncated = &content[..truncated_end].trim_end();

    (truncated, lines.len() - MAX_LINES)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CHANGELOG: &str = r"# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Something new

## [1.2.0] - 2024-01-15

### Added
- Feature X
- Feature Y

### Fixed
- Bug in authentication

## [1.1.0] - 2024-01-01

### Added
- Initial feature
";

    #[test]
    fn extracts_version_section() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "1.2.0");
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("Feature X"));
        assert!(content.contains("Bug in authentication"));
        assert!(!content.contains("Initial feature"));
    }

    #[test]
    fn extracts_version_with_v_prefix() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "v1.2.0");
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("Feature X"));
    }

    #[test]
    fn handles_changelog_with_v_prefix_in_header() {
        let changelog = r"# Changelog

## [v2.0.0] - 2024-02-01

### Changed
- Major update
";
        let result = extract_version_section(changelog, "2.0.0");
        assert!(result.is_some());
        assert!(result.unwrap().contains("Major update"));

        let result2 = extract_version_section(changelog, "v2.0.0");
        assert!(result2.is_some());
    }

    #[test]
    fn returns_none_for_missing_version() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "9.9.9");
        assert!(result.is_none());
    }

    #[test]
    fn extracts_unreleased_section() {
        let result = extract_version_section(SAMPLE_CHANGELOG, "Unreleased");
        assert!(result.is_some());
    }

    #[test]
    fn returns_none_for_empty_section() {
        let changelog = r"# Changelog

## [1.0.0]

## [0.9.0]

### Added
- Something
";
        let result = extract_version_section(changelog, "1.0.0");
        assert!(result.is_none());
    }

    #[test]
    fn extracts_version_from_level3_headers() {
        // Faker-style changelog with ### for versions
        let changelog = r"## Changelog

### [v40.1.2 - 2026-01-13](https://github.com/joke2k/faker/compare/v40.1.1...v40.1.2)

* Make `tzdata` conditionally required based on platform. Thanks @rodrigobnogueira.

### [v40.1.1 - 2026-01-10](https://github.com/joke2k/faker/compare/v40.1.0...v40.1.1)

* Fix something else
";
        let result = extract_version_section(changelog, "v40.1.2");
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("tzdata"));
        assert!(!content.contains("Fix something else"));
    }

    #[test]
    fn extracts_version_from_level3_headers_without_v_prefix() {
        let changelog = r"## Changelog

### [40.1.2 - 2026-01-13]

* Feature A

### [40.1.1 - 2026-01-10]

* Feature B
";
        // Search with v prefix, should still find version without v
        let result = extract_version_section(changelog, "v40.1.2");
        assert!(result.is_some());
        assert!(result.unwrap().contains("Feature A"));

        // Search without v prefix
        let result2 = extract_version_section(changelog, "40.1.2");
        assert!(result2.is_some());
        assert!(result2.unwrap().contains("Feature A"));
    }

    #[test]
    fn truncates_long_content() {
        let long_content = (0..30)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (truncated, remaining) = truncate_content(&long_content);

        assert_eq!(remaining, 10);
        assert!(truncated.lines().count() <= MAX_LINES);
        assert!(truncated.contains("Line 0"));
        assert!(truncated.contains("Line 19"));
        assert!(!truncated.contains("Line 20"));
    }

    #[test]
    fn does_not_truncate_short_content() {
        let short_content = "Line 1\nLine 2\nLine 3";
        let (result, remaining) = truncate_content(short_content);

        assert_eq!(remaining, 0);
        assert_eq!(result, short_content);
    }
}
