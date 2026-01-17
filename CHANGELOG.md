# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- next-header -->

## [Unreleased] - ReleaseDate

### Added
- Backend abstraction with Git, GitHub, and combined implementations for local-first resolution with API fallback.
- New input parsing pipeline that standardizes queries and makes GitHub URL handling more robust and secure.
- Resolution layer to identify issues, PRs, commits, files, and tags from structured inputs.

### Changed
- GitHub repository detection now prefers upstream remotes and iterates until a GitHub remote is found.
- Remote repository handling uses lazy fetches, improved release sorting, and a ls-remote check to avoid unnecessary API calls.
- GitHub client initialization is lazy to avoid unnecessary auth setup for local-only queries.

### Deprecated
-

### Removed
-

### Fixed
- Fallback to anonymous GitHub client on public repo's in SAML protected orgs, when auth fails.
- Properly identify closing PRs when multiple PRs reference the same issue.
- Updated GitHub API client to latest version to fix issues with new timeline events.
- Commit and issue URL resolution across repositories now finds the correct target more reliably.
- Closing PR identification now works for cross-project references.

### Security
-

## [0.1.1] - 2025-11-07

### Added
- Support for GitHub comment and PR tab URLs.

### Fixed
- Fixed fetching remote repositories with moved tags.
- Fixed file history detection.
- Added outer timeouts to ensure they are honored.

### Security
- URL sanitization in GitHub URL parsing.

## [0.1.0] - 2025-11-03

### Why This Even Exists

Ever find yourself staring at a commit hash, issue number, or PR and thinking "which release was this in?" Of course you have. The "proper" way involves running `git tag --contains` (or was it `git describe`? `git log --tags`? who can remember?), then manually finding the closest tag, then hunting down the GitHub release URL like some kind of digital archaeologist. After the 47th time doing this dance, wtg was born.

Because life's too short to memorize git's 300+ commands just to answer "where did this ship?"

### Added

- 🔍 Smart detection of commits, issues, PRs, file paths, and tags - just throw stuff at it and watch it figure things out
- 🌐 Remote repository support - query any GitHub repo without cloning (because your disk is already full)
- 🔗 GitHub URL parsing - paste any GitHub URL and wtg does the thinking
- 📦 Release tracking - automatically finds which release first shipped a commit
- 👤 Blame information - discover who's responsible for that "pesky bug" (their words, not mine)
- 🎨 Colorful terminal output with emojis - because grey text is for git itself
- 😄 Snarky error messages - if git is going to be complicated, at least wtg can be entertaining
- 🚀 Smart caching for remote repos with `--filter=blob:none` (Git 2.17+) and automatic fallback
- 🌐 Graceful degradation without network or GitHub access
