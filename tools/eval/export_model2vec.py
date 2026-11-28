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


MAX_HEADER_BYTES = 64 * 1024 * 1024
MAX_EMBEDDING_DIM = 1024
SUPPORTED_SCRIPTS = {"latin", "cyrillic"}


def load_safetensors_embeddings(path: Path) -> tuple[list[int], str, bytes]:
    file_size = path.stat().st_size
    if file_size < 8:
        raise RuntimeError(f"truncated safetensors file {path} ({file_size} bytes)")
    with path.open("rb") as fh:
        len_bytes = fh.read(8)
        if len(len_bytes) != 8:
            raise RuntimeError(f"failed to read 8-byte safetensors header length from {path}")
        header_len = struct.unpack("<Q", len_bytes)[0]
        if header_len == 0 or header_len > min(MAX_HEADER_BYTES, file_size - 8):
            raise RuntimeError(f"invalid safetensors header length {header_len} in {path}")
        header_raw = fh.read(header_len)
        if len(header_raw) != header_len:
            raise RuntimeError(f"truncated safetensors header in {path}")
        header = json.loads(header_raw.decode("utf-8"))
        info = header.get("embeddings")
        if not isinstance(info, dict):
            raise RuntimeError(f"'embeddings' tensor not found in {path}")
        dtype = str(info.get("dtype", ""))
        elem_size = {"F32": 4, "F16": 2}.get(dtype)
        if elem_size is None:
            raise RuntimeError(f"unsupported safetensors dtype '{dtype}' (expected F32 or F16)")
        shape = [int(x) for x in info.get("shape", [])]
        if len(shape) != 2 or shape[0] <= 0 or shape[1] <= 0:
            raise RuntimeError(f"invalid 2D embeddings shape={shape} in {path}")
        offsets = info.get("data_offsets")
        if not isinstance(offsets, list) or len(offsets) != 2:
            raise RuntimeError(f"invalid data_offsets in {path}")
        start, end = int(offsets[0]), int(offsets[1])
        data_region_len = file_size - 8 - header_len
        expected_bytes = shape[0] * shape[1] * elem_size
        if start < 0 or end < start or end > data_region_len or (end - start) < expected_bytes:
            raise RuntimeError(
                f"invalid embeddings data_offsets [{start}, {end}] for shape={shape} dtype={dtype}"
            )
        fh.seek(8 + header_len + start)
        raw = fh.read(end - start)
        if len(raw) < expected_bytes:
            raise RuntimeError(f"truncated embeddings tensor bytes ({len(raw)} < {expected_bytes})")
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


def matches_scripts(token: str, allowed_scripts: set[str]) -> bool:
    if not allowed_scripts:
        return True
    has_letter = False
    for ch in token:
        if not ch.isalpha():
            continue
        has_letter = True
        cp = ord(ch)
        is_latin = (0x0041 <= cp <= 0x024F) or (0x1E00 <= cp <= 0x1EFF)
        is_cyrillic = 0x0400 <= cp <= 0x052F
        if is_latin and "latin" in allowed_scripts:
            continue
        if is_cyrillic and "cyrillic" in allowed_scripts:
            continue
        return False
    return has_letter


def unpack_row(raw: bytes, row_idx: int, dim: int, dtype: str, out_dim: int) -> list[float]:
    if dtype == "F32":
        stride = dim * 4
        off = row_idx * stride
        vals = list(struct.unpack_from(f"<{out_dim}f", raw, off))
    elif dtype == "F16":
        stride = dim * 2
        off = row_idx * stride
        vals = list(struct.unpack_from(f"<{out_dim}e", raw, off))
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
    parser.add_argument(
        "--scripts",
        default="",
        help="Optional comma-separated script filter (e.g. 'latin,cyrillic') to prune unrelated scripts",
    )
    parser.add_argument(
        "--truncate-dim",
        type=int,
        default=0,
        help="Optional PCA prefix truncation dimension (0 = full model dim; re-normalized to unit L2)",
    )
    parser.add_argument("--no-ngram-fallback", action="store_true", help="Disable char-trigram OOV fallback")
    args = parser.parse_args()
    if args.max_tokens < 0:
        print("error: --max-tokens must be >= 0", file=sys.stderr)
        return 2
    if args.truncate_dim < 0:
        print("error: --truncate-dim must be >= 0", file=sys.stderr)
        return 2

    allowed_scripts = {s.strip().lower() for s in args.scripts.split(",") if s.strip()}
    unknown_scripts = sorted(allowed_scripts - SUPPORTED_SCRIPTS)
    if unknown_scripts:
        print(
            f"error: unsupported --scripts value(s): {', '.join(unknown_scripts)} "
            f"(supported: {', '.join(sorted(SUPPORTED_SCRIPTS))})",
            file=sys.stderr,
        )
        return 2

    shape, dtype, raw = load_safetensors_embeddings(args.model_dir / "model.safetensors")
    rows, dim = shape
    out_dim = args.truncate_dim if 0 < args.truncate_dim < dim else dim
    if out_dim > MAX_EMBEDDING_DIM:
        print(
            f"error: exported dim {out_dim} exceeds MAX_EMBEDDING_DIM ({MAX_EMBEDDING_DIM}); pass --truncate-dim <= {MAX_EMBEDDING_DIM}",
            file=sys.stderr,
        )
        return 2
    vocab = load_vocab(args.model_dir / "tokenizer.json")

    vectors: dict[str, list[float]] = {}
    for token, idx in vocab:
        if idx < 0 or idx >= rows:
            continue
        norm_tok = normalize_token(token)
        if norm_tok is None or norm_tok in vectors:
            continue
        if not matches_scripts(norm_tok, allowed_scripts):
            continue
        vec = unpack_row(raw, idx, dim, dtype, out_dim)
        if not vec:
            continue
        vectors[norm_tok] = vec
        if args.max_tokens > 0 and len(vectors) >= args.max_tokens:
            break

    args.out.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "dim": out_dim,
        "use_char_ngram_fallback": not args.no_ngram_fallback,
        "vectors": vectors,
    }
    args.out.write_text(json.dumps(payload, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    est_bytes = len(vectors) * (out_dim * 4 + 48)
    print(
        f"exported {len(vectors)} tokens (dim={out_dim}/{dim}, dtype={dtype}, scripts={sorted(allowed_scripts) or 'all'}, est_ram={est_bytes / (1024 * 1024):.2f} MiB) -> {args.out}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
