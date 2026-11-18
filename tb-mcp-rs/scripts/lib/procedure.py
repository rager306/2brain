#!/usr/bin/env python3
"""The two repeatable procedures as state machines with code transitions.

C2 of the architecture plan: the FSM owns the process, the model is free inside a state,
and only the runtime may declare a state complete. This module is the runtime half — the
states, their checks and the transitions. The shell scripts own the model turn and call
this to know whether the turn moved the machine forward.

Two procedures:

  mutation_turn   read -> propose -> apply -> verify -> done
  recovery_turn   read -> propose -> unknown -> reconcile -> done

Why a state machine and not three if-statements: the checks are the contract, they are
tested without a model (`--selftest`), and a failing run names the state it stopped in
instead of printing "the turn looked wrong".

Exit codes: 0 all checks passed, 1 a check failed (the state is named), 2 bad usage.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from typing import Callable


@dataclass
class State:
    name: str
    requires: str          # what the caller must have produced to enter this state
    check: Callable[[dict], str | None]  # None = pass, string = the failure reason


@dataclass
class Procedure:
    name: str
    states: list[State]
    notes: str = ""

    def names(self) -> list[str]:
        return [state.name for state in self.states]

    def verify(self, context: dict) -> tuple[bool, list[tuple[str, str | None]]]:
        """Run every state's check in order; stop at the first failure."""
        trail: list[tuple[str, str | None]] = []
        for state in self.states:
            problem = state.check(context)
            trail.append((state.name, problem))
            if problem:
                return False, trail
        return True, trail


def _git(repo: str, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", repo, *args], capture_output=True, text=True, check=False
    ).stdout.strip()


def _commit_count(repo: str) -> int:
    out = _git(repo, "rev-list", "--count", "HEAD")
    return int(out) if out.isdigit() else 0


def _tracked_changes(repo: str) -> list[str]:
    out = _git(repo, "status", "--porcelain")
    return [line for line in out.splitlines() if line.strip()]


# --- the mutation turn -----------------------------------------------------------------


def check_target_written(context: dict) -> str | None:
    path = context["target_path"]
    expected = context.get("expected_text")
    if not os.path.isfile(path):
        return f"{path} does not exist: the turn wrote nothing"
    if expected is not None:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        if expected not in text:
            return f"{path} does not contain the intended text"
    return None


def check_one_commit(context: dict) -> str | None:
    before = context["commits_before"]
    after = _commit_count(context["repo"])
    if after != before + 1:
        return f"expected exactly one commit ({before} -> {before + 1}), got {before} -> {after}"
    return None


# The engine keeps its own state inside the repo it serves; that is not the turn's work.
# Both identity spellings are listed: a repository migrated to `.2brain` must not look like
# stray work, and an un-migrated one must keep working.
IGNORED_STATE = (".2brain/", ".arsumbris/", ".arscontexta/", ".claude/")


def check_no_stray_files(context: dict) -> str | None:
    stray = [
        line
        for line in _tracked_changes(context["repo"])
        if not line.startswith(" M ")  # modified tracked files are the intended edit
        and not any(marker in line for marker in IGNORED_STATE)
    ]
    if stray:
        return f"the turn left untracked or staged work: {stray}"
    return None


def check_history_carries_the_target(context: dict) -> str | None:
    name = os.path.basename(context["target_path"])
    changed = _git(context["repo"], "show", "--name-only", "--pretty=format:", "HEAD")
    if name not in changed:
        return f"the last commit does not touch {name}: {changed.splitlines()[:3]}"
    return None


MUTATION_TURN = Procedure(
    name="mutation_turn",
    states=[
        State("read", "tb_content returned the file and its hash", lambda c: None),
        State("propose", "the model chose an edit", lambda c: None),
        State("apply", "the adapter committed exactly once", check_one_commit),
        State("write-visible", "the file carries the intended text", check_target_written),
        State("history", "the commit names the file", check_history_carries_the_target),
        State("clean", "nothing else was touched", check_no_stray_files),
    ],
    notes="The model turn happens between propose and apply; this module cannot see inside it.",
)


