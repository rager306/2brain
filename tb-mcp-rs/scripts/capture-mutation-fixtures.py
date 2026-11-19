#!/usr/bin/env python3
"""Re-capture one SUCCESS answer per mutating tool into tests/fixtures/mutations/.

`tests/schema_contract.rs::every_mutation_payload_fits_the_schema_its_tool_declares`
compares the `outputSchema` a tool declares against the payload a real engine sent,
because a client that validates `structuredContent` refuses an answer the schema does
not describe — after the mutation has already landed. Two drifts of that class were
found live (`commits` declared as an array while the engine sends `{repo: sha}`, and
`hash` declared a string while a delete answers null), so the corpus covers every
mutating tool, not three of them:

    tb_write_file tb_edit_file tb_delete_file tb_create_file tb_batch
    tb_assign_block_id tb_rename_block_id tb_rename tb_rename_type
    tb_inline tb_promote tb_add_dep tb_remove_dep
    tb_add_workspace_member tb_remove_workspace_member tb_set_workspace_member_role

The fixture is disposable and never the served vault: a tempdir HOME, a temp repo, one
`au daemon start` on it, and the adapter launched with --allow-mutations
--allow-composition --allow-workspace-shape. Every tool is put in a state where it
SUCCEEDS, which is the state the schema describes: a guarded mutation reads first
(`tb_content` for the content hash, `tb_type` for a type hash), and each capture owns its
own file, so one capture cannot change another's answer.

Normalization is the corpus's existing one: the fixture repo root becomes `$FIXTURE`, a
40-character lowercase commit sha becomes `$COMMIT`, keys are sorted and the file is
indented by one space. Two captures also avoid the engine's clock: `tb_assign_block_id`
runs on a block that already carries a `^id` (the idempotent success path, same answer
shape), and `tb_inline`'s host hash — the one answer field a freshly minted `^: b-<millis>`
id reaches — is frozen to `$HASH`. Everything else survives a re-capture unchanged, so a
second run over a fresh tempdir writes byte-identical files.

Run from `tb-mcp-rs/`:

    python3 scripts/capture-mutation-fixtures.py

`AU_ENGINE_BIN` / `AU_MCP_RS_BIN` select the binaries it drives; `--keep` leaves the
tempdir in place for inspection.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import socket as socket_module
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "lib"))
from mcp_stdio import StdioMcp, newest_socket  # noqa: E402
from target_dir import cargo_target_dir  # noqa: E402

CRATE = HERE.parent
# The release builds in the repository's shared target directory (`.cargo/config.toml`).
TARGET = cargo_target_dir()
ENGINE = os.environ.get("AU_ENGINE_BIN", str(TARGET / "release/au"))
ADAPTER = os.environ.get("AU_MCP_RS_BIN", str(TARGET / "release/2brain"))
REPO = "fixture"

# The fixture, one file per capture so the captures stay independent. The two types and
# the record slot are the minimum a promote/inline/rename_block_id needs to succeed.
FIXTURE = {
    ".2brain/repo.yaml": "type: tb.engine.repo::tb-engine\nname: fixture\n",
    "README.md": "---\ntype: tb.engine.readme::tb-engine\ntldr: a disposable fixture repo\n---\n\n"
                 "# Repo Overview\n\n## What this is\n\nA disposable fixture.\n\n"
                 "## How to use this\n\nCapture mutation payloads.\n\n"
                 "## How to extend this\n\nAdd a file and capture again.\n",
    "type/note.type.yaml": "fields:\n  title?: String\n",
    "type/canvas.type.yaml": "fields:\n  nodes: node&[]\n",
    "type/node.type.yaml": "fields:\n  content?: String\n",
    "notes/a.md": "---\ntype: note\ntitle: a\n---\n\nalpha\n",
    "notes/edit.md": "---\ntype: note\ntitle: edit\n---\n\nreplace me\n",
    "notes/delete.md": "---\ntype: note\ntitle: delete\n---\n\nobsolete\n",
    "notes/batch-write.md": "---\ntype: note\ntitle: batch write\n---\n\nbefore\n",
    "notes/batch-edit.md": "---\ntype: note\ntitle: batch edit\n---\n\nfirst\n",
    "notes/marker.md": "---\ntype: note\ntitle: marker\n---\n\nfind me: unique-marker-7f3a ^seedid\n",
    "notes/host.md": "---\ntype: canvas\nnodes:\n  - ^: rec1\n    type: node\n    content: x\n---\n\nhost\n",
    "notes/canvas.md": "---\ntype: canvas\nnodes:\n  - type: node\n    content: x\n---\n\ncanvas\n",
    "notes/rename.md": "---\ntype: note\ntitle: rename\n---\n\nold name\n",
    "fold.md": "---\ntype: node\ncontent: folded\n---\n",
    "into.md": "---\ntype: canvas\nnodes:\n  - \"[[fold]]\"\n---\n\nhost\n",
}


class CaptureError(RuntimeError):
    """A tool did not succeed, so its payload is not the corpus's answer."""


