# What The Git (wtg) 🔍

A snarky but helpful CLI tool to identify git commits, issues, PRs and file changes, and tell you which release they shipped in. Because sometimes you just need to know what the git is going on!

A totally vibe-coded tool, so do not blame me if it hurts your feelings. 😄

## Features

- 🔍 **Smart Detection**: Automatically identifies what you're looking for (commit hash, issue/PR number, file path, or tag)
- 🌐 **Remote Repository Support**: Work with any GitHub repository without cloning it locally
- 🔗 **GitHub URL Parsing**: Paste any GitHub URL (commit, issue, PR, file) and wtg figures it out
- 🎨 **Colorful Output**: Beautiful terminal output with emojis and colors
- 😄 **Snarky Messages**: Helpful error messages with personality
- 📦 **Release Tracking**: Finds which release first shipped your commit
- 👤 **Blame Info**: Shows who's responsible for that pesky bug
- 🚀 **Smart Caching**: Efficiently caches remote repositories with minimal disk usage
- 🌐 **Graceful Degradation**: Works without network or GitHub remote

## Installation

### Recommended: Python package

Run the (python) CLI without installing anything permanently:

```bash
uvx --from wtg-cli wtg --help
```

Or install it as a global tool (works on macOS, Linux, and Windows):

```bash
uv tool install wtg-cli
wtg --help
```

### Alternative: build/install from source

```bash
cargo install wtg-cli
```

## Usage

### Local Repository

Simply run `wtg` with any of the following:

```bash
# Find a commit by hash
wtg c62bbcc

# Find an issue or PR
wtg 123
wtg #123

# Find a file
wtg Cargo.toml

# Find a tag
wtg v1.2.3
```

### Remote Repository

Query any GitHub repository without cloning it first:

```bash
# Using -r flag with owner/repo
wtg -r astral-sh/uv 0.5.0

# Using full GitHub URL
wtg -r https://github.com/astral-sh/uv v0.5.0

# Paste any GitHub URL directly
wtg https://github.com/astral-sh/uv/commit/abc123
wtg https://github.com/astral-sh/uv/issues/42
wtg https://github.com/astral-sh/uv/pull/123
wtg https://github.com/astral-sh/uv/blob/main/README.md
```

**Note**: Remote repositories are cached in your system's cache directory for faster subsequent queries. The cache uses `--filter=blob:none` (Git 2.17+) for minimal disk usage, with automatic fallback to bare clones on older Git versions.

## Output Examples

### Commit
```
🔍 Found commit: c4e0cc8e98d3

💻 The Commit:
   Hash: c4e0cc8
   👤 Who wrote this gem:
      mishamsk (5206955+mishamsk@users.noreply.github.com)
      🔗 https://github.com/mishamsk
   📝 ⬇️ reduce msrv
   📅 2024-11-07 03:37:26
   🔗 https://github.com/mishamsk/sas-lexer/commit/c4e0cc8e98d3e786916ac811c5617ecb68d1a6fd

📦 First shipped in:
   🎉 rust-v1.0.0-beta.3 (rust-v1.0.0-beta.3)
   📅 2024-11-08 03:03:21 U
   🔗 https://github.com/mishamsk/sas-lexer/releases/tag/rust-v1.0.0-beta.3
```

## GitHub Authentication

For better rate limits, set a GitHub token:

1. **Environment variable** (recommended):
   ```bash
   export GITHUB_TOKEN=ghp_your_token_here
   ```

2. **GitHub CLI**: wtg automatically reads from `~/.config/gh/hosts.yml` if you have `gh` installed

3. **Anonymous**: Works without auth but has lower rate limits (60 requests/hour)

## How It Works

1. Opens your git repository
2. Tries to identify the input type (commit, issue, file, tag)
3. Fetches additional info from GitHub API if available
4. Finds the closest release that contains the commit
5. Displays everything in a beautiful, colorful format

## Limitations

- Only supports GitHub (GitLab and others coming... maybe?)
- Squashed commits detection not yet implemented
- No TUI mode (planned for future)

## License

MIT

## Contributing

Found a bug? Want to add a snarky message? PRs welcome! Just make sure to keep the snark levels high and the code quality higher.
