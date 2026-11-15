# Install

No route needs a Rust toolchain. Every release on GitHub carries `oncebitten-<target>.tar.gz` (holding `oncebitten` and `oncebitten-checker`) and a `.sha256` file for:

| Target | Platform |
| --- | --- |
| `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` | Linux (static) |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS |
| `x86_64-pc-windows-msvc` | Windows |

## Claude Code plugin

```
/plugin marketplace add bcmyguest/oncebitten
/plugin install oncebitten@oncebitten
```

Or from a shell: `claude plugin marketplace add bcmyguest/oncebitten` then `claude plugin install oncebitten@oncebitten`.

- The plugin's hooks capture prompts and tool outcomes (see [capture.md](capture.md)). `bin/oncebitten` is on the agent's Bash `PATH`, so the agent can run `oncebitten evidence list` and the other commands.
- On the first session start the launcher downloads the release that matches the plugin's version into `${CLAUDE_PLUGIN_DATA}/bin/<version>/` and checks its SHA-256. A plugin update fetches its own version; uninstalling the plugin deletes the data dir.
- On Windows the launcher needs Git Bash.
- Do not also run `oncebitten hooks install --host claude`: each event would then be captured twice, and the store drops a duplicate only when both captures land in the same millisecond. The plugin and the command line share one events database.

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

1. Set the same version in `Cargo.toml`, every `packaging/*` plugin manifest, `version=` in `packaging/claude/bin/oncebitten` and the pins in `templates/`. `tests/packaging.rs` and `tests/gate.rs` fail if they differ.
2. Merge, then push a tag `v<version>`. `.github/workflows/release.yml` builds all five targets and publishes the release; it fails if the tag does not match `Cargo.toml`.
