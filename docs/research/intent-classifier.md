# Intent classifier: accuracy and model delivery

Stage 2 measurements on Linux x86_64. Model: `minishlab/potion-base-8M` via `model2vec-rs` 0.4.0. Classes: `rule-request`, `correction`, `frustration`, and `neutral`. The code has no repeated-mistake class.

## Data

| File | Rows | Per class (dev / test) |
|---|---|---|
| `intent/examples.toml` (shipped prototypes) | 36 | 12 per scored class, no neutral |
| `tests/fixtures/intent/labelled.tsv` | 120 | 15 / 15 for each of the four classes |
| `tests/fixtures/intent/labelled-extra.tsv` | 136 | 10 / 10 per scored class, 38 / 38 neutral |
| Both fixtures | 256 | 25 / 25 per scored class, 53 / 53 neutral |

Provenance: a language model (Claude) hand-wrote every example and fixture row. No row comes from real user data or real session logs. The extra rows imitate agent-session messages: terse, lowercase, typos, non-native phrasing, inline code, one-line pasted compiler and runtime logs. Neutral covers ordinary instructions, questions, approvals (`1 a 2 b`, `option B pls`), status requests, and hard negatives that contain trigger words (`never mind the windows build`, `stop here, I want to review first`, `try again with the verbose flag`). TSV rows hold one line each, so the fixture has no multi-line pastes.

The extra rows sit in a second file because `tests/intent_fixture.rs` pins `labelled.tsv` at 15 rows per class and split. Splits alternate within each class, so both files stay stratified. New tests check the combined fixtures for equal dev and test counts per class, at least 50 rows per scored class and 100 neutral, no duplicate texts, and no overlap with the shipped examples.

## Int8 model size

| Form | `model.safetensors` | Source |
|---|---|---|
| F32 | 30,236,760 B | HF download |
| Int8 with per-row scales | 7,677,440 B (7.3 MiB) | `scripts/intent-quantize.py` |

## Accuracy

Two classifiers per model, both tuned on dev only and reported on test:

- **nn**: nearest shipped example by cosine; neutral below a threshold.
- **knn**: k nearest neighbours over the 36 shipped examples plus the 128 dev rows, neutral rows included. Votes sum cosine scores per class. A non-neutral winner below the threshold becomes neutral. Dev rows score leave-one-out. k = 5 won on dev from {1, 3, 5, 7, 9} for both models.

Each gets two thresholds from dev: **max-F1** (max macro-F1) and **hook** (lowest threshold with dev neutral false-positive rate at most 5%). Neutral FPR is the share of neutral rows given any other class: how often a hook would flag an ordinary message. Regex is `DEFAULT_CORRECTION_LEXICON`, correction class only, no threshold.

Test split, 128 rows (25 per scored class, 53 neutral):

| Run | Threshold | Accuracy | Macro-F1 | Neutral FPR |
|---|---|---|---|---|
| F32 nn | max-F1 0.37 | 0.516 | 0.505 | 0.453 |
| Int8 nn | max-F1 0.37 | 0.523 | 0.513 | 0.453 |
| F32 knn k=5 | max-F1 0.35 | 0.586 | 0.583 | 0.415 |
| Int8 knn k=5 | max-F1 0.38 | 0.586 | 0.577 | 0.377 |
| F32 nn | hook 0.52 | 0.508 | 0.345 | 0.000 |
| Int8 nn | hook 0.52 | 0.508 | 0.345 | 0.000 |
| F32 knn k=5 | hook 0.57 | 0.461 | 0.264 | 0.038 |
| Int8 knn k=5 | hook 0.57 | 0.461 | 0.264 | 0.038 |
| Regex lexicon | none | 0.445 | 0.252 | 0.151 |

Dev macro-F1 at max-F1: F32 nn 0.578, int8 nn 0.587, F32 knn 0.664, int8 knn 0.666, regex 0.270.

Per-class test precision / recall at the max-F1 threshold:

