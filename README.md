# oncebitten

oncebitten turns repeated agent mistakes into rules that git enforces. A mistake of the same class that recurs twice, or a rule the user states ("never push to main"), becomes a check that runs at pre-push, in CI and at PR creation, so a violation fails before human review. Prompt instructions bind an agent only when it reads them; a git gate binds every push.

Status: pre-alpha, step 0 of [PLAN.md](PLAN.md). Only the scaffold and layered configuration exist.

## Install from source

```sh
git clone https://github.com/bcmyguest/oncebitten
cd oncebitten
cargo install --path .
```

## Usage

```sh
oncebitten config show            # merged configuration as TOML
oncebitten config show --origin   # annotate each value with its layer
oncebitten config path            # configuration files it reads
```

Settings and their precedence: [docs/configuration.md](docs/configuration.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
