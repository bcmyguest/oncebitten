# Install

No route needs a Rust toolchain. Every release on GitHub carries `oncebitten-<target>.tar.gz` (holding `oncebitten` and `oncebitten-checker`) and a `.sha256` file for:

| Target | Platform |
| --- | --- |
| `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` | Linux (static) |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS |
| `x86_64-pc-windows-msvc` | Windows |

## Agent plugins

The repository is a plugin marketplace for four hosts and a pi package:

| Host | Marketplace file | Plugin | Install |
| --- | --- | --- | --- |
| Claude Code | `.claude-plugin/marketplace.json` | `packaging/claude` | `/plugin marketplace add bcmyguest/oncebitten`, `/plugin install oncebitten@oncebitten` |
| Codex | `.agents/plugins/marketplace.json` | `packaging/codex` | `codex plugin marketplace add bcmyguest/oncebitten`, then `/plugins` |
| Copilot CLI | `.github/plugin/marketplace.json` | `packaging/copilot` | `copilot plugin marketplace add bcmyguest/oncebitten`, `copilot plugin install oncebitten@oncebitten` |
| Cursor | `.cursor-plugin/marketplace.json` | `packaging/cursor` | Team marketplace import of the repository URL |
| pi | `package.json` (`pi.extensions`) | `packaging/pi/package.js` | `pi install git:github.com/bcmyguest/oncebitten` |

- Each plugin ships the same `bin/oncebitten` launcher and `scripts/install.sh`. The launcher runs the release binary that matches its version from `<data>/bin/<version>/`, where `<data>` is `$CLAUDE_PLUGIN_DATA`, else `$PLUGIN_DATA` (Codex), else `$XDG_DATA_HOME/oncebitten` or `~/.local/share/oncebitten`. A missing binary is installed with SHA-256 verification.
- A `SessionStart` hook (cursor `sessionStart`) runs the launcher once with `--version`, so the download happens before any capture. pi has no such hook: its first event starts the download in the background and is dropped.
- In Claude Code the plugin's `bin/` is on the agent's Bash `PATH`, so the agent can run `oncebitten evidence list` and the other commands.
- The launcher is a POSIX `sh` script. On Windows it needs Git Bash (or another `sh` on `PATH`); otherwise use `install.ps1` and `oncebitten hooks install`.
- opencode loads plugins from npm or local files only, and oncebitten is not on npm: install the binary and run `oncebitten hooks install --host opencode --scope user`.
- Verified here: `claude plugin validate` on the marketplace and plugin; the launcher and the pi package against a local release with the real Linux binary. The Codex, Copilot and Cursor marketplace formats come from their CLIs and docs, not from a live install.
- Do not also run `oncebitten hooks install` for a host whose plugin is installed: each event would then be captured twice, and the store drops a duplicate only when both captures land in the same millisecond. Plugins and the command line share one events database.

## Command line

```sh
curl -fsSL https://raw.githubusercontent.com/bcmyguest/oncebitten/main/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/bcmyguest/oncebitten/main/install.ps1 | iex
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `ONCEBITTEN_VERSION` | `latest` | Release to install, such as `0.1.0` |
| `ONCEBITTEN_INSTALL_DIR` | `~/.local/bin`; Windows `%LOCALAPPDATA%\oncebitten\bin` | Where the binaries go |
| `ONCEBITTEN_RELEASE_URL` | `https://github.com/bcmyguest/oncebitten/releases` | Releases base URL, for a mirror |

`install.sh` needs `curl` or `wget`, `tar`, and `sha256sum` or `shasum`. `install.ps1` needs Windows 10 1803 or later for `tar.exe`, and adds the install dir to the user `PATH`. Both refuse an archive whose checksum does not match.

Then install hooks for any other host: `oncebitten hooks install --host <codex|cursor|copilot|opencode|pi> --scope user`.

## CI and pre-commit

The templates in `templates/` install a pinned release with `install.sh`. The pre-commit hook `oncebitten-gate-system` runs the installed binary; `oncebitten-gate` builds from source instead (see [gate.md](gate.md)).

## From source

```sh
cargo install --locked --git https://github.com/bcmyguest/oncebitten oncebitten
```

The build compiles tree-sitter grammars and SQLite, so it needs a C compiler.

## Releasing

1. Set the same version in `Cargo.toml`, `package.json`, every `packaging/*` plugin manifest, `version=` in `packaging/claude/bin/oncebitten` and the pins in `templates/`. `tests/packaging.rs` and `tests/gate.rs` fail if they differ.
2. Merge, then push a tag `v<version>`. `.github/workflows/release.yml` builds all five targets and publishes the release; it fails if the tag does not match `Cargo.toml`.