| Class | F32 nn | Int8 nn | F32 knn | Int8 knn | Regex |
|---|---|---|---|---|---|
| rule-request | 0.375 / 0.240 | 0.375 / 0.240 | 0.500 / 0.360 | 0.500 / 0.320 | 0.000 / 0.000 |
| correction | 0.464 / 0.520 | 0.464 / 0.520 | 0.441 / 0.600 | 0.469 / 0.600 | 0.308 / 0.480 |
| frustration | 0.720 / 0.720 | 0.731 / 0.760 | 0.833 / 0.800 | 0.826 / 0.760 | 0.000 / 0.000 |
| neutral | 0.492 / 0.547 | 0.500 / 0.547 | 0.596 / 0.585 | 0.579 / 0.623 | 0.506 / 0.849 |

Per-class test recall at the hook threshold (F32 and int8 identical): nn rule-request 0.080, correction 0.040, frustration 0.360; knn rule-request 0.000, correction 0.000, frustration 0.320.

Previous run, 120-row fixture only: test macro-F1 F32 0.473, int8 0.486, regex 0.168.

Reading:

- Neither classifier is fit to gate a hook. At the max-F1 threshold the best run flags 38% of ordinary messages. At the 5% FPR threshold recall falls to 0 to 0.36 per class, and only frustration survives.
- kNN with dev prototypes raises test macro-F1 by 6 to 8 points over nn and cuts neutral FPR from 0.453 to 0.415 (F32) and 0.377 (int8). Neutral and frustration precision rise most. No ablation separates the effect of neutral prototypes from that of more prototypes.
- Rule-request recall stays under 0.4 in every run, and correction recall stays at or under 0.6. The 8M static model barely separates "do X from now on" from "do X".
- The regex lexicon flags 15% of neutral rows and finds 48% of corrections. It has no rule-request or frustration coverage.
- Int8 stays within 1 point of F32 on test accuracy and macro-F1 in every run. Per-class and FPR figures differ by up to 4 points, one or two rows at this size.
- 25 test rows per scored class put each recall figure at a resolution of 0.04. Differences under about 0.1 are noise.

Quantization check: the mean cosine between int8 and F32 query vectors is 1.0000 at four decimals. The file-loaded int8 model and the in-memory `quantize_int8()` model give identical scores.

## Commands

```
cargo build --release --example intent_eval
python3 scripts/intent-quantize.py <f32_model_dir> <int8_out_dir>
./target/release/examples/intent_eval <f32_model_dir> <int8_out_dir>   # reads both fixtures
```

`<f32_model_dir>` holds `model.safetensors`, `tokenizer.json`, `config.json` from the HF repo.

## Model delivery (decided, not built)

The int8 model ships as a GitHub release asset on the oncebitten repo. Source code pins the SHA-256 of each file. Hugging Face hosts nothing that oncebitten downloads; the maintainer builds the asset from the HF F32 model with `scripts/intent-quantize.py`.

- Cache location, resolved in this order:
  - Linux and macOS: `$XDG_CACHE_HOME/oncebitten/models/<model-id>/`, else `$HOME/.cache/oncebitten/models/<model-id>/`. Only absolute values count.
  - Windows: `%LOCALAPPDATA%\oncebitten\models\<model-id>\`.
  - Build paths with `std::path::PathBuf` and canonicalize with `dunce`. The repo already resolves `XDG_DATA_HOME` and `LOCALAPPDATA` in `src/sources.rs`.
- Trigger: the first capture that wants the model starts the download in a detached background process and returns at once, as `start_background_triage` in `src/capture.rs` does for triage. The hook never waits on the network.
- Download: one HTTPS GET per file to a `.part` name, verify SHA-256 (`sha2` is already a dependency), then rename into place. On mismatch, delete the file; a later capture retries.
- Until the file exists and its checksum matches, capture classifies with the regex lexicon only. A missing model, a running download, or a failed download produce no error in the hook.
