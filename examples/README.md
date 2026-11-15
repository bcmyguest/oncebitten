# Example rules

oncebitten ships these rules inside its binary. None runs until you turn it on.

| Id | Engine | Fires when |
|---|---|---|
| `py-utcnow` | ast-grep | Python calls `datetime.utcnow()` |
| `py-no-print` | ast-grep | Python calls `print` |
| `js-moment` | ast-grep | JavaScript imports or requires `moment` |
| `ts-no-as-any` | ast-grep | TypeScript casts to `any` |
| `rust-no-unwrap` | native | `cargo clippy` reports `clippy::unwrap_used` |
| `arch-diagram` | diff | `src/lib.rs` or a `src/**/mod.rs` changes and no `docs/architecture/*.svg` or `*.png` does |
| `ui-screenshot` | diff | `src/ui/**` changes and no `docs/screenshots/*.png` does |

## Turn one on

Name it in `.oncebitten/config.toml` at the repository root:

```toml
[rules]
use_examples = ["py-utcnow", "ui-screenshot"]
```

A listed example runs as active under the id `examples/<id>`. Check your changes against it:

```sh
oncebitten rules list     # shows examples/py-utcnow  block  active  examples
oncebitten check          # changes since HEAD
```

## Change one

Copy its directory into `.oncebitten/rules/` under a new id, set that id in `rule.toml`, and edit it there:

```sh
cp -r examples/py-utcnow .oncebitten/rules/team-utcnow
```

Set `status = "active"` in the copy to enable it.

## Gate on one

`oncebitten gate pr` needs a proof for each active ast-grep rule. A proof written for a shipped example stays on the machine that wrote it, so copy the example into `.oncebitten/rules/` first, then prove the copy and commit the proof:

```sh
oncebitten replay team-utcnow     # runs in an OpenShell sandbox; writes proof.json
git add .oncebitten/rules/team-utcnow
```

Replay needs a non-empty `corpus/` of clean files. Of the shipped examples, only `py-utcnow` has one; give a copy of another example its own corpus before you replay it.

## Layout

```
py-utcnow/
  rule.toml          # id, mode, status, engine, language
  check.yml          # ast-grep rule (ast-grep engine only)
  fixtures/
    bad.py           # must fire
    fixed.py         # must stay clean
  corpus/            # clean files that must not fire (replay)
```

`diff` rules list changed paths in `fixtures/bad.diff` and `fixtures/fixed.diff`, one per line. `rust-no-unwrap` runs a command; its fixtures are crate roots. Every field: [docs/rules.md](../docs/rules.md).
