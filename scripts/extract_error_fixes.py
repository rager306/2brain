#!/usr/bin/env python3
"""Declarative recurring-error extractor -> `2brain ingest --format error-fix` (Tooling layer).

Extracts repeated error occurrences from plain-text logs or JSONL files using either
built-in generic rules or a caller-supplied JSON ruleset (`--rules <rules.json>`), groups
them by normalized signature (stripping timestamps, hex addresses, line numbers, UUIDs),
filters by `--min-occurrences`, emits canonical `error-fix` JSONL atoms, and optionally
drives `2brain ingest --format error-fix` (dry-run by default; pass `--apply` to write).
"""
from __future__ import annotations

import argparse
from collections import OrderedDict
from collections.abc import Iterable
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile

_DEFAULT_ROOT_CAUSE = "Pending diagnosis from recurring logs"
_DEFAULT_FIX = "Pending verified fix"
_ERROR_LEVELS = frozenset({"error", "err", "fatal", "critical", "crit", "panic", "severe"})

DEFAULT_RULES: list[dict[str, object]] = [
    {
        "id": "generic-http-or-service-error",
        "pattern": r"(?:^|\s)(?:ERROR|FATAL|CRITICAL)\s+(?:\[(?P<component>[^\]]+)\]\s*)?(?P<message>.+)",
        "component": "service",
        "symptom": "Service error log entry",
        "tags": ["ops", "error"],
    },
    {
        "id": "rust-compiler-error",
        "pattern": r"^(?:\d{4}-\d{2}-\d{2}[T ]\S+\s+)?error(?:\[(?P<code>E\d{4})\])?:\s+(?P<message>.+)$",
        "component": "rustc",
        "symptom": "Rust compiler failure",
        "tags": ["rust", "build"],
    },
    {
        "id": "python-traceback-error",
        "pattern": r"^(?P<code>[A-Z][A-Za-z0-9_]*(?:Error|Exception)):\s+(?P<message>.+)$",
        "component": "python",
        "symptom": "Python runtime exception",
        "tags": ["python", "runtime"],
    },
]

