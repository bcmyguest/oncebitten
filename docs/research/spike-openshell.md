# Spike: replay an ast-grep rule in an OpenShell sandbox from Rust

Second run, 2026-11-14. It replaces the first spike, which grepped for `utcnow`,
read remote exit code 1 as "rule fired", and passed when the input file was missing.

Code: `/tmp/spike-openshell2` (throwaway). Gateway: the first spike's, on
`127.0.0.1:17670`, unauthenticated dev mode, openshell v0.1.8, Docker driver.

## Verdict

**Works.** A Rust driver (`std::process::Command` + `openshell` CLI) replays the
ast-grep rule `py-utcnow` in a sandbox with no network and a read-only `/corpus`.
It reports `Fired(1)` for `bad.py` and `Clean` for `fixed.py`. Each of the eight
failure scenarios below ends as could-not-run, and none reads as a pass.
Verified: `driver bad.py fixed.py`, plus the scenario commands below.

Caveat: "read-only corpus" means files baked into the image under a `read_only`
filesystem policy. OpenShell has no host bind mount (`sandbox create --help` lists
`--from` and `--upload` only). Verified by the help output.

## Pieces

- **Checker** (`checker/src/main.rs`, 30 lines): Rust on `ast-grep-core`/`-config`/`-language`
  0.51.2, Python grammar only. Usage `checker <rule.yml> <file>`. It prints one JSON line,
  `{"status":"fired"|"clean","file":...,"matches":[{"rule","line","text"}]}`
  or `{"status":"error","reason":...}`, and always exits 0. Built static in
  `rust:1-alpine`: `file` reports `static-pie linked`, 4.9 MB stripped. Verified.
  Host `cargo build --target x86_64-unknown-linux-musl` fails without `x86_64-linux-musl-gcc`;
  `CC=cc` fails at link. Verified.
- **Rule**: `/tmp/spike-astgrep/rules/py-utcnow.yml` (`datetime.utcnow()` and
  `datetime.datetime.utcnow()`), copied to `/rules` in the image.
- **Images**: `spike2-baked` = ubuntu 24.04 + `/corpus` + `/rules` + `/opt/oncebitten/checker`;
  `spike2-plain` = the same without the checker, for the upload route.
- **Policy** (`policy.yaml`): `read_only` for `/corpus /rules /opt /bin /usr /lib /etc /proc /dev/urandom`;
  `read_write` for `/sandbox /tmp /dev/null`; `network_policies: {}`.
- **Driver** (`driver/src/main.rs`, 83 lines): create, optional upload, one exec per
  file, delete always. Env knobs: `MODE=upload`, `IMAGE`, `CHECKER`, `GATEWAY_ENDPOINT`
  (passed as `--gateway-endpoint`), `OPENSHELL_BIN`.

### Classification rule (driver)

The driver accepts **Fired** only when stdout holds exactly one JSON line with
`status=fired`, non-empty `matches`, `file` equal to the requested path, and CLI exit 0.
It accepts **Clean** only for `status=clean`, empty `matches`, matching `file`, and exit 0.
Everything else, including `status=error`, no JSON, extra lines and a JSON/exit-code
mismatch, becomes **CouldNotRun(reason)** carrying rc and stderr. Exit codes count as
evidence and never decide the verdict.

## Scenarios

All rows verified by running `/tmp/spike-openshell2/driver/target/release/driver` with the
listed env and args. Driver exit: 0 = all files classified fired/clean, 2 = any could-not-run.