# --- the recovery turn ----------------------------------------------------------------


def check_outcome_unknown(context: dict) -> str | None:
    transcript = context["transcript_path"]
    if not os.path.isfile(transcript):
        return f"{transcript} is missing: no transcript to inspect"
    with open(transcript, encoding="utf-8") as handle:
        text = handle.read()
    if "outcome_unknown" not in text:
        return "the transcript never shows outcome_unknown: the scenario did not run"
    return None


def check_no_retry(context: dict) -> str | None:
    """A second write after `outcome_unknown` is the failure this procedure exists to stop."""
    writes = context["write_calls_after_unknown"]
    if writes:
        return f"{writes} write call(s) after outcome_unknown: the model retried"
    return None


def check_reconcile_called(context: dict) -> str | None:
    if not context["reconcile_called"]:
        return "tb_reconcile was never called: the model guessed instead of asking"
    return None


def check_settled(context: dict) -> str | None:
    """Exactly one of applied/not_applied, and the file agrees with it."""
    said = context.get("reconcile_outcome")
    if said not in ("applied", "not_applied"):
        return f"reconcile answered {said!r}, not applied/not_applied"
    exists = os.path.isfile(context["target_path"])
    return None if said == ("applied" if exists else "not_applied") else (
        f"reconcile said {said} but the file {'exists' if exists else 'is missing'}"
    )


RECOVERY_TURN = Procedure(
    name="recovery_turn",
    states=[
        State("read", "tb_content returned the file and its hash", lambda c: None),
        State("propose", "the model chose an edit", lambda c: None),
        State("unknown", "the reply was lost and reported as outcome_unknown", check_outcome_unknown),
        State("no-retry", "the model did not write again", check_no_retry),
        State("reconcile", "the model asked tb_reconcile", check_reconcile_called),
        State("settled", "the answer matches the filesystem", check_settled),
    ],
    notes="The lost reply is injected by scripts/lost_reply_proxy.py.",
)

PROCEDURES = {procedure.name: procedure for procedure in (MUTATION_TURN, RECOVERY_TURN)}


# --- the self test: every check must reject a wrong context ---------------------------


