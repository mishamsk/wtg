# Tag Identification Output Design

## Overview

Add meaningful output when a tag is identified, replacing the current placeholder message. The output will show tag metadata and changes from the best available source.

## Data Sources & Selection Logic

When a tag is queried, gather information from up to three sources and select the best one:

**Sources (in order of preference when equal quality):**
1. **GitHub Release description** - Fetched via GitHub API if the tag has an associated release
2. **CHANGELOG.md** - Parsed locally using strict Keep a Changelog format (`## [version]` headers)
3. **Commit diff** - Commits between this tag and the previous tag (limit 5)

**Selection logic:**
- Fetch GitHub release description and parse CHANGELOG section (if both exist)
- Compare their content lengths; use whichever is more substantial
- If neither has meaningful content (both empty or trivially short), fall back to commit diff
- If no previous tag exists, skip commit diff entirely

**Previous tag detection:**
- For semver tags: sort all semver tags, pick the one immediately before
- For non-semver tags: find the most recent tag pointing to an earlier commit (by commit date)

## CHANGELOG Parsing

**Format requirements (strict Keep a Changelog):**
- File must be named `CHANGELOG.md` (case-insensitive) at repository root
- Version sections must use `## [version]` header format
- Examples of valid headers:
  - `## [1.2.0]` or `## [v1.2.0]`
  - `## [1.2.0] - 2024-01-15` (with date)
  - `## [Unreleased]` (skipped for tag lookups)

**Matching logic:**
- Strip leading `v` from both tag name and changelog headers for comparison
- Match the version portion exactly (e.g., tag `v1.2.0` matches `## [1.2.0]` or `## [v1.2.0]`)
- Extract content from the matched header until the next `## ` header or end of file

**Content extraction:**
- Include all subsections (Added, Changed, Fixed, Removed, etc.)
- Preserve markdown formatting for display
- Apply 20-line truncation with hint if exceeded

## Output Format

**Structure:**
```
🏷️  Tag: v1.2.0
📅 Created: 2024-01-15
🔗 Release: https://github.com/owner/repo/releases/tag/v1.2.0

Changes (from CHANGELOG):
### Added
- New feature X
- New feature Y

### Fixed
- Bug in authentication flow
... 8 more lines (see full release)
```

**Field details:**
- **Tag** - The tag name as-is
- **Created** - The tagged commit's timestamp (from `TagInfo.created_at`)
- **Release** - GitHub release URL if it's an actual release, otherwise the tag tree URL
- **Changes header** - Indicates source: `(from GitHub release)`, `(from CHANGELOG)`, or `(commits since v1.1.0)`

**Commit diff format (when used as fallback):**
```
Changes (commits since v1.1.0):
• abc1234 Fix login redirect issue
• def5678 Add password reset flow
• ghi9012 Update dependencies
```

**Truncation:**
- After 20 lines of changes content, show `... N more lines (see full release)` with the URL

## Implementation Approach

**Files to modify:**

1. **`crates/wtg/src/output.rs`** - Replace `display_tag_warning()` with full `display_tag()` implementation

2. **`crates/wtg/src/changelog.rs`** (new module) - CHANGELOG parsing:
   - `parse_changelog_for_version(path, version) -> Option<String>` - Extract section for a given version
   - Strict Keep a Changelog format parsing
   - Add `mod changelog;` to `lib.rs`

3. **`crates/wtg/src/backend/mod.rs`** - Add to `Backend` trait:
   - `commits_between_tags(from_tag, to_tag, limit) -> WtgResult<Vec<CommitInfo>>`
   - `find_previous_tag(tag_name) -> WtgResult<Option<TagInfo>>`

4. **`crates/wtg/src/backend/git_backend.rs`** - Implement via local git operations

5. **`crates/wtg/src/backend/github_backend.rs`** - Implement via GitHub compare API

6. **`crates/wtg/src/backend/combined_backend.rs`** - Delegate appropriately (likely to git first, fallback to GitHub)

7. **`crates/wtg/src/github.rs`** - Expose release body/description if not already available

8. **`crates/wtg/src/resolution.rs`** - Enrich `TagOnly` resolution to gather all data sources

## Error Handling & Edge Cases

**Network/API failures:**
- If GitHub release fetch fails (rate limit, network error), silently fall back to CHANGELOG or commits
- Log the error at debug level but don't surface to user - just use next available source

**Missing data scenarios:**
- No GitHub release exists -> try CHANGELOG
- No CHANGELOG.md or version not found -> try commit diff
- No previous tag found -> show tag info without changes section
- All three empty -> show tag metadata only (name, date, URL)

**CHANGELOG edge cases:**
- File exists but malformed -> treat as not found, try next source
- Multiple versions in one section (shouldn't happen with strict format) -> take first match
- Version header exists but section is empty -> treat as empty, try next source

**Tag edge cases:**
- Tag points to same commit as "previous" tag -> skip that tag, find the one before it
- Multiple tags on same commit -> pick any one as "previous" (deterministic by name sort)
- Non-semver tag with no other tags in repo -> show tag info without changes

**Truncation:**
- Count rendered lines after markdown formatting preserved
- "... N more lines" links to release URL if available, otherwise tag tree URL

## Testing

**Unit tests for `changelog.rs`:**
- Parse valid Keep a Changelog format (various header styles)
- Extract correct section for given version (with/without `v` prefix)
- Handle missing version gracefully
- Reject malformed files (wrong header format)
- Handle empty sections
- Truncation at 20 lines

**Unit tests for backend trait methods:**
- `find_previous_tag` with semver tags (correct ordering)
- `find_previous_tag` with non-semver tags (date-based)
- `find_previous_tag` when no previous exists
- `commits_between_tags` returns correct commits in order
- `commits_between_tags` respects limit

**Offline tests (with test fixtures):**
- Tag with CHANGELOG entry -> shows changelog content
- Tag with both sources -> picks more substantial one
- Changelog-only fallback path

**Integration tests (wtg repo):**
- Tag with GitHub release -> shows release content
- Update `integration_identify_tag` snapshot to reflect new output format

**Uncovered (accepted):**
- Changelog-only case in integration tests (wtg repo has GitHub releases)