def build_fixture(root: Path) -> Path:
    repo = root / "fixture"
    for relative, text in FIXTURE.items():
        path = repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    (root / "home").mkdir()
    git = ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
           "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    subprocess.run(["git", "add", "-A"], cwd=repo, check=True)
    subprocess.run([*git, "commit", "-q", "-m", "fixture"], cwd=repo, check=True)
    return repo


def start_daemon(engine: str, repo: Path, home: Path) -> subprocess.Popen:
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home.parent / "config"))
    return subprocess.Popen(
        [engine, "daemon", "start", str(repo)], env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def wait_for_socket(home: Path, timeout: float = 120.0) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for candidate in [newest_socket(home)] if newest_socket(home) else []:
            # A socket file can exist before the daemon listens, so connect once.
            try:
                with socket_module.socket(socket_module.AF_UNIX) as probe:
                    probe.settimeout(1)
                    probe.connect(candidate)
                return candidate
            except OSError:
                pass
        time.sleep(0.1)
    raise CaptureError("the disposable engine never became ready")


def call(rpc: StdioMcp, tool: str, arguments: dict | None = None) -> dict:
    """One tool call whose answer is the payload; a refusal is an error, not a fixture."""
    reply = rpc.call(tool, arguments or {})
    if "error" in reply:
        raise CaptureError(f"{tool}: JSON-RPC error {json.dumps(reply['error'])[:800]}")
    result = reply.get("result") or {}
    if result.get("isError"):
        text = (result.get("content") or [{}])[0].get("text", "")
        raise CaptureError(f"{tool}: refused: {text[:800]}")
    if result.get("structuredContent") is not None:
        return result["structuredContent"]
    return json.loads(result["content"][0]["text"])


def content(rpc: StdioMcp, path: str) -> dict:
    """The tb_content read a guarded mutation needs: its text and its hash."""
    payload = call(rpc, "tb_content", {"repo": REPO, "path": path})
    entry = payload.get("content")
    if not isinstance(entry, dict) or not entry.get("hash"):
        raise CaptureError(f"tb_content returned no content for {path}: {payload}")
    return entry


def capture_write_file(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/a.md")
    return call(rpc, "tb_write_file", {
        "repo": REPO, "path": "notes/a.md",
        "content": "---\ntype: note\ntitle: a\n---\n\nalpha changed\n",
        "expected_hash": read["hash"]})


def capture_edit_file(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/edit.md")
    return call(rpc, "tb_edit_file", {
        "repo": REPO, "path": "notes/edit.md",
        "old_string": "replace me", "new_string": "replaced", "replace_all": False,
        "expected_hash": read["hash"]})


def capture_delete_file(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/delete.md")
    return call(rpc, "tb_delete_file", {
        "repo": REPO, "path": "notes/delete.md", "expected_hash": read["hash"]})


def capture_create_file(rpc: StdioMcp) -> dict:
    return call(rpc, "tb_create_file", {
        "repo": REPO, "path": "notes/created.md",
        "content": "---\ntype: note\ntitle: created\n---\n\nnew\n"})


def capture_batch(rpc: StdioMcp) -> dict:
    written = content(rpc, "notes/batch-write.md")
    edited = content(rpc, "notes/batch-edit.md")
    return call(rpc, "tb_batch", {"repo": REPO, "ops": [
        {"op": "write_file", "path": "notes/batch-write.md",
         "content": "---\ntype: note\ntitle: batch write\n---\n\nafter\n",
         "expected_hash": written["hash"]},
        {"op": "edit_file", "path": "notes/batch-edit.md",
         "old_string": "first", "new_string": "second", "replace_all": False,
         "expected_hash": edited["hash"]},
    ]})


def capture_assign_block_id(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/marker.md")
    # The offset lands inside the paragraph whose last token is `^seedid`, the
    # already-addressable case: the engine answers with that id and writes nothing.
    return call(rpc, "tb_assign_block_id", {
        "repo": REPO, "path": "notes/marker.md",
        "at": read["text"].index("unique-marker"), "expected_hash": read["hash"]})


def capture_rename_block_id(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/host.md")
    return call(rpc, "tb_rename_block_id", {
        "repo": REPO, "path": "notes/host.md",
        "block_id": "rec1", "to_block_id": "rec2", "expected_hash": read["hash"]})


def capture_rename(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/rename.md")
    return call(rpc, "tb_rename", {
        "repo": REPO, "path": "notes/rename.md", "to": "notes/renamed.md",
        "expected_hash": read["hash"]})


def capture_inline(rpc: StdioMcp) -> dict:
    folded = content(rpc, "fold.md")
    host = content(rpc, "into.md")
    payload = call(rpc, "tb_inline", {
        "repo": REPO, "path": "fold.md", "into": "into.md",
        "expected_hash": folded["hash"], "into_expected_hash": host["hash"]})
    # The folded record lands in the host under a clock-minted `^: b-<millis>` id, so the
    # host's content hash — the only answer field that id reaches — differs per run.
    # Its TYPE is what the schema test checks, so the value is frozen and a re-capture
    # stays byte-identical.
    payload["hash"] = "$HASH"
    return payload


def capture_promote(rpc: StdioMcp) -> dict:
    read = content(rpc, "notes/canvas.md")
    return call(rpc, "tb_promote", {
        "repo": REPO, "path": "notes/canvas.md", "to": "notes/promoted.md",
        "at": read["text"].index("content: x"), "expected_hash": read["hash"]})


def capture_add_dep(rpc: StdioMcp) -> dict:
    return call(rpc, "tb_add_dep", {"repo": REPO, "dep": "peer"})


def capture_remove_dep(rpc: StdioMcp) -> dict:
    return call(rpc, "tb_remove_dep", {"repo": REPO, "dep": "peer"})


def capture_add_workspace_member(rpc: StdioMcp) -> dict:
    return call(rpc, "tb_add_workspace_member", {"repo": REPO, "name": "aux", "role": "edit"})


def capture_set_workspace_member_role(rpc: StdioMcp) -> dict:
    return call(rpc, "tb_set_workspace_member_role",
                {"repo": REPO, "name": "aux", "role": "discover"})


def capture_remove_workspace_member(rpc: StdioMcp) -> dict:
    return call(rpc, "tb_remove_workspace_member", {"repo": REPO, "name": "aux"})


def capture_rename_type(rpc: StdioMcp) -> dict:
    definition = call(rpc, "tb_type", {"name": "note"}).get("type")
    if not isinstance(definition, dict) or not definition.get("hash"):
        raise CaptureError(f"tb_type returned no hash for note: {definition}")
    return call(rpc, "tb_rename_type", {
        "old_name": "note", "new_name": "memo", "expected_hash": definition["hash"]})


# Ordered: rename_type rewrites every `type: note` claim, so it runs last.
CAPTURES = [
    ("tb_write_file", capture_write_file),
    ("tb_edit_file", capture_edit_file),
    ("tb_delete_file", capture_delete_file),
    ("tb_create_file", capture_create_file),
    ("tb_batch", capture_batch),
    ("tb_assign_block_id", capture_assign_block_id),
    ("tb_rename_block_id", capture_rename_block_id),
    ("tb_rename", capture_rename),
    ("tb_inline", capture_inline),
    ("tb_promote", capture_promote),
    ("tb_add_dep", capture_add_dep),
    ("tb_remove_dep", capture_remove_dep),
    ("tb_add_workspace_member", capture_add_workspace_member),
    ("tb_set_workspace_member_role", capture_set_workspace_member_role),
    ("tb_remove_workspace_member", capture_remove_workspace_member),
    ("tb_rename_type", capture_rename_type),
]


def normalize(payload: dict, root: Path) -> str:
    """The corpus's normalization: root -> $FIXTURE, sha -> $COMMIT, sorted, indent 1."""
    text = json.dumps(payload, sort_keys=True).replace(str(root), "$FIXTURE")
    text = re.sub(r"\b[0-9a-f]{40}\b", "$COMMIT", text)
    return json.dumps(json.loads(text), indent=1, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", default=str(CRATE / "tests/fixtures/mutations"),
                        help="fixture directory (default: tests/fixtures/mutations)")
    parser.add_argument("--keep", action="store_true", help="keep the disposable fixture")
    options = parser.parse_args()

    for binary in (ENGINE, ADAPTER):
        if not (Path(binary).is_file() and os.access(binary, os.X_OK)):
            print(f"missing binary: {binary} (set AU_ENGINE_BIN / AU_MCP_RS_BIN)", file=sys.stderr)
            return 2

    out = Path(options.out)
    out.mkdir(parents=True, exist_ok=True)
    temp = tempfile.mkdtemp(prefix="au-mutations-")
    root = Path(temp)
    daemon = None
    written: list[tuple[str, list[str]]] = []
    try:
        repo = build_fixture(root)
        daemon = start_daemon(ENGINE, repo, root / "home")
        socket = wait_for_socket(root / "home")
        print(f"fixture: {repo}")
        print(f"engine:  {ENGINE}\nadapter: {ADAPTER}\nsocket:  {socket}")
        with StdioMcp([ADAPTER, "mcp", socket, "--allow-mutations", "--allow-composition",
                       "--allow-workspace-shape"], name="capture-mutations") as rpc:
            for tool, capture in CAPTURES:
                payload = capture(rpc)
                (out / f"{tool}.json").write_text(normalize(payload, repo))
                keys = ", ".join(sorted(payload))
                written.append((tool, sorted(payload)))
                print(f"  {tool}: {keys}")
        print(f"wrote {len(written)} fixtures to {out}")
        for tool, keys in written:
            print(f"  {out.name}/{tool}.json  ({len(keys)} keys)")
        return 0
    except CaptureError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    finally:
        if daemon is not None:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()
        if options.keep:
            print(f"kept fixture: {root}")
        else:
            import shutil
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
