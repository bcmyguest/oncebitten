"""Write an int8 copy of a model2vec safetensors table with per-row scales.

Usage: python3 -I scripts/intent-quantize.py <model_dir> <out_dir>

Copies tokenizer.json and config.json. Writes model.safetensors with
`embeddings` (I8, [rows, dim]) and `weights` (F32, [rows]) where each
weight is the row scale max(|row|) / 127. Rounding matches Rust's
f32::round (half away from zero), as in model2vec-rs quantize_rows.
"""

import pathlib
import shutil
import sys

import numpy as np
from safetensors.numpy import load_file, save_file


def quantize(table: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    maxes = np.abs(table).max(axis=1)
    scales = (maxes / 127.0).astype(np.float32)
    safe = np.where(scales > 0.0, scales, 1.0)
    scaled = table / safe[:, None]
    rounded = np.sign(scaled) * np.floor(np.abs(scaled) + 0.5)
    q = np.clip(rounded, -127, 127).astype(np.int8)
    q[scales == 0.0] = 0
    return q, scales


def main(src: str, dst: str) -> None:
    src_dir, dst_dir = pathlib.Path(src), pathlib.Path(dst)
    dst_dir.mkdir(parents=True, exist_ok=True)
    table = load_file(str(src_dir / "model.safetensors"))["embeddings"].astype(np.float32)
    q, scales = quantize(table)
    save_file({"embeddings": q, "weights": scales}, str(dst_dir / "model.safetensors"))
    for name in ("tokenizer.json", "config.json"):
        shutil.copy(src_dir / name, dst_dir / name)
    print(f"rows={q.shape[0]} dim={q.shape[1]} bytes={(dst_dir / 'model.safetensors').stat().st_size}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
