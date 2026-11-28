#!/usr/bin/env python3
"""Independent Black-Box Semantic Capture Harness (Issue #7 U2/R22/AE22, MIT).

STRICT ORACLE ISOLATION RULE:
This module knows ONLY fixture inputs and public `2brain` CLI / MCP JSON-RPC calls.
It contains ZERO expected assertions, ZERO gold verdicts, and ZERO pass/fail logic.
It materializes disposable workspaces, drives the compiled `2brain` binary as a
black box, records exact ordered tool/CLI exchanges and post-action filesystem
digests, and freezes them with a SHA-256 `bundle_digest` + versioned run manifest
(`2brain.semantic.run-manifest/v1`) for offline evaluation by `scorer.py`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

MANIFEST_SCHEMA = "2brain.semantic.run-manifest/v1"
OBSERVATION_SCHEMA = "2brain.semantic.observations/v1"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def canonical_json_bytes(obj: Any) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


class McpStdioSession:
    """Minimal line-delimited JSON-RPC 2.0 MCP client over stdio."""

    def __init__(self, binary: Path, extra_args: list[str], env: dict[str, str] | None = None):
        cmd = [str(binary), "mcp", *extra_args]
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        self._cmd = cmd
        self._proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=merged_env,
        )
        self._next_id = 1
        self.exchanges: list[dict[str, Any]] = []
        self._initialize()

    def _send_raw(self, msg: dict[str, Any]) -> None:
        assert self._proc.stdin is not None
        self._proc.stdin.write(json.dumps(msg, ensure_ascii=False) + "\n")
        self._proc.stdin.flush()

    def _read_response(self, req_id: int) -> dict[str, Any]:
        assert self._proc.stdout is not None
        while True:
            line = self._proc.stdout.readline()
            if not line:
                stderr = self._proc.stderr.read() if self._proc.stderr else ""
                raise RuntimeError(f"MCP server closed stdout prematurely (cmd={self._cmd}): {stderr}")
            line = line.strip()
            if not line:
                continue
            parsed = json.loads(line)
            if parsed.get("id") == req_id:
                return parsed

    def _initialize(self) -> None:
        rid = self._next_id
        self._next_id += 1
        self._send_raw(
            {
                "jsonrpc": "2.0",
                "id": rid,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2026-07-28",
                    "capabilities": {},
                    "clientInfo": {"name": "2brain-semantic-capture", "version": "1.0"},
                },
            }
        )
        self._read_response(rid)
        self._send_raw({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call_tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        rid = self._next_id
        self._next_id += 1
        t0 = time.monotonic()
        req = {
            "jsonrpc": "2.0",
            "id": rid,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }
        self._send_raw(req)
        resp = self._read_response(rid)
        elapsed_ms = round((time.monotonic() - t0) * 1000.0, 2)
        result = resp.get("result", {})
        rec = {
            "kind": "mcp_tool",
            "tool": name,
            "arguments": arguments,
            "is_error": bool(result.get("isError", False)) or ("error" in resp),
            "structured": result.get("structuredContent"),
            "text": [c.get("text") for c in result.get("content", []) if isinstance(c, dict)],
            "rpc_error": resp.get("error"),
            "elapsed_ms": elapsed_ms,
        }
        self.exchanges.append(rec)
        return rec

    def close(self) -> None:
        if self._proc.stdin:
            try:
                self._proc.stdin.close()
            except OSError:
                pass
        try:
            self._proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            self._proc.wait(timeout=2)


def run_cli(binary: Path, args: list[str]) -> dict[str, Any]:
    t0 = time.monotonic()
    proc = subprocess.run(
        [str(binary), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
    )
    elapsed_ms = round((time.monotonic() - t0) * 1000.0, 2)
    return {
        "kind": "cli",
        "args": args,
        "exit_code": proc.returncode,
        "stdout": proc.stdout,
        "stderr": proc.stderr,
        "elapsed_ms": elapsed_ms,
    }


def init_git_repo(ws: Path) -> None:
    subprocess.run(["git", "init", "-q"], cwd=ws, check=True)
    subprocess.run(["git", "config", "user.email", "capture@2brain.local"], cwd=ws, check=True)
    subprocess.run(["git", "config", "user.name", "Capture"], cwd=ws, check=True)
    subprocess.run(["git", "add", "-A"], cwd=ws, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "seed"], cwd=ws, check=True)


def capture_sem01_unicode_receipt(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-sem01-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        seed = "---\ndescription: Unicode seed note\n---\nПривет 🌍 café — line two.\n"
        target = ws / "notes" / "unicode.md"
        target.write_text(seed, encoding="utf-8")
        raw_bytes = target.read_bytes()
        sess = McpStdioSession(binary, ["--in-process", str(ws)])
        try:
            full = sess.call_tool("tb_content", {"repo": "vault", "path": "notes/unicode.md"})
            window = sess.call_tool(
                "tb_content",
                {"repo": "vault", "path": "notes/unicode.md", "offset_chars": 0, "max_chars": 45},
            )
        finally:
            sess.close()
        return {
            "case_id": "SEM-01",
            "family": "evidence_identity",
            "seed_meta": {
                "path": "notes/unicode.md",
                "disk_sha256": sha256_bytes(raw_bytes),
                "disk_bytes": len(raw_bytes),
                "disk_chars": len(seed),
            },
            "observations": [full, window],
        }


def capture_sem02_write_guard(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-sem02-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        body = "---\ndescription: Guarded policy\n---\n" + ("Alpha line.\n" * 20)
        target = ws / "notes" / "policy.md"
        target.write_text(body, encoding="utf-8")
        init_git_repo(ws)

        sess = McpStdioSession(binary, ["--in-process", str(ws), "--allow-mutations"])
        try:
            # Step 1: truncated page only
            p1 = sess.call_tool(
                "tb_content",
                {"repo": "vault", "path": "notes/policy.md", "offset_chars": 0, "max_chars": 40},
            )
            short_hash = (p1.get("structured") or {}).get("content", {}).get("hash", "")
            # Step 2: attempt write after truncated read
            w_trunc = sess.call_tool(
                "tb_write_file",
                {
                    "repo": "vault",
                    "path": "notes/policy.md",
                    "expected_hash": short_hash,
                    "content": "---\ndescription: Mutated\n---\nTruncated attempt.\n",
                },
            )
            # Step 3: tail read without page 0
            sess.call_tool(
                "tb_content",
                {"repo": "vault", "path": "notes/policy.md", "offset_chars": 40, "max_chars": 500},
            )
            w_eof = sess.call_tool(
                "tb_write_file",
                {
                    "repo": "vault",
                    "path": "notes/policy.md",
                    "expected_hash": short_hash,
                    "content": "---\ndescription: Mutated\n---\nTail attempt.\n",
                },
            )
            # Step 4: full whole-file read -> emits stateless read_receipt inside content
            p_full = sess.call_tool("tb_content", {"repo": "vault", "path": "notes/policy.md"})
            receipt = ((p_full.get("structured") or {}).get("content") or {}).get("read_receipt")
            # Step 4b: tampered read_receipt must fail closed even after full read
            w_tampered_receipt = sess.call_tool(
                "tb_write_file",
                {
                    "repo": "vault",
                    "path": "notes/policy.md",
                    "expected_hash": short_hash,
                    "read_receipt": (receipt or "v1.bad") + "00",
                    "content": "---\ndescription: Bad receipt\n---\nMust fail.\n",
                },
            )
            # Step 5: valid read_receipt succeeds
            w_ok = sess.call_tool(
                "tb_write_file",
                {
                    "repo": "vault",
                    "path": "notes/policy.md",
                    "expected_hash": short_hash,
                    "read_receipt": receipt,
                    "content": "---\ndescription: Mutated cleanly\n---\nWhole read + valid receipt succeeded.\n",
                },
            )
        finally:
            sess.close()

        return {
            "case_id": "SEM-02",
            "family": "write_guard",
            "seed_meta": {
                "has_receipt": bool(receipt),
                "post_disk_text": target.read_text(encoding="utf-8"),
            },
            "observations": [p1, w_trunc, w_eof, p_full, w_tampered_receipt, w_ok],
        }


def capture_sem03_conditional_hydration(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-sem03-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        target = ws / "notes" / "rev.md"
        target.write_text("---\ndescription: Revision one\n---\nVersion 1 body.\n", encoding="utf-8")
        init_git_repo(ws)

        sess = McpStdioSession(binary, ["--in-process", str(ws)])
        try:
            r1 = sess.call_tool("tb_content", {"repo": "vault", "path": "notes/rev.md"})
            sha1 = (r1.get("structured") or {}).get("content", {}).get("sha256", "")
            # Dirty edit WITHOUT changing Git HEAD!
            target.write_text("---\ndescription: Revision two\n---\nVersion 2 dirty edit.\n", encoding="utf-8")
            sha2_disk = sha256_file(target)
            r_stale = sess.call_tool(
                "tb_content",
                {
                    "repo": "vault",
                    "path": "notes/rev.md",
                    "expected": {"sha256": sha1},
                },
            )
            r_match = sess.call_tool(
                "tb_content",
                {
                    "repo": "vault",
                    "path": "notes/rev.md",
                    "expected": {"sha256": sha2_disk},
                },
            )
        finally:
            sess.close()

        return {
            "case_id": "SEM-03",
            "family": "revision_continuation",
            "seed_meta": {"sha1": sha1, "sha2_disk": sha2_disk},
            "observations": [r1, r_stale, r_match],
        }


def capture_sem04_invalid_utf8(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-sem04-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "good.md").write_text("---\ndescription: Good note\n---\nValid.\n", encoding="utf-8")
        (ws / "notes" / "bad.md").write_bytes(b"---\ndescription: Bad\n---\n\xff\xfe\xfd invalid utf8\n")
        sess = McpStdioSession(binary, ["--in-process", str(ws)])
        try:
            bad = sess.call_tool("tb_content", {"repo": "vault", "path": "notes/bad.md"})
        finally:
            sess.close()
        return {
            "case_id": "SEM-04",
            "family": "decode_fault",
            "seed_meta": {"path": "notes/bad.md"},
            "observations": [bad],
        }


def capture_sem09_bounded_search(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-sem09-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "a.md").write_text(
            "---\ndescription: Rate limit exception\n---\nDefault rate limit is 100 rps. Emergency override raises limit.\n",
            encoding="utf-8",
        )
        (ws / "notes" / "b.md").write_text(
            "---\ndescription: Unrelated rate note\n---\nExchange rate for EUR is updated daily.\n",
            encoding="utf-8",
        )
        (ws / "notes" / "c.md").write_text(
            "---\ndescription: Unrelated limit note\n---\nMemory limit for daemon is 64 MiB.\n",
            encoding="utf-8",
        )
        sess = McpStdioSession(binary, ["--in-process", str(ws)])
        try:
            q_and = sess.call_tool("tb_search", {"query": "rate limit", "limit": 10})
            q_exact = sess.call_tool("tb_search", {"query": "emergency override", "exact": True, "limit": 10})
            q_bounded = sess.call_tool("tb_search", {"query": "limit", "limit": 1})
        finally:
            sess.close()
        return {
            "case_id": "SEM-09",
            "family": "bounded_retrieval",
            "seed_meta": {},
            "observations": [q_and, q_exact, q_bounded],
        }


def capture_sem18_scope_and_symlink(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-sem18-") as td:
        outside = Path(td) / "outside_secret.md"
        outside.write_text("TOP SECRET EXTERNAL DATA", encoding="utf-8")
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "ok.md").write_text("---\ndescription: In scope\n---\nSafe content.\n", encoding="utf-8")
        os.symlink(outside, ws / "notes" / "escape.md")
        sess = McpStdioSession(binary, ["--in-process", str(ws)])
        try:
            esc = sess.call_tool("tb_content", {"repo": "vault", "path": "notes/escape.md"})
            ok = sess.call_tool("tb_content", {"repo": "vault", "path": "notes/ok.md"})
        finally:
            sess.close()
        return {
            "case_id": "SEM-18",
            "family": "scope_confinement",
            "seed_meta": {"outside_secret": "TOP SECRET EXTERNAL DATA"},
            "observations": [esc, ok],
        }


def capture_ae04_ae08_context_conflicts(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-ae04-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "rule.md").write_text(
            "---\n"
            "description: Primary deployment freeze rule\n"
            "claim_key: deploy-freeze\n"
            "polarity: supports\n"
            "conflicts_with:\n"
            "  - notes/exception.md\n"
            "---\n"
            "# Rule\n\nProduction deploys are frozen on Fridays.\n",
            encoding="utf-8",
        )
        (ws / "notes" / "exception.md").write_text(
            "---\n"
            "description: Security hotfix carve-out\n"
            "claim_key: deploy-freeze\n"
            "polarity: contradicts\n"
            "---\n"
            "# Exception\n\nSeverity-1 security hotfixes bypass the Friday freeze.\n",
            encoding="utf-8",
        )
        sess = McpStdioSession(binary, ["--in-process", str(ws)])
        try:
            ctx = sess.call_tool("tb_context", {"query": "Friday deploys frozen", "max_notes": 4})
        finally:
            sess.close()
        return {
            "case_id": "AE04-08",
            "family": "rule_exception_conflict",
            "seed_meta": {},
            "observations": [ctx],
        }


def capture_ae24_revocation_egress_audit(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-ae24-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "secret.md").write_text("---\ndescription: Revoked note\n---\nClassified.\n", encoding="utf-8")

        audit_path = Path(td) / "audit.jsonl"
        policy_revoked = Path(td) / "policy_revoked.json"
        policy_revoked.write_text(
            json.dumps(
                {
                    "revoked_repos": ["vault"],
                    "egress_destination": "local",
                    "allow_external_egress": False,
                    "audit_log_path": str(audit_path),
                }
            ),
            encoding="utf-8",
        )
        sess1 = McpStdioSession(
            binary,
            ["--in-process", str(ws), "--policy", str(policy_revoked), "--principal", "semantic-agent"],
        )
        try:
            rev_read = sess1.call_tool("tb_content", {"repo": "vault", "path": "notes/secret.md"})
            rev_search = sess1.call_tool("tb_search", {"query": "Classified"})
        finally:
            sess1.close()

        policy_ext = Path(td) / "policy_ext.json"
        policy_ext.write_text(
            json.dumps(
                {
                    "revoked_repos": [],
                    "egress_destination": "external-cloud-llm",
                    "allow_external_egress": False,
                    "audit_log_path": str(audit_path),
                }
            ),
            encoding="utf-8",
        )
        sess2 = McpStdioSession(
            binary,
            ["--in-process", str(ws), "--policy", str(policy_ext), "--principal", "semantic-agent"],
        )
        try:
            ext_denied = sess2.call_tool("tb_overview", {})
        finally:
            sess2.close()

        audit_lines = [
            json.loads(line)
            for line in audit_path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        return {
            "case_id": "AE24",
            "family": "privacy_revocation_egress",
            "seed_meta": {"audit_events": audit_lines},
            "observations": [rev_read, rev_search, ext_denied],
        }


def capture_ae25_backup_restore(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-ae25-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "one.md").write_text("---\ndescription: One\n---\nBackup content.\n", encoding="utf-8")
        bak = Path(td) / "bak"
        restored = Path(td) / "restored"
        tampered_target = Path(td) / "tampered_target"

        c_bak = run_cli(binary, ["backup", str(ws), "--out", str(bak), "--json"])
        c_res = run_cli(binary, ["restore", str(bak), "--to", str(restored), "--json"])
        # Tamper with backup payload and verify restore fails closed
        (bak / "data" / "notes" / "one.md").write_text("CORRUPTED", encoding="utf-8")
        c_tamper = run_cli(binary, ["restore", str(bak), "--to", str(tampered_target), "--json"])

        return {
            "case_id": "AE25",
            "family": "backup_restore_ops",
            "seed_meta": {
                "restored_matches": (restored / "notes" / "one.md").exists()
                and (restored / "notes" / "one.md").read_text(encoding="utf-8")
                == (ws / "notes" / "one.md").read_text(encoding="utf-8"),
                "tampered_target_created": tampered_target.exists(),
            },
            "observations": [c_bak, c_res, c_tamper],
        }


def capture_adr0009_durable_outcomes(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-adr0009-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "seed.md").write_text("---\ndescription: Seed\n---\nSeed.\n", encoding="utf-8")
        init_git_repo(ws)

        c_enable = run_cli(binary, ["outcomes", "enable", str(ws)])
        req_id = "0123456789abcdef0123456789abcdef"

        # Process 1: commit mutation with request_id
        sess1 = McpStdioSession(
            binary,
            ["--in-process", str(ws), "--allow-mutations", "--outcome-registry"],
        )
        try:
            m1 = sess1.call_tool(
                "tb_create_file",
                {
                    "repo": "vault",
                    "path": "notes/durable.md",
                    "content": "---\ndescription: Durable note\n---\nCreated once.\n",
                    "request_id": req_id,
                },
            )
        finally:
            sess1.close()

        # Process 2 (after restart!): same request_id + same payload => replay; different payload => conflict
        sess2 = McpStdioSession(
            binary,
            ["--in-process", str(ws), "--allow-mutations", "--outcome-registry"],
        )
        try:
            m_replay = sess2.call_tool(
                "tb_create_file",
                {
                    "repo": "vault",
                    "path": "notes/durable.md",
                    "content": "---\ndescription: Durable note\n---\nCreated once.\n",
                    "request_id": req_id,
                },
            )
            m_conflict = sess2.call_tool(
                "tb_create_file",
                {
                    "repo": "vault",
                    "path": "notes/durable.md",
                    "content": "---\ndescription: DIFFERENT PAYLOAD\n---\nMust conflict.\n",
                    "request_id": req_id,
                },
            )
        finally:
            sess2.close()

        c_status = run_cli(binary, ["outcomes", "status", str(ws), "--json"])
        c_compact = run_cli(binary, ["outcomes", "compact", str(ws), "--max-age-secs", "0"])

        return {
            "case_id": "ADR-0009",
            "family": "durable_outcomes",
            "seed_meta": {"request_id": req_id},
            "observations": [c_enable, m1, m_replay, m_conflict, c_status, c_compact],
        }


def capture_ae15_granted_procedures(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-ae15-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "seed.md").write_text("---\ndescription: Seed\n---\nSeed.\n", encoding="utf-8")
        init_git_repo(ws)

        bad_post_spec = {
            "schema_version": "2brain.procedure/v1",
            "id": "proc-wrong-artifact",
            "title": "Commit note missing expected artifact",
            "required_grants": ["mutations", "procedures"],
            "dry_run": False,
            "steps": [
                {
                    "op": "create_file",
                    "repo": "vault",
                    "path": "notes/proc_out.md",
                    "content": "---\ndescription: Produced note\n---\nActual body without token.\n",
                }
            ],
            "postconditions": [
                {
                    "kind": "contains_text",
                    "repo": "vault",
                    "path": "notes/proc_out.md",
                    "needle": "REQUIRED_PROOF_TOKEN",
                }
            ],
        }
        good_spec = {
            "schema_version": "2brain.procedure/v1",
            "id": "proc-verified",
            "title": "Commit note matching all postconditions",
            "required_grants": ["mutations", "procedures"],
            "dry_run": False,
            "steps": [
                {
                    "op": "create_file",
                    "repo": "vault",
                    "path": "notes/proc_good.md",
                    "content": "---\ndescription: Verified note\n---\nIncludes REQUIRED_PROOF_TOKEN.\n",
                }
            ],
            "postconditions": [
                {"kind": "file_exists", "repo": "vault", "path": "notes/proc_good.md"},
                {
                    "kind": "contains_text",
                    "repo": "vault",
                    "path": "notes/proc_good.md",
                    "needle": "REQUIRED_PROOF_TOKEN",
                },
            ],
        }

        # Session WITHOUT --allow-mutations: non-dry-run refused fail-closed!
        sess_ro = McpStdioSession(binary, ["--in-process", str(ws), "--allow-workflows"])
        try:
            p_denied = sess_ro.call_tool("tb_run_procedure", {"spec": bad_post_spec})
        finally:
            sess_ro.close()

        # Session WITH --allow-workflows --allow-mutations:
        # 1) dry_run preview MUST create zero files even when full write grants are held!
        # 2) wrong-artifact commit MUST report operation_outcome=committed & postcondition_outcome=failed.
        # 3) matching postconditions MUST report verified_success=true.
        sess_rw = McpStdioSession(binary, ["--in-process", str(ws), "--allow-workflows", "--allow-mutations"])
        try:
            p_preview = sess_rw.call_tool("tb_run_procedure", {"spec": bad_post_spec, "dry_run": True})
            preview_created_file = (ws / "notes" / "proc_out.md").exists()
            p_failed_post = sess_rw.call_tool("tb_run_procedure", {"spec": bad_post_spec})
            p_verified = sess_rw.call_tool("tb_run_procedure", {"spec": good_spec})
        finally:
            sess_rw.close()

        return {
            "case_id": "AE15",
            "family": "granted_procedures",
            "seed_meta": {"preview_created_file": preview_created_file},
            "observations": [p_preview, p_denied, p_failed_post, p_verified],
        }


def capture_ae17_task_handoff(binary: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="tb-ae17-") as td:
        ws = Path(td) / "vault"
        (ws / "notes").mkdir(parents=True)
        (ws / "notes" / "seed.md").write_text("---\ndescription: Seed\n---\nSeed.\n", encoding="utf-8")

        # Client 1 creates TaskPacket with critical publish=false
        sess1 = McpStdioSession(
            binary,
            ["--in-process", str(ws), "--allow-workflows", "--principal", "client-alpha"],
        )
        try:
            pkt1 = {
                "schema_version": "2brain.task-packet/v1",
                "task_id": "handoff-42",
                "revision": 0,
                "title": "Audit release without publishing",
                "status": "open",
                "criterion_revision": "2brain.criterion/v1",
                "scope_repos": ["vault"],
                "obligations": [{"key": "publish", "value": "false", "critical": True}],
                "budget": {"max_actions": 8, "spent_actions": 1, "max_tokens": 5000, "spent_tokens": 400},
                "completed_steps": ["read_notes"],
                "unchecked_steps": ["verify_backup"],
                "rejected_routes": ["fp-exhausted-1"],
                "pending_operations": [],
                "evidence_digests": {},
                "updated_by": "client-alpha",
                "updated_unix": 0,
            }
            c_create = sess1.call_tool("tb_task_checkpoint", {"expected_revision": 0, "packet": pkt1})
        finally:
            sess1.close()

        # Client 2 (fresh process!) resumes, advances rev 1->2, then Client 1 stale rev=1 is rejected!
        sess2 = McpStdioSession(
            binary,
            ["--in-process", str(ws), "--allow-workflows", "--principal", "client-beta"],
        )
        try:
            c_resume = sess2.call_tool("tb_task_resume", {"task_id": "handoff-42"})
            resumed = ((c_resume.get("structured") or {}).get("task_packet") or {}).copy()
            # Advance cleanly
            next_pkt = json.loads(json.dumps(resumed))
            next_pkt["budget"]["spent_actions"] = 2
            next_pkt["budget"]["spent_tokens"] = 850
            next_pkt["updated_by"] = "client-beta"
            c_advance = sess2.call_tool(
                "tb_task_checkpoint",
                {"expected_revision": 1, "packet": next_pkt},
            )
            # Stale writer using rev=1 MUST fail
            stale_pkt = json.loads(json.dumps(resumed))
            stale_pkt["title"] = "Stale overwrite"
            c_stale = sess2.call_tool(
                "tb_task_checkpoint",
                {"expected_revision": 1, "packet": stale_pkt},
            )
            # Dropping critical publish=false at rev=2 MUST fail
            drop_pkt = json.loads(json.dumps(next_pkt))
            drop_pkt["obligations"] = []
            c_drop = sess2.call_tool(
                "tb_task_checkpoint",
                {"expected_revision": 2, "packet": drop_pkt},
            )
        finally:
            sess2.close()

        return {
            "case_id": "AE17",
            "family": "task_handoff_concurrency",
            "seed_meta": {},
            "observations": [c_create, c_resume, c_advance, c_stale, c_drop],
        }


CASES = [
    capture_sem01_unicode_receipt,
    capture_sem02_write_guard,
    capture_sem03_conditional_hydration,
    capture_sem04_invalid_utf8,
    capture_sem09_bounded_search,
    capture_sem18_scope_and_symlink,
    capture_ae04_ae08_context_conflicts,
    capture_ae15_granted_procedures,
    capture_ae17_task_handoff,
    capture_ae24_revocation_egress_audit,
    capture_ae25_backup_restore,
    capture_adr0009_durable_outcomes,
]


def run_capture(binary: Path, out_dir: Path) -> tuple[Path, Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    ver_proc = run_cli(binary, ["--version"])
    started_unix = int(time.time())
    captured_cases = []
    for fn in CASES:
        captured_cases.append(fn(binary))

    observations = {
        "schema_version": OBSERVATION_SCHEMA,
        "captured_unix": started_unix,
        "sut_binary": str(binary),
        "sut_version": ver_proc["stdout"].strip(),
        "cases": captured_cases,
    }
    obs_bytes = canonical_json_bytes(observations)
    bundle_digest = sha256_bytes(obs_bytes)

    obs_path = out_dir / "observations.json"
    obs_path.write_text(json.dumps(observations, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    manifest = {
        "schema_version": MANIFEST_SCHEMA,
        "created_unix": started_unix,
        "trust_boundary": "parent_black_box_capture_isolated_from_oracle",
        "sut_binary": str(binary),
        "sut_version": ver_proc["stdout"].strip(),
        "assigned_cases": [c["case_id"] for c in captured_cases],
        "observations_file": obs_path.name,
        "bundle_digest_sha256": bundle_digest,
    }
    manifest_path = out_dir / "run_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return obs_path, manifest_path


def main() -> int:
    parser = argparse.ArgumentParser(description="Capture black-box observations from 2brain")
    parser.add_argument("--bin", required=True, type=Path, help="Path to compiled 2brain binary")
    parser.add_argument("--out", required=True, type=Path, help="Output directory for frozen capture bundle")
    args = parser.parse_args()

    obs_path, manifest_path = run_capture(args.bin.resolve(), args.out.resolve())
    print(f"captured bundle: {obs_path} (manifest: {manifest_path})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