| Scenario | Command | Expected | Observed | Result |
|---|---|---|---|---|
| Real violation, baked checker | `driver bad.py fixed.py` | bad fired, fixed clean | `bad.py: Fired(1)`, `fixed.py: Clean`, rc 0 (5 runs) | PASS |
| Real violation, uploaded checker | `MODE=upload driver bad.py fixed.py` | same | same, rc 0 (3 runs) | PASS |
| Missing input file | `driver nope.py` | could-not-run | `CouldNotRun("checker error: input /corpus/nope.py: No such file ...")`, exec rc 0 | PASS |
| Missing checker (wrong path) | `CHECKER=/opt/oncebitten/missing driver bad.py` | could-not-run | `CouldNotRun("expected 1 JSON line, got 0; rc=127 ...No such file")` | PASS |
| Missing checker (upload skipped) | `IMAGE=spike2-plain:latest driver bad.py` | could-not-run | same shape, rc 127 | PASS |
| Gateway unreachable | `GATEWAY_ENDPOINT=http://127.0.0.1:17699 driver bad.py` | could-not-run | `COULD-NOT-RUN all files: sandbox create failed: rc=1 ... transport error` | PASS |
| Sandbox create failure | `IMAGE=does-not-exist:nope driver bad.py` | could-not-run | `COULD-NOT-RUN ... rc=1 ... ImagePullFailed ... 404` | PASS |
| openshell binary absent | `OPENSHELL_BIN=/nonexistent/openshell driver bad.py` | could-not-run | `COULD-NOT-RUN all files: spawn openshell: No such file` | PASS |
| Checker exits 0, prints nothing | `CHECKER=/bin/true driver bad.py` | could-not-run | `CouldNotRun("expected 1 JSON line, got 0; rc=0")` | PASS |
| Checker exits 1, prints nothing (old spike's "fired") | `CHECKER=/bin/false driver bad.py` | could-not-run | `CouldNotRun("expected 1 JSON line, got 0; rc=1")` | PASS |

The shared gateway stayed up throughout; `openshell status` reported `Connected` before the run.

## openshell CLI exit codes for its own failures

Verified with `openshell ...; echo $?` (gateway-down rows use `--gateway-endpoint http://127.0.0.1:17699`).

| Failure | rc | Message head |
|---|---|---|
| Gateway unreachable: `sandbox create`/`list` | 1 | `connect error: transport error` |
| Gateway unreachable: `status` | 1 | `tcp connect error ... Connection refused` |
| `sandbox create`, image missing | 1 | `ImagePullFailed ... 404` |
| `sandbox create`, name > 19 chars | 1 | `name exceeds maximum length (25 > 19)` |
| `sandbox create`, policy file missing | 1 | `failed to inspect sandbox policy` |
| `sandbox create --upload` into read-only path | 1 | `sandbox upload failed ... Permission denied`; **the sandbox stays `Ready`** |
| `sandbox exec`/`upload`, unknown sandbox | 1 | `sandbox not found` |
| `sandbox delete`, unknown sandbox | **0** | `already deleted` |
| Unknown flag or subcommand | 2 | clap usage error |
| `sandbox exec --timeout 1 -- sleep 5` | 124 | (none) |

`sandbox exec` passes the remote exit code through unchanged: remote 1, 2, 3, 127 gave
CLI rc 1, 2, 3, 127; a remote `kill -9 $$` gave 137. Verified. CLI failures use 1 and 2,
which collide with remote codes, so rc alone cannot tell "gateway broke" from "command
exited 1". Exec runs commands through `/bin/bash` (stderr `/bin/bash: line 1: ...`), so a
missing binary returns 127 from bash. Verified.

## Isolation checks (sandbox from `spike2-baked` + `policy.yaml`)

Verified via `openshell sandbox exec --no-tty ob2-man -- ...`:
- `echo x > /corpus/bad.py` and `touch /corpus/new`: `Permission denied`.
- `bash` `/dev/tcp/1.1.1.1/443` and `/dev/udp/1.1.1.1/53`: `connect: Permission denied`.
- Runs as uid 1000 `ubuntu`. `/opt/oncebitten/checker` is root-owned, mode 755.

Unverified: the enforcement mechanism (Landlock or seccomp is likely; I observed only
the behavior); DNS through a resolver socket; behavior on Docker-less hosts.

## Timings (warm image, local gateway)

Measured by the driver's `timing` stderr lines (`Instant` around each `Command`).

| Step | Observed |
|---|---|
| `sandbox create` | 493-516 ms (10 runs) |
| `sandbox upload` checker (4.9 MB) | 46-50 ms (3 runs) |
| `sandbox exec` checker, one file | 9-12 ms |
| `sandbox delete` | 79-90 ms, once 5092 ms |
| Whole baked run, 2 files | 621 ms (`time driver bad.py fixed.py`) |

Cost of each route to get the checker in:
- **Baked**: one-off build of the static binary 39.7 s (`time docker run rust:1-alpine ... cargo build`,
  includes image pull); `docker build` of the image with the binary 0.29-0.32 s. Per run: 0 ms extra;
  create took the same time as the plain image. Verified.
- **Upload**: about 48 ms per sandbox, and the executable bit survives. Verified.

Exec now takes ~10 ms, against ~217 ms in the first spike. I did not find the cause;
the first spike went through `sh -c`, this one calls the binary directly. Unverified.

## What step 4 of PLAN.md needs

Step 4: "Replay: fires on bad, passes on fixed, passes on valid corpus."

1. A checker with a JSON contract: one line per file (or per run), `status` in
   `fired|clean|error`, matches with rule id and line, exit 0 when it ran. Ship it as a
   static musl binary, built in Alpine or with a musl cross toolchain, because the host
   toolchain cannot build it here.
2. The driver's classification rule above, with unit tests over captured `Output`
   values for every scenario row. Never map rc 1 to "fired".
3. A decision on corpus delivery. Read-only exists only for paths that the image
   provides. `--upload` into a `read_only` path fails, and an uploaded corpus
   stays writable (inferred: upload needs a `read_write` destination; untested). Options: build a per-replay image (cost unmeasured for real repos),
   or upload to `/sandbox` and accept a writable copy. Open.
4. Checker delivery: upload is cheap (~48 ms) and avoids an image per checker version;
   baking costs nothing per run but needs an image rebuild per checker release.
5. Always run `sandbox delete`: a failed `create --upload` leaves a `Ready` sandbox,
   and delete returns 0 for unknown names, so the cleanup is safe to repeat.
6. Sandbox names of 19 characters or fewer.
7. A "valid corpus" fixture set (PLAN's third condition). Not tested in this spike.
8. A label for runs without a gateway or Docker: a could-not-run status, never a pass.
   Windows and WSL 2 remain untested; the first spike's Windows and SDK notes live in git history.
