# Spike: drive OpenShell from Rust to lint bad.py and fixed.py offline

## Verdict

**Works with caveats.** A Rust program using `std::process::Command` drives the `openshell` CLI: it creates a sandbox with a read-only corpus and no network, runs a grep check on `bad.py` and `fixed.py`, deletes the sandbox, and prints `PASS`. Caveats: Linux and Docker only, pre-release v0.1.8, local gateway run in unauthenticated dev mode, and exec-based isolation checks rather than a production review.

## Environment

- Docker daemon reachable by the user; local `docker build` works. Base image `nvcr.io/nvidia/base/ubuntu:24.04` pulled by the Docker driver.
- No sudo used. No root, no system config touched. `install.sh` was read but not run.
- `install.sh` (read only): uses `sudo` for apt/dpkg/snap, writes system paths and systemd units. Not executed.
- Installed by copying release tarballs into user paths:
  - CLI: `~/.local/bin/openshell` (from `openshell-x86_64-unknown-linux-musl.tar.gz`, v0.1.8). Checksum OK.
  - Gateway: `/tmp/spike-openshell/dl/gw` (from `openshell-gateway-x86_64-unknown-linux-gnu.tar.gz`). Checksum not verified: the checksum file matched only the CLI line.

## Commands used

```
openshell gateway add http://127.0.0.1:17670 --local
openshell-gateway --config gateway.toml --disable-tls --compute-driver docker   # env OPENSHELL_LOCAL_TLS_DIR=/tmp/spike-openshell/tls
openshell-gateway generate-certs --output-dir /tmp/spike-openshell/tls
openshell sandbox create --name <n> --from spike-corpus:ro --detach --no-auto-providers --no-tty --policy policy-ro-nonet.yaml
openshell sandbox exec <n> -- sh -c 'grep -q utcnow /corpus/bad.py'   # rc 0 bad, 1 fixed
openshell sandbox delete <n>
```

gateway.toml: `version = 2` under `[openshell]`, plus `[openshell.gateway.auth] allow_unauthenticated_users = true`.

Policy (`policy-ro-nonet.yaml`): `filesystem_policy` with `read_only` for `/corpus`, `/bin`, `/usr`, `/lib`, `/etc`, `/proc`, `/dev/urandom`; `read_write` for `/tmp`, `/dev/null`; `network_policies: {}`.

Image: `Dockerfile` = `FROM nvcr.io/nvidia/base/ubuntu:24.04` + `COPY corpus /corpus`, built as `spike-corpus:ro`. Fixtures are in the image, not uploaded.

## Timings (5 runs, warm image)

| Step | Range |
|---|---|
| `sandbox create` | 512–712 ms |
| `exec` grep bad.py | ~217 ms |
| `exec` grep fixed.py | ~216 ms |
| `sandbox delete` | ~5.1 s |

First create that pulled the image: ~2.9 s. The full Rust run took 639 ms, with delete included in that figure, so it reflects a different delete path from the table. Treat the table as the measured figures.

## Isolation: verified vs assumed

Verified:
- Default sandbox: TCP to example.com:443 and to a raw IP refused.
- Positive control (policy allows only example.com:443 for bash): example.com:443 connects, 1.1.1.1:443 refused. The denials come from the policy.
- No-network policy: TCP refused.
- Read-only `/corpus`: `grep` works, writes fail.
- Exec runs as uid 1000 (`ubuntu`).
- Exit codes propagate: `sandbox exec` returns the remote exit code (0/1 observed).
- Stdout and stderr are captured by `Command::output()`.

Assumed (not verified):
- The filesystem policy is enforced by Landlock or a similar kernel mechanism. I observed the behavior only.
- The network policy holds for DNS and for UDP.

## Getting files in

- `openshell sandbox upload` works. It needs `--no-git-ignore` when a parent `.git` exists (`/tmp/.git` here). It nests the source: uploading `corpus` to `/sandbox/corpus` created `/sandbox/corpus/corpus`.
- Upload into a read-only path fails with `Permission denied`.
- Baking files into a `--from` image is the route that works with a read-only repo. The repo is not mounted: the CLI copies the image's content.

## Failure modes observed

- Gateway down: `failed to connect to gateway 'openshell' at http://127.0.0.1:17670 ... transport error`. The program prints `CANNOT RUN` and exits 2.
- Docker driver without `generate-certs` output: sandbox create refused for missing launch-scoped gateway authentication.
- Sandbox name over 19 characters: `name exceeds maximum length`.
- Sandbox unavailable without Docker: not tested.

## Rust program

`/tmp/spike-openshell/rust-spike/src/main.rs`, 43 lines (over the ~30-line target, because the failure paths are explicit). Exit codes: 0 PASS, 1 FAIL (rule did not behave as expected), 2 CANNOT RUN. The `OPENSHELL_BIN` env var overrides the binary path. Output on the last run: `bad.py exit=1 fixed.py exit=0`, `PASS`.

## SDK as a git dependency

- `crates/openshell-sdk` at rev `2e5589b2`: async Rust client for gateways. Deps include tonic, tokio, reqwest, rustls, oauth2, hyper, and tokio-tungstenite.
- Workspace requires `rust-version = "1.94"`, edition 2024. Local rustc 1.97.1.
- `openshell-core` build script uses a vendored `protoc` unless `PROTOC` is set. Not needed on this machine (`protoc` absent; build succeeded).
- Verified: a throwaway crate with `openshell-sdk = { git = ..., package = "openshell-sdk" }` runs `cargo check` in 43 s cold, rc 0.
- Not verified: API stability, crates.io availability, and any call against the gateway through the SDK.

## Windows

From `WINDOWS.md` on `main` (read via `gh api`):
- Native Windows runs on the `windows` branch only, with Microsoft MXC ProcessContainer isolation (x64 and ARM64).
- Docker, Podman, Kubernetes and MicroVM are not native Windows backends.
- The `windows` branch is not production-qualified: outer-fence guarantees are asserted for compatibility, not verified; loopback exposure to host services; no foreground attach; no gateway restart recovery.
- Native Windows needs `wxc-exec.exe` MXC 1.0.0 and `--probe` capability. Command-based create needs `allow_driver_config = true`.
- Verified: release v0.1.8 has no Windows assets (only Linux and Apple Darwin tarballs and Linux .deb/.rpm/.snap).
- Assumed: WSL 2 running the Linux CLI and gateway works. WINDOWS.md says WSL 2 is separate from native Windows, which implies it is a Linux install. Not tested.

Fallback when no sandbox runtime exists (proposal, not tested): report `CANNOT RUN` as an explicit lint status, not a pass. Options are (a) run the check unsandboxed with a labelled status, (b) require WSL 2 with Docker, (c) skip the check in CI. Choose one before implementation.

## Recommendation

Use the CLI now: it works, needs no crate, and the exit codes are clear. Revisit the SDK once it has a crates.io release or a stable API, since a git dependency pins a commit and needs Rust 1.94 or later.