_NORMALIZE_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b", re.I), "<uuid>"),
    (re.compile(r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\b"), "<ts>"),
    (re.compile(r"\b0x[0-9a-fA-F]+\b"), "<hex>"),
    (re.compile(r"(?<=[:#])\d+\b"), "<num>"),
    (re.compile(r"\b\d{4,}\b"), "<num>"),
    (re.compile(r"\s+"), " "),
]


def normalize_signature(raw: str) -> str:
    text = raw.strip()
    for rx, repl in _NORMALIZE_PATTERNS:
        text = rx.sub(repl, text)
    return text.strip()[:240]


def _normalize_tags(raw: object, default: list[str]) -> list[str]:
    if isinstance(raw, list):
        cleaned = [str(t).strip() for t in raw if isinstance(t, str) and t.strip()]
        if cleaned:
            return cleaned
    return list(default)


def _positive_occurrences(raw: object) -> int:
    if isinstance(raw, bool):
        return 1
    if isinstance(raw, int) and raw >= 1:
        return raw
    return 1


def _open_regular_read_nofollow(path: Path):
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(str(path), flags)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            raise OSError(f"input path is not a regular file: {path}")
    except Exception:
        os.close(fd)
        raise
    return os.fdopen(fd, "r", encoding="utf-8", errors="replace")


def _write_regular_nofollow(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(str(path), flags, 0o600)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            raise OSError(f"output path is not a regular file: {path}")
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(content)
            fh.flush()
            os.fsync(fh.fileno())
    except Exception:
        try:
            os.close(fd)
        except OSError:
            pass
        raise


def load_rules(rules_path: Path | None) -> list[dict[str, object]]:
    if rules_path is None:
        return DEFAULT_RULES
    with _open_regular_read_nofollow(rules_path) as fh:
        raw = json.load(fh)
    if not isinstance(raw, list) or not raw:
        raise ValueError("rules file must contain a non-empty JSON array of rule objects")
    validated: list[dict[str, object]] = []
    for idx, rule in enumerate(raw):
        if not isinstance(rule, dict) or not isinstance(rule.get("pattern"), str):
            raise ValueError(f"rule[{idx}] must be an object with string 'pattern'")
        re.compile(rule["pattern"])
        validated.append(rule)
    return validated


def _is_jsonl_error_record(obj: dict[str, object]) -> bool:
    if isinstance(obj.get("error_signature"), str) and obj["error_signature"].strip():
        return True
    if isinstance(obj.get("error"), str) and obj["error"].strip():
        return True
    raw_level = obj.get("level") or obj.get("severity")
    if isinstance(raw_level, str) and raw_level.strip().lower() in _ERROR_LEVELS:
        return isinstance(obj.get("message"), str) and bool(obj["message"].strip())
    return False


def extract_atoms_from_lines(
    lines: Iterable[str],
    rules: list[dict[str, object]],
    *,
    min_occurrences: int = 2,
    extra_fields: dict[str, object] | None = None,
) -> list[dict[str, object]]:
    compiled = [(r, re.compile(str(r["pattern"]))) for r in rules]
    buckets: OrderedDict[str, dict[str, object]] = OrderedDict()

    for raw_line in lines:
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("{") and line.endswith("}"):
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                obj = None
            if isinstance(obj, dict):
                if not _is_jsonl_error_record(obj):
                    continue
                sig_raw = obj.get("error_signature") or obj.get("error") or obj.get("message")
                if isinstance(sig_raw, str) and sig_raw.strip():
                    sig = normalize_signature(sig_raw)
                    occ = _positive_occurrences(obj.get("occurrences", 1))
                    row_root_cause = (
                        str(obj["root_cause"]).strip()
                        if isinstance(obj.get("root_cause"), str) and obj["root_cause"].strip()
                        else ""
                    )
                    row_fix = (
                        str(obj["fix"]).strip()
                        if isinstance(obj.get("fix"), str) and obj["fix"].strip()
                        else ""
                    )
                    row_ver = (
                        str(obj["verification"]).strip()
                        if isinstance(obj.get("verification"), str) and obj["verification"].strip()
                        else ""
                    )
                    entry = buckets.get(sig)
                    if entry is None:
                        entry = {
                            "error_signature": sig,
                            "title": str(obj.get("title") or sig[:96]),
                            "symptom": str(obj.get("symptom") or sig_raw.strip()[:200]),
                            "root_cause": row_root_cause or _DEFAULT_ROOT_CAUSE,
                            "fix": row_fix or _DEFAULT_FIX,
                            "component": str(obj.get("component") or "runtime"),
                            "occurrences": occ,
                            "tags": _normalize_tags(obj.get("tags"), ["error-fix"]),
                        }
                        if row_ver:
                            entry["verification"] = row_ver
                        buckets[sig] = entry
                    else:
                        entry["occurrences"] = int(entry["occurrences"]) + occ
                        if entry["root_cause"] == _DEFAULT_ROOT_CAUSE and row_root_cause:
                            entry["root_cause"] = row_root_cause
                        if entry["fix"] == _DEFAULT_FIX and row_fix:
                            entry["fix"] = row_fix
                        if "verification" not in entry and row_ver:
                            entry["verification"] = row_ver
                    continue

        for rule, rx in compiled:
            m = rx.search(line)
            if not m:
                continue
            gd = m.groupdict()
            code = (gd.get("code") or "").strip()
            msg = (gd.get("message") or m.group(0)).strip()
            sig_raw = f"{code}: {msg}" if code else msg
            sig = normalize_signature(sig_raw)
            component = (gd.get("component") or str(rule.get("component", "service"))).strip()
            entry = buckets.get(sig)
            if entry is None:
                entry = {
                    "error_signature": sig,
                    "title": f"{component}: {sig[:80]}",
                    "symptom": str(rule.get("symptom") or msg[:200]),
                    "root_cause": str(rule.get("root_cause") or "Recurring failure matched from logs"),
                    "fix": str(rule.get("fix") or "Document root cause and verified mitigation"),
                    "component": component,
                    "occurrences": 1,
                    "tags": _normalize_tags(rule.get("tags"), ["error-fix"]),
                }
                buckets[sig] = entry
            else:
                entry["occurrences"] = int(entry["occurrences"]) + 1
            break

    out: list[dict[str, object]] = []
    for rec in buckets.values():
        if int(rec["occurrences"]) < min_occurrences:
            continue
        if extra_fields:
            for k, v in extra_fields.items():
                if k == "tags" and isinstance(v, list):
                    merged = list(rec.get("tags", []))
                    for t in v:
                        if isinstance(t, str) and t.strip() and t.strip() not in merged:
                            merged.append(t.strip())
                    rec["tags"] = merged
                else:
                    rec.setdefault(k, v)
        out.append(rec)
    return out


def run_selftest() -> int:
    sample_lines = [
        "2026-11-28T10:00:01Z ERROR [tb-ipc] tls error: handshake failed on fd 10234",
        "2026-11-28T10:05:19Z ERROR [tb-ipc] tls error: handshake failed on fd 59812",
        "2026-11-28T10:06:00Z ERROR [tb-ipc] one-off transient timeout 0xdeadbeef",
        '{"level": "INFO", "message": "healthcheck ok"}',
        '{"level": "INFO", "message": "healthcheck ok"}',
        '{"error_signature": "E0308: mismatched types u64 vs i32", "component": "tb-journal", "occurrences": false}',
        '{"error_signature": "E0308: mismatched types u64 vs i32", "component": "tb-journal", "fix": "Use rustix::fs::fstat vs statat", "occurrences": 1}',
    ]
    atoms = extract_atoms_from_lines(sample_lines, DEFAULT_RULES, min_occurrences=2)
    assert len(atoms) == 2, f"expected 2 recurring atoms (INFO ignored), got {len(atoms)}: {atoms}"
    assert atoms[0]["component"] == "tb-ipc", f"service ERROR misclassified: {atoms[0]}"
    assert atoms[0]["occurrences"] == 2
    assert "<num>" in str(atoms[0]["error_signature"])
    assert atoms[1]["component"] == "tb-journal"
    assert atoms[1]["occurrences"] == 2
    assert atoms[1]["fix"] == "Use rustix::fs::fstat vs statat"
    print("extract_error_fixes selftest ok: 2 recurring atoms extracted, INFO & 1x filtered, service ERROR preserved")
    return 0


def _iter_input_lines(paths: list[Path]) -> Iterable[str]:
    for p in paths:
        with _open_regular_read_nofollow(p) as fh:
            for line in fh:
                yield line


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Extract recurring error->fix atoms from logs and drive `2brain ingest --format error-fix`"
    )
    parser.add_argument("--selftest", action="store_true", help="Run built-in unit self-test and exit")
    parser.add_argument("--input", type=Path, action="append", default=[], help="Log or JSONL input file (repeatable)")
    parser.add_argument("--rules", type=Path, default=None, help="Optional JSON rules file overriding built-in rules")
    parser.add_argument("--min-occurrences", type=int, default=2)
    parser.add_argument("--output-jsonl", type=Path, default=None, help="Optional path to write extracted JSONL atoms")
    parser.add_argument("--extra-fields-json", default=None, help="Optional JSON object of workspace schema extra fields (e.g. topics/provenance)")
    parser.add_argument("--workspace", type=Path, default=None, help="Target 2brain workspace for ingest")
    parser.add_argument("--bin", default="2brain", help="Path to 2brain binary")
    parser.add_argument("--type-name", default="error-fix", help="Target note type passed to `2brain ingest --type`")
    parser.add_argument("--target-dir", default="inbox/errors", help="Destination folder passed to `2brain ingest --dest`")
    parser.add_argument("--repo", default=None)
    parser.add_argument("--apply", action="store_true", help="Actually write notes (default is dry-run)")
    args = parser.parse_args()

    if args.selftest:
        return run_selftest()
    if not args.input:
        print("error: provide at least one --input file (or --selftest)", file=sys.stderr)
        return 2
    if args.min_occurrences < 1:
        print("error: --min-occurrences must be >= 1", file=sys.stderr)
        return 2

    try:
        rules = load_rules(args.rules)
        extra_fields: dict[str, object] | None = None
        if args.extra_fields_json:
            parsed_extra = json.loads(args.extra_fields_json)
            if not isinstance(parsed_extra, dict):
                raise ValueError("--extra-fields-json must decode to a JSON object")
            extra_fields = parsed_extra
        atoms = extract_atoms_from_lines(
            _iter_input_lines(args.input),
            rules,
            min_occurrences=args.min_occurrences,
            extra_fields=extra_fields,
        )
    except (OSError, ValueError, json.JSONDecodeError, re.error) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    jsonl_body = "\n".join(json.dumps(a, ensure_ascii=False) for a in atoms) + ("\n" if atoms else "")
    if args.output_jsonl is not None:
        try:
            _write_regular_nofollow(args.output_jsonl, jsonl_body)
        except OSError as exc:
            print(f"error: failed to write {args.output_jsonl}: {exc}", file=sys.stderr)
            return 1

    print(f"[error-fix] extracted {len(atoms)} recurring atom(s) from {len(args.input)} input(s)")
    if args.workspace is None or not atoms:
        return 0

    tmp_dir_ctx = None
    try:
        if args.output_jsonl is not None:
            feed_path = args.output_jsonl
        else:
            tmp_dir_ctx = tempfile.TemporaryDirectory(prefix="2brain-error-fix-")
            feed_path = Path(tmp_dir_ctx.name) / "recurring-errors.jsonl"
            _write_regular_nofollow(feed_path, jsonl_body)

        cmd = [
            args.bin,
            "ingest",
            str(args.workspace),
            str(feed_path),
            "--format",
            "error-fix",
            "--type",
            args.type_name,
            "--dest",
            args.target_dir,
            "--json",
        ]
        if args.repo:
            cmd.extend(["--repo", args.repo])
        if not args.apply:
            cmd.append("--dry-run")
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0:
            print(proc.stderr.strip() or proc.stdout.strip(), file=sys.stderr)
            return proc.returncode
        report = json.loads(proc.stdout)
        admitted = report.get("admitted", len(report.get("reports", [])))
        rejected = report.get("rejected", 0)
        written = report.get("written", 0)
        print(
            f"[error-fix] ingest dry_run={not args.apply} admitted={admitted} "
            f"rejected={rejected} written={written}"
        )
        return 0 if report.get("ok", rejected == 0) else 1
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    finally:
        if tmp_dir_ctx is not None:
            tmp_dir_ctx.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
