#!/usr/bin/env python3
"""Export a Model2Vec static embedding model (safetensors + tokenizer.json) into
`2brain experiment hybrid-eval --model-vectors` JSON format.

Zero heavy ML dependencies: reads safetensors and tokenizer.json using Python stdlib only.
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
import struct
import sys


def load_safetensors_embeddings(path: Path) -> tuple[list[int], str, bytes]:
    with path.open("rb") as fh:
        header_len = struct.unpack("<Q", fh.read(8))[0]
        header = json.loads(fh.read(header_len).decode("utf-8"))
        info = header.get("embeddings")
        if not isinstance(info, dict):
            raise RuntimeError(f"'embeddings' tensor not found in {path}")
        dtype = str(info["dtype"])
        shape = [int(x) for x in info["shape"]]
        start, end = info["data_offsets"]
        fh.seek(8 + header_len + int(start))
        raw = fh.read(int(end) - int(start))
    if len(shape) != 2:
        raise RuntimeError(f"expected 2D embeddings tensor, got shape={shape}")
    return shape, dtype, raw


def load_vocab(tokenizer_path: Path) -> list[tuple[str, int]]:
    tok = json.loads(tokenizer_path.read_text(encoding="utf-8"))
    vocab = tok.get("model", {}).get("vocab")
    pairs: list[tuple[str, int]] = []
    if isinstance(vocab, dict):
        for token, idx in vocab.items():
            pairs.append((str(token), int(idx)))
    elif isinstance(vocab, list):
        for idx, entry in enumerate(vocab):
            if isinstance(entry, list) and entry:
                pairs.append((str(entry[0]), idx))
            elif isinstance(entry, str):
                pairs.append((entry, idx))
    else:
        raise RuntimeError(f"unsupported tokenizer.json vocab structure in {tokenizer_path}")
    pairs.sort(key=lambda p: p[1])
    return pairs


def normalize_token(raw: str) -> str | None:
    if not raw or (raw.startswith("[") and raw.endswith("]")) or (raw.startswith("<") and raw.endswith(">")):
        return None
    if raw.startswith("##"):
        # WordPiece continuation subwords do not match standalone whitespace/unicode tokens in tb-syntax.
        return None
    cleaned = raw.lstrip("\u2581").strip().lower()
    if len(cleaned) < 2:
        return None
    if not any(ch.isalnum() for ch in cleaned):
        return None
    return cleaned


def unpack_row(raw: bytes, row_idx: int, dim: int, dtype: str) -> list[float]:
    if dtype == "F32":
        stride = dim * 4
        off = row_idx * stride
        vals = list(struct.unpack_from(f"<{dim}f", raw, off))
    elif dtype == "F16":
        stride = dim * 2
        off = row_idx * stride
        vals = list(struct.unpack_from(f"<{dim}e", raw, off))
    else:
        raise RuntimeError(f"unsupported safetensors dtype '{dtype}' (expected F32 or F16)")
    norm = math.sqrt(sum(v * v for v in vals))
    if not math.isfinite(norm) or norm <= 1e-12:
        return []
    inv = 1.0 / norm
    return [round(v * inv, 6) for v in vals]


def main() -> int:
    parser = argparse.ArgumentParser(description="Export Model2Vec safetensors to 2brain --model-vectors JSON")
    parser.add_argument("model_dir", type=Path, help="Directory containing model.safetensors and tokenizer.json")
    parser.add_argument("--out", type=Path, required=True, help="Output JSON path")
    parser.add_argument("--max-tokens", type=int, default=0, help="Optional cap on exported tokens (0 = all)")
    parser.add_argument("--no-ngram-fallback", action="store_true", help="Disable char-trigram OOV fallback")
    args = parser.parse_args()

    shape, dtype, raw = load_safetensors_embeddings(args.model_dir / "model.safetensors")
    rows, dim = shape
    vocab = load_vocab(args.model_dir / "tokenizer.json")

    vectors: dict[str, list[float]] = {}
    for token, idx in vocab:
        if idx < 0 or idx >= rows:
            continue
        norm_tok = normalize_token(token)
        if norm_tok is None or norm_tok in vectors:
            continue
        vec = unpack_row(raw, idx, dim, dtype)
        if not vec:
            continue
        vectors[norm_tok] = vec
        if args.max_tokens > 0 and len(vectors) >= args.max_tokens:
            break

    args.out.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "dim": dim,
        "use_char_ngram_fallback": not args.no_ngram_fallback,
        "vectors": vectors,
    }
    args.out.write_text(json.dumps(payload, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    est_bytes = len(vectors) * (dim * 4 + 48)
    print(
        f"exported {len(vectors)} tokens (dim={dim}, dtype={dtype}, est_ram={est_bytes / (1024 * 1024):.2f} MiB) -> {args.out}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
