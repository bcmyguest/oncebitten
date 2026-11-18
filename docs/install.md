# Install

No route needs a Rust toolchain. Every release on GitHub carries `oncebitten-<target>.tar.gz` (holding `oncebitten` and `oncebitten-checker`) and a `.sha256` file for:

| Target | Platform |
| --- | --- |
| `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` | Linux (static) |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS |
| `x86_64-pc-windows-msvc` | Windows |

## Agent plugins

The repository is a plugin marketplace for four hosts and a pi package. Each host below has its own steps.

### Claude Code

```
/plugin marketplace add bcmyguest/oncebitten
/plugin install oncebitten@oncebitten
```

Marketplace file `.claude-plugin/marketplace.json`, plugin `packaging/claude`. In Claude Code the plugin's `bin/` is on the agent's Bash `PATH`, so the agent can run `oncebitten evidence list` and the other commands. The plugin ships one skill, `/oncebitten:new-rule` (`packaging/claude/skills/new-rule/SKILL.md`): it writes a rule directory by hand, with fixtures and a replay proof. The other hosts' plugins ship no skill.

### Codex

```sh
codex plugin marketplace add bcmyguest/oncebitten
```

Then install `oncebitten` from `/plugins`. Marketplace file `.agents/plugins/marketplace.json`, plugin `packaging/codex`.

### Copilot CLI

```sh
copilot plugin marketplace add bcmyguest/oncebitten
copilot plugin install oncebitten@oncebitten
```

Marketplace file `.github/plugin/marketplace.json`, plugin `packaging/copilot`.

### Cursor

Import `https://github.com/bcmyguest/oncebitten` as a team marketplace (Dashboard, Settings, Plugins). Marketplace file `.cursor-plugin/marketplace.json`, plugin `packaging/cursor`.

### pi

```sh
pi install git:github.com/bcmyguest/oncebitten
```

`package.json` (`pi.extensions`) loads `packaging/pi/package.js`. pi has no session-start hook: its first event starts the download in the background and is dropped.

### opencode

opencode loads plugins from npm or local files only, and oncebitten is not an opencode plugin: install the binary from [Command line](#command-line), then:

```sh
oncebitten hooks install --host opencode --scope user
```

### How the plugins work

- Each plugin ships the same `bin/oncebitten` launcher and `scripts/install.sh`. The launcher runs the release binary that matches its version from `<data>/bin/<version>/`, where `<data>` is `$CLAUDE_PLUGIN_DATA`, else `$PLUGIN_DATA` (Codex), else `$XDG_DATA_HOME/oncebitten` or `~/.local/share/oncebitten`. A missing binary is installed with SHA-256 verification.
- A `SessionStart` hook (cursor `sessionStart`) runs the launcher once with `--version`, so the download happens before any capture.
- The claude and codex plugins also run `oncebitten steer --host <host>` on `UserPromptSubmit`, `PreToolUse`, `Stop` and `SessionStart` with a 10 s timeout: steer rules work without `hooks install`, and a session start announces waiting rule proposals (see [steer.md](steer.md)). The cursor and copilot plugins ship capture only; `oncebitten hooks install` adds their steer hook.
- A `capture` or `steer` call that finds no binary yet drains stdin, starts the download in the background and exits 0, so a hook never waits on a download and never blocks the agent.
- The launcher is a POSIX `sh` script. On Windows it needs Git Bash (or another `sh` on `PATH`); otherwise use `install.ps1` and `oncebitten hooks install`.
- Verified here: `claude plugin validate` on the marketplace and plugin; the launcher and the pi package against a local release with the real Linux binary. The Codex, Copilot and Cursor marketplace formats come from their CLIs and docs, not from a live install.
- Do not also run `oncebitten hooks install` for a host whose plugin is installed: each event would then be captured twice, and the store drops a duplicate only when both captures land in the same millisecond. On claude and codex the steer hook would run twice as well and repeat each message. Plugins and the command line share one events database.


## Command line

One route per tool. Each installs `oncebitten` and `oncebitten-checker`.

### Python (PyPI)

```sh
uvx oncebitten --version        # run once, no install
uv tool install oncebitten      # install into an isolated environment
```

The wheels are built per platform with maturin (`bindings = "bin"`): Linux x86_64 and aarch64 (manylinux and musllinux), macOS x86_64 and arm64, Windows x86_64.

### Node (npm)

```sh
npx oncebitten --version        # run once
npm install -g oncebitten       # install
```

`oncebitten` is one package holding a small launcher. On first run it downloads the release tarball for your platform from the GitHub Release, verifies its SHA-256, caches it under `~/.cache/oncebitten/<version>/` (`$XDG_CACHE_HOME/oncebitten`, `%LOCALAPPDATA%\oncebitten\cache` on Windows) and runs it. Nothing runs at install time, so `--ignore-scripts` works; the first run needs network access. Capture and steer hooks never wait on that download: they exit 0 and fetch in the background.

- `ONCEBITTEN_RELEASE_URL`: releases base URL, `https://` or `file://` (default: the GitHub releases page).
- `ONCEBITTEN_CACHE_DIR`: cache directory.

### Shell scripts

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

## Teams

Pin one version so every machine and CI run behaves the same.

```toml
# pyproject.toml
[dependency-groups]
dev = ["oncebitten==0.5.0"]
```

```sh
uv add --dev oncebitten==0.5.0 && uv run oncebitten --version
```

```sh
npm install --save-dev --save-exact oncebitten@0.5.0 && npx oncebitten --version
```

```yaml
# .pre-commit-config.yaml
repos:
  - repo: https://github.com/bcmyguest/oncebitten
    rev: v0.5.0
    hooks:
      - id: oncebitten-gate-python
```

`oncebitten-gate-python` installs the prebuilt wheel into the hook's virtualenv. The `==` pin in `.pre-commit-hooks.yaml` follows the release; `rev` selects it. The other hooks stay: `oncebitten-gate` builds from source, `oncebitten-gate-system` runs the binary on `PATH`.

## CI and pre-commit

The templates in `templates/` install a pinned release with `install.sh`. The pre-commit hook `oncebitten-gate-system` runs the installed binary; `oncebitten-gate` builds from source instead (see [gate.md](gate.md)).

## From source

```sh
cargo install --locked --git https://github.com/bcmyguest/oncebitten oncebitten
```

The build compiles tree-sitter grammars and SQLite, so it needs a C compiler.

## Releasing

1. Run the **Prepare release** workflow (Actions tab) on `main`. `git-cliff` picks the next version from the Conventional Commits since the last tag; `scripts/set-version.sh` pins it in `Cargo.toml`, `pyproject.toml`, `package.json`, the `packaging/*` manifests, the launcher, `templates/` and `.pre-commit-hooks.yaml`; the workflow commits, tags `v<version>` and dispatches **Release**. Tick `dry_run` to see the version only.
2. **Release** builds the five targets once (`build.yml`) and feeds the GitHub Release (tarballs and checksums), PyPI (trusted publishing) and npm (one launcher package, trusted publishing with provenance), then installs the published packages on Linux, macOS and Windows (`smoke.yml`). It fails if the tag does not match `Cargo.toml`.

The npm package is published last, after the GitHub Release exists, because its launcher downloads the binary from that release. Rerun **Release** on the tag to publish a version npm lacks.

`tests/packaging.rs` and `tests/gate.rs` fail if the pinned versions differ. `scripts/fresh-project.sh` is the install smoke test: `scripts/fresh-project.sh artifacts <dir>` for the local wheel, npm tarball and release tarball, `scripts/fresh-project.sh published <version>` for the registries.
