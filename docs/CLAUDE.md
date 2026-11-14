# Notes for agents

- `Inputs::from_process` is the only function that reads the process environment. Tests build `Inputs` directly.
- Project discovery stops at the first ancestor that holds `.git`. Test temp directories sit under `/tmp`, and this machine has `/tmp/.git`, so do not add a test that expects "no git root" for a temp path.
- `Config` deserialization validates each file and each environment layer, then the merged result. Add a validation rule to `Config::validate` or a type, not to the merge code.
- Config keys are two levels deep (`section.key`). `set_leaf` and `flatten` assume that shape.
