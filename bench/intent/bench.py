"""Run `examples/intent_eval --json` and log one MLflow run per classifier variant.

Usage (from bench/intent):
    uv run bench.py --f32-dir <dir> --int8-dir <dir>

Model dirs also come from INTENT_F32_DIR and INTENT_INT8_DIR. Tracking URI comes from
MLFLOW_TRACKING_URI, else a local SQLite store at bench/intent/mlruns/mlflow.db with
artifacts under bench/intent/mlruns/artifacts. MLflow 3.18 refuses the plain file store
(`./mlruns` directories) unless MLFLOW_ALLOW_FILE_STORE=true.

Selection uses dev only: among variants whose dev neutral FPR is at most the eval's cap,
the highest dev macro-F1 wins. Ties go to the smaller model, then nn before knn, then
smaller k, then threshold mode name. The test split is reported, never used to choose.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import mlflow

BENCH_DIR = Path(__file__).resolve().parent
REPO = BENCH_DIR.parent.parent
FIXTURE_DIR = "tests/fixtures/intent"
EXPERIMENT = "oncebitten-intent"
LOCAL_STORE = BENCH_DIR / "mlruns"
SPLITS = ("dev", "test")


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True
    ).stdout.strip()


def run_eval(f32_dir: str, int8_dir: str, report: Path) -> str:
    report.parent.mkdir(parents=True, exist_ok=True)
    command = [
        "cargo", "run", "--release", "--quiet", "--example", "intent_eval", "--",
        f32_dir, int8_dir, "--json", str(report),
    ]
    result = subprocess.run(command, cwd=REPO, check=True, capture_output=True, text=True)
    return result.stdout


def selection_key(variant: dict) -> tuple:
    dev = variant["splits"]["dev"]
    return (
        -dev["macro_f1"],
        variant["model_safetensors_bytes"],
        0 if variant["classifier"] == "nn" else 1,
        variant["k"] or 0,
        variant["threshold_mode"],
    )


def select_on_dev(variants: list[dict], fpr_cap: float) -> dict:
    eligible = [v for v in variants if v["splits"]["dev"]["neutral_fpr"] <= fpr_cap]
    if not eligible:
        sys.exit(f"no variant reaches dev neutral FPR <= {fpr_cap}")
    return min(eligible, key=selection_key)


def confusion_tsv(confusion: dict) -> str:
    labels = confusion["labels"]
    lines = ["gold\\pred\t" + "\t".join(labels)]
    for label, row in zip(labels, confusion["matrix"]):
        lines.append(label + "\t" + "\t".join(str(n) for n in row))
    return "\n".join(lines) + "\n"


def log_variant(variant: dict, common: dict, model_id: str, selected: bool) -> None:
    params = {
        **common,
        "model": model_id if variant["quant"] else "regex-lexicon",
        "quant": variant["quant"] or "none",
        "classifier": variant["classifier"],
        "k": variant["k"] if variant["k"] is not None else "none",
        "k_selected_on_dev": variant["k_selected_on_dev"],
        "threshold_mode": variant["threshold_mode"],
        "threshold": variant["threshold"] if variant["threshold"] is not None else "none",
        "model_dir": variant["model_dir"] or "none",
    }
    metrics = {
        "latency_p50_us": variant["latency_us"]["p50"],
        "latency_p95_us": variant["latency_us"]["p95"],
        "latency_mean_us": variant["latency_us"]["mean"],
        "model_safetensors_bytes": variant["model_safetensors_bytes"],
        "model_dir_bytes": variant["model_dir_bytes"],
    }
    for split in SPLITS:
        report = variant["splits"][split]
        for name in ("accuracy", "macro_f1", "neutral_fpr"):
            metrics[f"{split}_{name}"] = report[name]
        for label, scores in report["per_class"].items():
            for name in ("precision", "recall", "f1"):
                metrics[f"{split}_{label}_{name}"] = scores[name]
    with mlflow.start_run(run_name=variant["name"]):
        mlflow.log_params(params)
        mlflow.log_metrics(metrics)
        mlflow.set_tag("selected_dev_fpr_capped", str(selected).lower())
        for split in SPLITS:
            confusion = variant["splits"][split]["confusion"]
            mlflow.log_dict(confusion, f"confusion_{split}.json")
            mlflow.log_text(confusion_tsv(confusion), f"confusion_{split}.tsv")


def print_table(variants: list[dict], winner: dict) -> None:
    print("| Variant | Threshold | Test macro-F1 | Test neutral FPR | Dev macro-F1 "
          "| Dev neutral FPR | p50 us | p95 us | Weights bytes |")
    print("|---|---|---|---|---|---|---|---|---|")
    for v in variants:
        dev, test = v["splits"]["dev"], v["splits"]["test"]
        mark = " (selected)" if v is winner else ""
        threshold = "-" if v["threshold"] is None else f"{v['threshold']:.2f}"
        print(f"| {v['name']}{mark} | {threshold} | {test['macro_f1']:.3f} "
              f"| {test['neutral_fpr']:.3f} | {dev['macro_f1']:.3f} | {dev['neutral_fpr']:.3f} "
              f"| {v['latency_us']['p50']:.1f} | {v['latency_us']['p95']:.1f} "
              f"| {v['model_safetensors_bytes']} |")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--f32-dir", default=os.environ.get("INTENT_F32_DIR"))
    parser.add_argument("--int8-dir", default=os.environ.get("INTENT_INT8_DIR"))
    parser.add_argument("--model-id", default="minishlab/potion-base-8M")
    parser.add_argument("--experiment", default=EXPERIMENT)
    parser.add_argument("--report", type=Path, default=BENCH_DIR / "out" / "report.json")
    args = parser.parse_args()
    if not args.f32_dir or not args.int8_dir:
        parser.error("pass --f32-dir and --int8-dir, or set INTENT_F32_DIR and INTENT_INT8_DIR")

    text = run_eval(args.f32_dir, args.int8_dir, args.report)
    data = json.loads(args.report.read_text())
    variants = data["variants"]
    winner = select_on_dev(variants, data["hook_max_neutral_fpr"])

    common = {
        "fixture_git_sha": git("log", "-1", "--format=%H", "--", FIXTURE_DIR),
        "fixture_dirty": bool(git("status", "--porcelain", "--", FIXTURE_DIR)),
        "code_git_sha": git("rev-parse", "HEAD"),
        "code_dirty": bool(git("status", "--porcelain", "--", "examples", "src")),
        "bench_id": time.strftime("%Y%m%dT%H%M%S"),
        "latency_passes": data["latency_passes"],
        "hook_max_neutral_fpr": data["hook_max_neutral_fpr"],
    }
    for fixture in data["fixtures"]:
        common[f"rows_{Path(fixture['path']).stem}"] = fixture["rows"]
    for split, per_class in data["rows"].items():
        common[f"rows_{split}"] = sum(per_class.values())
        for label, n in per_class.items():
            common[f"rows_{split}_{label}"] = n

    uri = os.environ.get("MLFLOW_TRACKING_URI")
    if uri:
        mlflow.set_tracking_uri(uri)
    else:
        LOCAL_STORE.mkdir(exist_ok=True)
        uri = f"sqlite:///{LOCAL_STORE / 'mlflow.db'}"
        mlflow.set_tracking_uri(uri)
        if mlflow.get_experiment_by_name(args.experiment) is None:
            artifacts = (LOCAL_STORE / "artifacts").as_uri()
            mlflow.create_experiment(args.experiment, artifact_location=artifacts)
    mlflow.set_experiment(args.experiment)
    for variant in variants:
        log_variant(variant, common, args.model_id, variant is winner)
    with mlflow.start_run(run_name=f"report {common['bench_id']}"):
        mlflow.log_params(common)
        mlflow.set_tag("kind", "report")
        mlflow.log_text(text, "intent_eval.txt")
        mlflow.log_artifact(str(args.report))

    print(f"logged {len(variants)} variant runs to {uri}, experiment {args.experiment}")
    print(f"selected on dev (neutral FPR <= {data['hook_max_neutral_fpr']}): {winner['name']}")
    print_table(variants, winner)


if __name__ == "__main__":
    main()
