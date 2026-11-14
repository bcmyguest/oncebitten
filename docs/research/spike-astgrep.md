# Spike: embedding ast-grep as a rule engine

Scope: PLAN.md step 1, ast-grep half. Throwaway code lives in `/tmp/spike-astgrep` (not in the repo).
Tested 2026-11-14 with ast-grep-* 0.51.2 (MSRV 1.90), rustc 1.97.1, Linux.

## Verdict

Feasible. The library loads ast-grep YAML rules, matches Python, Rust, JavaScript and TypeScript,
and returns file:line plus the rendered message. All four rule groups fire on bad fixtures and stay
silent on fixed fixtures (`/tmp/spike-astgrep/check.sh`: 8 bad, 7 fixed, all pass).
Two real limits: no type information, and pattern matching does not see inside macro arguments.

## Numbers

| Build | Crates (transitive) | Clean release build (wall) | Binary, unstripped | Binary, stripped |
|---|---|---|---|---|
| py + rs + js + ts (`default-features = false`) | 76 | 9.7 s | 11.3 MB | 9.8 MB |
| all builtin grammars (default features) | 100 | 20.1 s | 49.5 MB | 47.8 MB |

Machine: this sandbox, `cargo build --release`, empty target dir.

## Crates

- `ast-grep-core` 0.51.2: tree-sitter AST, `Pattern`, `Matcher`. `LanguageExt` (provides `ast_grep(src)`)
  lives at `ast_grep_core::tree_sitter::LanguageExt`, not at the crate root.
- `ast-grep-config` 0.51.2: `from_yaml_string::<SupportLang>(yaml, &GlobalRules::default())` returns
  `Vec<RuleConfig>`. `RuleCollection::try_new` then `for_path(file)` selects rules by language and
  `files`/`ignores` globs. `RuleConfig::get_message(&match)` renders metavariables.
- `ast-grep-language` 0.51.2: `SupportLang` and `SupportLang::from_path`. Each grammar sits behind a
  `tree-sitter-<lang>` feature. Default enables all builtin grammars.

## Rules (verified: load and fire)

`rules/py-utcnow.yml`
```yaml
id: py-utcnow
language: python
severity: error
message: use datetime.now(UTC)
rule:
  any:
    - pattern: datetime.utcnow()
    - pattern: datetime.datetime.utcnow()
```

`rules/rust-unwrap.yml`
```yaml
id: rust-unwrap
language: rust
severity: error
message: use ? or expect with context
rule:
  pattern: $RECV.unwrap()
  not:
    any:
      - inside:
          kind: mod_item
          stopBy: end
          follows:
            kind: attribute_item
            regex: 'cfg\(test\)'
            stopBy: neighbor
      - inside:
          kind: function_item
          stopBy: end
          follows:
            kind: attribute_item
            regex: '^#\[test\]$'
            stopBy: neighbor
```

`rules/js-moment.yml`
```yaml
id: js-moment
language: javascript
severity: error
message: use date-fns
rule:
  any:
    - kind: import_statement
      has:
        field: source
        regex: '^["'']moment["'']$'
    - pattern: require("moment")
    - pattern: require('moment')
```

`rules/{py,rust,js}-no-comments.yml`: see "Comments" below for the allowlist form.

## Fixtures

| Rule | Bad (must match) | Fixed (must not match) |
|---|---|---|
| py-utcnow | `import datetime` + `datetime.datetime.utcnow()`; `from datetime import datetime` + `datetime.utcnow()` | `from datetime import UTC, datetime` + `datetime.now(UTC)` |
| rust-unwrap | `fs::read_to_string(path).unwrap()` | `?` and `.expect("...")`; `#[cfg(test)] mod` and top-level `#[test] fn` with `.unwrap()` (exclusion) |
| js-moment | `import moment from "moment";`; `const moment = require('moment');` | `import { format } from "date-fns";` |
| no-comments | file with shebang, license, `# type: ignore`, `# noqa`, `# pragma`, one `# plain note` | comment-free file |

Controls: `import _ from "lodash"` and `require("moment-timezone")` do not match. A string literal
containing `datetime.utcnow()` does not match.

## Comments: what grammars expose

- Python: `comment` node. Shebang is a `comment` (flagged unless allowlisted). Docstrings are `string`, not comments.
- Rust: `line_comment` (includes `///` and `//!`) and `block_comment`. `#[allow]` is an attribute, not a comment.
- JavaScript (tree-sitter-javascript): `comment` for `//` and `/* */` and `/** */`. Shebang is `hash_bang_line`, not a comment, so it never matches (verified).

Allowlist works in rule YAML. `not: regex:` tests the node text, so the rule skips pragmas and headers:

```yaml
# rules/py-no-comments.yml
id: no-comments-py
language: python
severity: warning
message: remove comment
rule:
  kind: comment
  not:
    regex: '^#!|type:|noqa|pragma:|Copyright|SPDX|License'
```

```yaml
# rules/rust-no-comments.yml
id: no-comments-rust
language: rust
severity: warning
message: remove comment
rule:
  any:
    - kind: line_comment
    - kind: block_comment
  not:
    regex: '^///|^//!|^/\*\*|SAFETY:|Copyright|SPDX|License'
```

```yaml
# rules/js-no-comments.yml
id: no-comments-js
language: javascript
severity: warning
message: remove comment
rule:
  kind: comment
  not:
    regex: '^/\*\*|eslint|@ts-|@flow|Copyright|SPDX|License'
```

Result on the comment fixture: only `# plain note` (py), `/* block */` (rs) and `// trailing` (js) fire.
Position-based allowlists (license header only at file top) were not tested. The regex route is the
verified one. Caveat: a regex on text also exempts any comment that merely mentions "License".

## Limits (verified)

- No types. `.unwrap()` matches both `Option` and `Result`. Both variants fired in the test.
- Macro arguments are invisible. `println!("{}", v.unwrap())` and `assert_eq!(v.unwrap(), 3)` do not match
  (tree-sitter keeps macro bodies as token trees).
- The test exclusion covers `#[test]` and `#[cfg(test)] mod`. It does not cover `#[tokio::test]` or other
  attribute forms (not tested).

## Library vs CLI

Verified parity: `ast-grep scan` (CLI 0.51.2) and the spike binary report the same results on the
comment-rule and unwrap-exclusion cases.

`ast-grep test` (CLI only) reads rule-test YAML with inline snippets:

```yaml
id: py-utcnow
valid:
  - "from datetime import UTC, datetime\ndatetime.now(UTC)"
invalid:
  - "import datetime\ndatetime.datetime.utcnow()"
```

Verified: the format works for our rules, and a valid case that matches makes it fail. It needs
snapshot files (`rule-tests/__snapshots__/`) created with `--update-all`. A fresh run fails until then.
Its fixtures are inline snippets, not files, so our file-based bad/fixed layout does not map 1:1.
Usable for rule tests; not a replacement for file fixtures.

Library-only, present in source but not exercised: `CombinedScan` (suppression comments, no-suppress-all),
`RuleCollection` `files`/`ignores` scoping (code path read, not run), auto-fix via `Fixer`.
CLI-only: `sgconfig.yml`, `ast-grep test`, snapshots, `--json`, `--update-all`.

## Assumptions not verified

- Rule YAML from the docs site matches `from_yaml_string` for every key. Only the keys above were run.
- Relational `follows` with `stopBy: neighbor` depends on tree-sitter-rust sibling layout. Verified on
  this grammar version only.
- Build times exclude a cold crates.io download.