def selftest() -> int:
    """Each check is proven to fail on a context that violates it, and pass on a good one."""
    import tempfile

    failures: list[str] = []
    with tempfile.TemporaryDirectory() as work:
        repo = os.path.join(work, "repo")
        os.makedirs(repo)
        subprocess.run(["git", "-C", repo, "init", "-q"], check=True)
        target = os.path.join(repo, "a.md")
        transcript = os.path.join(work, "transcript.txt")

        def commit_all(message: str) -> None:
            subprocess.run(["git", "-C", repo, "add", "-A"], check=True)
            subprocess.run(
                ["git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@e.invalid",
                 "commit", "-q", "-m", message],
                check=True,
            )

        with open(target, "w", encoding="utf-8") as handle:
            handle.write("before\n")
        commit_all("fixture")
        before = _commit_count(repo)

        with open(target, "w", encoding="utf-8") as handle:
            handle.write("after: the intended text\n")
        commit_all("change a.md")
        with open(transcript, "w", encoding="utf-8") as handle:
            handle.write("outcome_unknown: the reply was lost\n")

        good = {
            "repo": repo,
            "target_path": target,
            "expected_text": "the intended text",
            "commits_before": before,
            "transcript_path": transcript,
            "write_calls_after_unknown": 0,
            "reconcile_called": True,
            "reconcile_outcome": "applied",
        }
        ok, trail = MUTATION_TURN.verify(good)
        if not ok:
            failures.append(f"mutation_turn rejected a good run: {trail}")
        ok, trail = RECOVERY_TURN.verify(good)
        if not ok:
            failures.append(f"recovery_turn rejected a good run: {trail}")

        def expect_failure(state_name: str, context: dict, procedure: Procedure) -> None:
            ok, trail = procedure.verify(context)
            if ok:
                failures.append(f"{procedure.name}/{state_name} accepted a context that violates it")
            elif not any(name == state_name and problem for name, problem in trail):
                failures.append(
                    f"{procedure.name}: expected the failure at {state_name}, got {trail}"
                )

        expect_failure("apply", {**good, "commits_before": before - 1}, MUTATION_TURN)
        expect_failure("write-visible", {**good, "expected_text": "absent text"}, MUTATION_TURN)

        # `history` needs a file that exists (so write-visible passes) but is not in HEAD.
        other = os.path.join(repo, "other.md")
        with open(other, "w", encoding="utf-8") as handle:
            handle.write("the intended text\n")
        expect_failure("history", {**good, "target_path": other}, MUTATION_TURN)
        os.remove(other)

        # A stray file the turn did not intend must fail `clean`...
        stray = os.path.join(repo, "stray.txt")
        with open(stray, "w", encoding="utf-8") as handle:
            handle.write("x\n")
        expect_failure("clean", {**good, "target_path": target}, MUTATION_TURN)
        os.remove(stray)

        # ...while engine state under its own directory must not.
        engine_state = os.path.join(repo, ".arsumbris")
        os.makedirs(engine_state, exist_ok=True)
        with open(os.path.join(engine_state, "repo.yaml"), "w", encoding="utf-8") as handle:
            handle.write("name: fixture\n")
        if not MUTATION_TURN.verify({**good, "target_path": target})[0]:
            failures.append("engine state under .arsumbris/ was counted as stray work")

        expect_failure("unknown", {**good, "transcript_path": os.path.join(work, "none.txt")}, RECOVERY_TURN)
        expect_failure("no-retry", {**good, "write_calls_after_unknown": 1}, RECOVERY_TURN)
        expect_failure("reconcile", {**good, "reconcile_called": False}, RECOVERY_TURN)
        expect_failure("settled", {**good, "reconcile_outcome": "not_applied"}, RECOVERY_TURN)
        expect_failure("settled", {**good, "reconcile_outcome": "maybe"}, RECOVERY_TURN)

    for problem in failures:
        print(f"selftest: {problem}", file=sys.stderr)
    print(f"selftest: {'FAILED' if failures else 'ok'} ({len(failures)} problem(s))")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("list", help="print the two procedures and their states")
    sub.add_parser("selftest", help="prove every check rejects a wrong context")

    verify = sub.add_parser("verify", help="run one procedure's checks against a JSON context")
    verify.add_argument("procedure", choices=sorted(PROCEDURES))
    verify.add_argument("--context", required=True, help="path to a JSON context")

    arguments = parser.parse_args(argv)

    if arguments.command == "list":
        for name in sorted(PROCEDURES):
            procedure = PROCEDURES[name]
            print(f"{name}: {' -> '.join(procedure.names())}")
            for state in procedure.states:
                print(f"    {state.name:<14} requires: {state.requires}")
            if procedure.notes:
                print(f"    note: {procedure.notes}")
        return 0

    if arguments.command == "selftest":
        return selftest()

    procedure = PROCEDURES[arguments.procedure]
    try:
        with open(arguments.context, encoding="utf-8") as handle:
            context = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        print(f"procedure: cannot read the context: {error}", file=sys.stderr)
        return 2

    missing = [key for key in ("repo", "target_path") if key not in context]
    if missing:
        print(f"procedure: the context is missing {missing}", file=sys.stderr)
        return 2

    ok, trail = procedure.verify(context)
    for name, problem in trail:
        print(f"  {'pass' if problem is None else 'FAIL'}  {name}" + (f": {problem}" if problem else ""))
    print(f"{procedure.name}: {'all states passed' if ok else 'stopped at ' + trail[-1][0]}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
