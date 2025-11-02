# What The Git (wtg) 🔍

A snarky but helpful CLI tool to identify git commits, issues, PRs and file changes, and tell you which release they shipped in. Because sometimes you just need to know what the git is going on!

A totally vibe-coded tool, so do not blame me if it hurts your feelings. 😄

## Features

- 🔍 **Smart Detection**: Automatically identifies what you're looking for (commit hash, issue/PR number, file path, or tag)
- 🎨 **Colorful Output**: Beautiful terminal output with emojis and colors
- 😄 **Snarky Messages**: Helpful error messages with personality
- 📦 **Release Tracking**: Finds which release first shipped your commit
- 👤 **Blame Info**: Shows who's responsible for that pesky bug
- 🔗 **GitHub Integration**: Generates clickable links to commits, issues, PRs, and profiles
- 🌐 **Graceful Degradation**: Works without network or GitHub remote

## Installation

### Recommended: Python package

Run the CLI straight from PyPI without installing anything permanently:

```bash
uvx --from wtg-cli wtg --help
```

Or install it as a global tool (works on macOS, Linux, and Windows):

```bash
uv tool install wtg-cli
wtg --help
```

`wtg-cli` is just a regular wheel, so it can also be installed with `pip`, `pipx`, or any other Python package manager.

```bash
pip install wtg-cli
```

Working from a local checkout? `uv run` will build the wheel in place and execute it:

```bash
uv run wtg --help
```

### Alternative: build from source

```bash
cargo install --path crates/wtg
```

## Usage

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

## Output Examples

### Commit
```
🔍 Found commit: 9659c9f

📝 📝 changelog
   📅 2023-09-03 23:57:54
   📚 Someone likes to write essays... 1 more line

👎 Who's to blame for this pesky bug:
   👤 mishamsk (5206955+mishamsk@users.noreply.github.com)

📦 First shipped in:
   🏷️  v2.0.0
   🔗 https://github.com/mishamsk/pyoak/releases/tag/v2.0.0

🔗 https://github.com/mishamsk/pyoak/commit/9659c9f08a4cf2ba53e6a6f3316f02c302a10c50
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

## Limitations (v0.1.0)

- Only supports GitHub (GitLab and others coming... maybe?)
- No caching (every query hits git/GitHub fresh)
- Issue/PR closing commit detection not yet implemented
- No TUI mode (planned for future)

## License

MIT

## Contributing

Found a bug? Want to add a snarky message? PRs welcome! Just make sure to keep the snark levels high and the code quality higher.
