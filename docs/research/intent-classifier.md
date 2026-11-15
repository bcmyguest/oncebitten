# Intent classifier: int8 accuracy and first-use download

Stage 2 measurements on Linux x86_64. Model: `minishlab/potion-base-8M` via `model2vec-rs` 0.4.0. Fixture: `tests/fixtures/intent/labelled.tsv` (120 rows, 30 per class, split 15 dev / 15 test). Examples: `intent/examples.toml` (12 per scored class). No overlap between fixture and examples.

## Int8 model size

| Form | `model.safetensors` | Source |
|---|---|---|
| F32 | 30,236,760 B | HF download |
| Int8 with per-row scales | 7,677,440 B (7.3 MiB) | `scripts/intent-quantize.py` |

## Accuracy

Threshold per model chosen on dev (max macro-F1). Neutral is the fallback below threshold. Regex is the existing correction lexicon (`DEFAULT_CORRECTION_LEXICON`, correction class only, no threshold).

| Run | Split | Accuracy | Macro-F1 |
|---|---|---|---|
| F32 (threshold 0.37) | dev | 0.667 | 0.668 |
| Int8 (threshold 0.37) | dev | 0.683 | 0.683 |
| F32 | test | 0.467 | 0.473 |
| Int8 | test | 0.483 | 0.486 |
| Regex lexicon | dev | 0.367 | 0.244 |
| Regex lexicon | test | 0.250 | 0.168 |

Int8 stays within 2 points of F32 on both splits (test macro-F1 +1.3, accuracy +1.6).

Per-class test results:

| Class | F32 P / R | Int8 P / R | Regex P / R |
|---|---|---|---|
| rule-request | 0.667 / 0.267 | 0.667 / 0.267 | 0.000 / 0.000 |
| correction | 0.412 / 0.467 | 0.412 / 0.467 | 0.280 / 0.467 |
| frustration | 0.786 / 0.733 | 0.800 / 0.800 | 0.000 / 0.000 |
| neutral | 0.261 / 0.400 | 0.273 / 0.400 | 0.229 / 0.533 |

Reading: the embedding models classify poorly at this example count. Test macro-F1 is 0.47 to 0.49. The gap to F32 comes from the int8 quantization, not from the example set. The example set is the weak point: 12 examples per class, and rule-request recall is 0.27. Neutral has no examples and depends on the threshold.

Quantization check: the mean cosine between int8 and F32 query vectors is 1.0000 at four decimals. The file-loaded int8 model and the in-memory `quantize_int8()` model give identical scores.

## Commands

```
cargo build --release --example intent_eval
python3 scripts/intent-quantize.py <f32_model_dir> <int8_out_dir>
./target/release/examples/intent_eval <f32_model_dir> <int8_out_dir>
```

`<f32_model_dir>` holds `model.safetensors`, `tokenizer.json`, `config.json` from the HF repo.

## First-use download (sketch, not built)

Chosen option: download on first use, int8 preferred. Nothing is bundled in the packages.

- Cache location, resolved in this order:
  - Linux and macOS: `$XDG_CACHE_HOME/oncebitten/models/<model-id>/`, else `$HOME/.cache/oncebitten/models/<model-id>/`. Only absolute values count.
  - Windows: `%LOCALAPPDATA%\oncebitten\models\<model-id>\`.
  - Build paths with `std::path::PathBuf` and canonicalize with `dunce`. The repo already resolves `XDG_DATA_HOME` and `LOCALAPPDATA` in `src/sources.rs`.
- Checksum pinning: pin the SHA-256 of each file in source (`sha2` is already a dependency). Verify before the first load. On mismatch, delete the partial file and fall back to regex.
- Download: one HTTPS GET per file to a `.part` name, then rename. Open question: host the int8 file as a GitHub release asset or on HF. The HF URL is not under our control.
- Offline fallback: when the cache is empty and the download fails, the classifier reports `neutral` for every prompt. The regex lexicon keeps working. No error reaches the hook.
- Triggering the download: decide whether capture starts it or `oncebitten doctor` does. Capture must not block on network.

Open decisions for the user: hosting location for the int8 file, and whether first use runs in capture or in an explicit `oncebitten intent fetch` step.
