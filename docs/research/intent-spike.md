# Spike: intent classifier embedding model

Scope: can `model2vec-rs` with `minishlab/potion-base-8M` load cold in under ~300 ms and fit the npm and PyPI packages? Measured on Linux x86_64 (Ubuntu 24.04 kernel 6.17, rustc 1.97.1). macOS and Windows: not measured.

## Results

| Check | Result | Limit | Verdict |
|---|---|---|---|
| Crate | `model2vec-rs` 0.4.0, LICENSE file MIT | Apache-2.0 project | Compatible. Cargo.toml has no SPDX `license` field (`cargo info` shows `unknown`). |
| Model | `minishlab/potion-base-8M`, license `mit` (HF API `cardData`) | redistributable | Compatible |
| Model size, raw | `model.safetensors` 30,236,760 B (F32 `[29528, 256]`), `tokenizer.json` 683,666 B, `config.json` 202 B. Total 30.9 MB | npm and PyPI: 100 MB default PyPI file limit | Fits |
| Model size, compressed | gzip 28,133,153 B, xz 28,014,044 B | none | Compression saves 9%. Not worth it. |
| Model size, int8 | Not written to disk. Table bytes estimated at 29,528 × 256 B ≈ 7.6 MB, plus per-row scales. | none | Estimate, not measured |
| Cold load, fresh process, F32 (10 runs) | 44.3, 45.0, 45.7, 46.3, 49.7, 56.2, 68.7, 69.2, 76.3, 83.0 ms (median ≈ 53 ms) | < 300 ms | Passes |
| Cold load + `quantize_int8`, in memory (10 runs) | load 33–50 ms, quantize 19–38 ms | < 300 ms | Passes |
| First encode after load | 0.03–0.05 ms (one 0.31 ms outlier) | — | — |
| Per-prompt encode, steady state | p50 0.001 ms, p95 ≤ 0.004 ms (160 calls per process) | — | Passes |
| Whole process, 160 encodes | wall 0.03 s, max RSS 67.8 MB (`/usr/bin/time -v`) | — | — |
| Build with `default-features = false, features = ["onig"]` | Builds, runs offline, binary 6.2 MB (default features: 9.1 MB) | no network at runtime | Use the local-only build |
| Dependency count | 166 unique normal dependencies (`cargo tree -e normal`) | — | Licences of dependencies not audited |
| Semantic sanity | cos(“stop doing that, I told you to use the existing helper”, “from now on always run cargo fmt before committing”) = 0.249; cos(same prompt, unrelated weather sentence) = −0.118 | — | Ordering holds on one pair. Classification accuracy not measured. |

Gate result: load and latency pass by a wide margin. Model and crate licences pass. Sizes fit both registries.

## Decisions for stage 2

- Use `default-features = false` with `onig` for the shipped binary. The model is bundled, so runtime needs no HF download.
- Quantize to int8 only if the packaged size matters. F32 costs 30 MB; int8 costs about 8 MB by estimate.
- The packages ship as five platform binaries (`packaging/npm`, `packaging/pypi`, `build.yml`). The model adds ~30 MB to each platform package, or ~8 MB if int8.

## Unverified

- Cold-start numbers on macOS and Windows.
- Classification accuracy of the static embeddings on real prompts. Stage 2 must measure this.
- Dependency licence audit of the 166 transitive crates.
- The int8 on-disk size (estimate only).

## Commands

Model download:

```sh
mkdir -p model
for f in model.safetensors tokenizer.json config.json; do
  curl -sSL -o model/$f https://huggingface.co/minishlab/potion-base-8M/resolve/main/$f
done
curl -sSL https://huggingface.co/api/models/minishlab/potion-base-8M   # cardData.license = mit
```

Bench crate (kept outside the repo, at `/home/b/oncebitten-intent-spike-data/bench`). `Cargo.toml` depends on `model2vec-rs = "0.4.0"`. `src/main.rs` loads with `StaticModel::from_pretrained(dir, None, None, None)`, optionally calls `quantize_int8()`, times the first `encode_single`, then times 160 `encode_single` calls and prints p50 and p95.

```sh
cargo build --release
for i in $(seq 10); do ./target/release/intent-bench model; done        # f32
for i in $(seq 10); do ./target/release/intent-bench model int8; done   # int8 in memory
/usr/bin/time -v ./target/release/intent-bench model 2>&1 | grep -E "Maximum resident|Elapsed"
gzip -c model/model.safetensors | wc -c
xz -T0 -c model/model.safetensors | wc -c
cargo tree -e normal --prefix none | sort -u | wc -l
```

Local-only build: same bench with `model2vec-rs = { version = "0.4.0", default-features = false, features = ["onig"] }`.
