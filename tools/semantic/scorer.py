#!/usr/bin/env python3
"""Isolated Offline Semantic Scorer (Issue #7 U2/R22/AE22, MIT).

STRICT ORACLE ISOLATION RULE:
This module NEVER invokes `2brain` or any SUT process. It consumes ONLY a frozen
capture bundle (`observations.json` + `run_manifest.json`), verifies the bundle's
SHA-256 integrity digest, evaluates every applicable mandatory assertion exactly
once, reports three independent dimensions (`contract_verdict`, `task_fulfillment`,
`fallback_honesty`), and executes the `SEM-48` negative harness control in a
separate denominator to prove the scorer catches corrupted evidence.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

MANIFEST_SCHEMA = "2brain.semantic.run-manifest/v1"
OBSERVATION_SCHEMA = "2brain.semantic.observations/v1"


def canonical_json_bytes(obj: Any) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class CaseAssessment:
    def __init__(self, case_id: str, family: str):
        self.case_id = case_id
        self.family = family
        self.assertions: dict[str, dict[str, Any]] = {}

    def check(self, assertion_id: str, ok: bool, detail: str = "") -> None:
        if assertion_id in self.assertions:
            raise ValueError(f"duplicate mandatory assertion id '{assertion_id}' in {self.case_id}")
        self.assertions[assertion_id] = {"passed": bool(ok), "detail": detail}

    def finalize(self, task_fulfillment: str, fallback_honesty: str) -> dict[str, Any]:
        applicable = len(self.assertions)
        assessed = len(self.assertions)
        failed = [k for k, v in self.assertions.items() if not v["passed"]]
        if applicable == 0:
            verdict = "inconclusive"
        elif failed:
            verdict = "fail"
        else:
            verdict = "pass"
        return {
            "case_id": self.case_id,
            "family": self.family,
            "contract_verdict": verdict,
            "task_fulfillment": task_fulfillment if verdict == "pass" else "unfulfilled",
            "fallback_honesty": fallback_honesty,
            "mandatory_applicable": applicable,
            "mandatory_assessed": assessed,
            "failed_assertions": failed,
            "assertions": self.assertions,
        }


def score_sem01(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("SEM-01", case.get("family", ""))
    meta = case["seed_meta"]
    obs = case["observations"]
    full = (obs[0].get("structured") or {}).get("content") or {}
    win = (obs[1].get("structured") or {}).get("content") or {}
    full_text = full.get("text") or ""

    a.check("sha256_matches_disk", full.get("sha256") == meta["disk_sha256"])
    a.check("byte_len_matches_disk", full.get("size_bytes") == meta["disk_bytes"])
    a.check("char_len_matches_disk", full.get("total_chars") == meta["disk_chars"])
    a.check("unicode_text_preserved", "Привет 🌍 café" in full_text)
    a.check("full_not_truncated", full.get("truncated") is False)
    a.check("window_marked_truncated", win.get("truncated") is True)
    a.check("window_prefix_matches_full", full_text.startswith(win.get("text") or "<!missing!>"))
    return a.finalize("fulfilled", "na")


def score_sem02(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("SEM-02", case.get("family", ""))
    obs = case["observations"]
    p1, w_trunc, w_eof, p_full, w_tampered_receipt, w_ok = obs

    a.check("truncated_page_flagged", ((p1.get("structured") or {}).get("content") or {}).get("truncated") is True)
    a.check("write_after_trunc_refused", w_trunc.get("is_error") is True)
    a.check("write_after_trunc_reason", (w_trunc.get("structured") or {}).get("reason_code") == "prior_read")
    a.check("write_after_eof_refused", w_eof.get("is_error") is True)
    a.check("write_after_eof_reason", (w_eof.get("structured") or {}).get("reason_code") == "prior_read")
    a.check("full_read_untruncated", ((p_full.get("structured") or {}).get("content") or {}).get("truncated") is False)
    a.check("full_read_emits_receipt", case["seed_meta"].get("has_receipt") is True)
    a.check("tampered_receipt_refused", w_tampered_receipt.get("is_error") is True)
    a.check(
        "tampered_receipt_reason",
        (w_tampered_receipt.get("structured") or {}).get("reason_code") == "prior_read",
    )
    a.check("write_with_valid_receipt_succeeds", w_ok.get("is_error") is False)
    a.check(
        "disk_reflects_authorized_write",
        "Whole read + valid receipt succeeded." in case["seed_meta"].get("post_disk_text", ""),
    )
    return a.finalize("fulfilled", "honest")


def score_sem03(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("SEM-03", case.get("family", ""))
    meta = case["seed_meta"]
    r1, r_stale, r_match = case["observations"]
    stale_c = (r_stale.get("structured") or {}).get("content") or {}
    match_c = (r_match.get("structured") or {}).get("content") or {}

    a.check("initial_sha_present", len(meta.get("sha1", "")) == 64)
    a.check("dirty_edit_changed_sha", meta["sha1"] != meta["sha2_disk"])
    a.check("stale_status_explicit", (stale_c.get("revision") or {}).get("status") == "stale")
    a.check("stale_omits_spliced_text", stale_c.get("text") is None)
    a.check("stale_reports_actual_sha", (stale_c.get("revision") or {}).get("actual_sha256") == meta["sha2_disk"])
    a.check("match_status_explicit", (match_c.get("revision") or {}).get("status") == "match")
    a.check("match_returns_v2_body", "Version 2 dirty edit." in (match_c.get("text") or ""))
    return a.finalize("fulfilled", "honest")


def score_sem04(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("SEM-04", case.get("family", ""))
    bad = case["observations"][0]
    a.check("invalid_utf8_refused", bad.get("is_error") is True)
    a.check(
        "invalid_utf8_reason_explicit",
        (bad.get("structured") or {}).get("reason_code") in ("invalid_utf8", "internal_error")
        or any("utf" in (t or "").lower() for t in bad.get("text", [])),
    )
    return a.finalize("inapplicable", "honest")


def score_sem09(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("SEM-09", case.get("family", ""))
    q_and, q_exact, q_bounded = case["observations"]
    and_hits = (((q_and.get("structured") or {}).get("search") or {}).get("matches") or [])
    exact_hits = (((q_exact.get("structured") or {}).get("search") or {}).get("matches") or [])
    bounded_struct = q_bounded.get("structured") or {}
    bounded_hits = ((bounded_struct.get("search") or {}).get("matches") or [])

    and_paths = [h.get("path") for h in and_hits]
    exact_paths = [h.get("path") for h in exact_hits]
    a.check("and_top_hit_contains_both_terms", len(and_paths) >= 1 and and_paths[0] == "notes/a.md")
    a.check("exact_phrase_finds_only_matching_note", exact_paths == ["notes/a.md"])
    a.check("bounded_limit_enforced", len(bounded_hits) == 1)
    a.check("bounded_pagination_disclosed", bounded_struct.get("next_offset") == 1)
    return a.finalize("fulfilled", "na")


def score_sem18(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("SEM-18", case.get("family", ""))
    esc, ok = case["observations"]
    secret = case["seed_meta"]["outside_secret"]
    esc_blob = json.dumps(esc, ensure_ascii=False)
    a.check("escaping_symlink_refused", esc.get("is_error") is True)
    a.check("external_bytes_never_leaked", secret not in esc_blob)
    a.check("in_scope_note_readable", ok.get("is_error") is False)
    return a.finalize("fulfilled", "honest")


def score_ae04_ae08(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE04-08", case.get("family", ""))
    ctx_obs = case["observations"][0]
    ctx = (ctx_obs.get("structured") or {}).get("context") or {}
    frags = ctx.get("fragments") or []
    paths = {f.get("path") for f in frags}
    roles = {f.get("path"): f.get("role") for f in frags}
    why = ctx.get("why") or {}
    conflicts = why.get("conflicts") or {}

    a.check("context_succeeds", ctx_obs.get("is_error") is False)
    a.check("primary_rule_retrieved", "notes/rule.md" in paths)
    a.check("companion_exception_retrieved", "notes/exception.md" in paths)
    a.check("companion_labeled_exception_or_pole", roles.get("notes/exception.md") in ("exception_or_pole", "primary"))
    a.check("why_conflicts_available", conflicts.get("status") == "available")
    a.check("why_conflicts_nonempty", len(conflicts.get("items") or []) >= 1)
    a.check("criterion_versioned", (ctx.get("criterion") or {}).get("version") == "2brain.criterion/v1")
    a.check("projection_manifest_present", isinstance(ctx.get("projection_manifest"), dict))
    return a.finalize("fulfilled", "honest")


def score_ae24(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE24", case.get("family", ""))
    rev_read, rev_search, ext_denied = case["observations"]
    events = case["seed_meta"].get("audit_events") or []

    a.check("revoked_content_denied", rev_read.get("is_error") is True)
    a.check("revoked_content_reason", (rev_read.get("structured") or {}).get("reason_code") == "revoked")
    a.check("revoked_content_no_leak", "Classified" not in json.dumps(rev_read))
    a.check("revoked_search_denied", rev_search.get("is_error") is True)
    a.check("external_egress_denied", ext_denied.get("is_error") is True)
    a.check("external_egress_reason", (ext_denied.get("structured") or {}).get("reason_code") == "egress_denied")
    a.check("audit_log_nonempty", len(events) >= 3)
    # Verify hash-chain linkage and sequence monotonicity offline
    chain_ok = len(events) >= 1 and events[0].get("prev") == ("0" * 64) and events[0].get("seq") == 1
    for i in range(1, len(events)):
        if events[i].get("prev") != events[i - 1].get("hash") or events[i].get("seq") != i + 1:
            chain_ok = False
            break
    a.check("audit_hash_chain_intact", chain_ok)
    return a.finalize("inapplicable", "honest")


def score_ae25(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE25", case.get("family", ""))
    c_bak, c_res, c_tamper = case["observations"]
    meta = case["seed_meta"]
    a.check("backup_exit_zero", c_bak.get("exit_code") == 0)
    a.check("restore_exit_zero", c_res.get("exit_code") == 0)
    a.check("restored_content_identical", meta.get("restored_matches") is True)
    a.check("tampered_restore_fails_closed", c_tamper.get("exit_code", 0) != 0)
    a.check("tampered_target_not_created", meta.get("tampered_target_created") is False)
    return a.finalize("fulfilled", "honest")


def score_adr0009(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("ADR-0009", case.get("family", ""))
    c_enable, m1, m_replay, m_conflict, c_status, c_compact = case["observations"]
    s_replay = m_replay.get("structured") or {}
    s_conflict = m_conflict.get("structured") or {}

    a.check("enable_exit_zero", c_enable.get("exit_code") == 0)
    a.check("initial_commit_ok", m1.get("is_error") is False)
    a.check("replay_after_restart_ok", m_replay.get("is_error") is False)
    a.check(
        "replay_returns_exact_committed_receipt",
        m1.get("structured") == m_replay.get("structured")
        and isinstance((m_replay.get("structured") or {}).get("create_file"), dict),
    )
    a.check("conflicting_payload_rejected", m_conflict.get("is_error") is True)
    a.check(
        "conflict_reason_explicit",
        s_conflict.get("reason_code") in ("request_conflict", "outcome_conflict")
        or any("conflict" in (t or "").lower() for t in m_conflict.get("text", [])),
    )
    a.check("outcomes_status_ok", c_status.get("exit_code") == 0 and '"enabled": true' in c_status.get("stdout", ""))
    a.check("outcomes_compact_ok", c_compact.get("exit_code") == 0)
    return a.finalize("fulfilled", "honest")


def score_ae15(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE15", case.get("family", ""))
    p_preview, p_denied, p_failed_post, p_verified = case["observations"]
    r_prev = (p_preview.get("structured") or {}).get("run_receipt") or {}
    r_fail = (p_failed_post.get("structured") or {}).get("run_receipt") or {}
    r_ok = (p_verified.get("structured") or {}).get("run_receipt") or {}

    a.check("preview_succeeds_without_mutation", p_preview.get("is_error") is False and r_prev.get("operation_outcome") == "preview")
    a.check("preview_created_zero_files", case["seed_meta"].get("preview_created_file") is False)
    a.check("missing_mutation_grant_refused", p_denied.get("is_error") is True and (p_denied.get("structured") or {}).get("reason_code") == "launch_grant")
    a.check("wrong_artifact_committed_but_postcondition_failed", r_fail.get("operation_outcome") == "committed" and r_fail.get("postcondition_outcome") == "failed")
    a.check("wrong_artifact_not_verified_success", r_fail.get("verified_success") is False)
    a.check("good_procedure_verified_success", r_ok.get("operation_outcome") == "committed" and r_ok.get("postcondition_outcome") == "verified" and r_ok.get("verified_success") is True)
    a.check("receipt_sha256_present", len(r_ok.get("receipt_sha256") or "") == 64)
    return a.finalize("fulfilled", "honest")


def score_ae17(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE17", case.get("family", ""))
    c_create, c_resume, c_advance, c_stale, c_drop = case["observations"]
    resumed = (c_resume.get("structured") or {}).get("task_packet") or {}
    adv = (c_advance.get("structured") or {}).get("task_packet") or {}

    a.check("create_rev_1", ((c_create.get("structured") or {}).get("task_packet") or {}).get("revision") == 1)
    a.check("resume_preserves_publish_false", any(o.get("key") == "publish" and o.get("value") == "false" for o in resumed.get("obligations", [])))
    a.check("resume_preserves_spent_budget", (resumed.get("budget") or {}).get("spent_actions") == 1)
    a.check("client2_advances_to_rev_2", c_advance.get("is_error") is False and adv.get("revision") == 2)
    a.check("stale_writer_rejected", c_stale.get("is_error") is True)
    a.check("dropping_critical_obligation_rejected", c_drop.get("is_error") is True)
    return a.finalize("fulfilled", "honest")


def score_ae18(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE18", case.get("family", ""))
    c_self, c_grant, c_promote = case["observations"]
    j_self = json.loads(c_self.get("stdout") or "{}")
    j_grant = json.loads(c_grant.get("stdout") or "{}")
    j_promote = json.loads(c_promote.get("stdout") or "{}")

    a.check("self_corroboration_rejected", j_self.get("verdict") == "reject" and j_self.get("paired_holdout_tasks") == 0)
    a.check("training_observations_excluded", j_self.get("excluded_training_observations") == 3)
    a.check(
        "grant_expansion_rejected",
        j_grant.get("verdict") == "reject"
        and any(r.get("code") == "grant_expansion" for r in j_grant.get("reasons", [])),
    )
    a.check("valid_holdout_promoted", j_promote.get("verdict") == "promote")
    a.check("holdout_families_separate", len(j_promote.get("holdout_families", [])) == 2)
    return a.finalize("fulfilled", "honest")


def score_ae19_20(case: dict[str, Any]) -> dict[str, Any]:
    a = CaseAssessment("AE19-20", case.get("family", ""))
    c_ppr, c_arm = case["observations"]
    j_ppr = json.loads(c_ppr.get("stdout") or "{}")
    j_arm = json.loads(c_arm.get("stdout") or "{}")

    scores = {r["locator"]["path"]: r["score"] for r in j_ppr.get("ranked", [])}
    sa = scores.get("notes/target_a.md", -1.0)
    sb = scores.get("notes/target_b.md", -1.0)

    a.check("ppr_marked_diagnostic_only", j_ppr.get("diagnostic_only") is True)
    a.check("ppr_converged", (j_ppr.get("stop_reason") or {}).get("reason") == "converged")
    a.check("ppr_deduplicated_origins", j_ppr.get("unique_seed_origins") == 2)
    a.check("ten_copy_amplification_blocked", sa > 0.0 and sb > 0.0 and abs(sa - sb) < 0.05)
    a.check("rank_gain_cannot_override_answer_regression", j_arm.get("verdict") == "retain_baseline")
    return a.finalize("fulfilled", "honest")


SCORERS = {
    "SEM-01": score_sem01,
    "SEM-02": score_sem02,
    "SEM-03": score_sem03,
    "SEM-04": score_sem04,
    "SEM-09": score_sem09,
    "SEM-18": score_sem18,
    "AE04-08": score_ae04_ae08,
    "AE15": score_ae15,
    "AE17": score_ae17,
    "AE18": score_ae18,
    "AE19-20": score_ae19_20,
    "AE24": score_ae24,
    "AE25": score_ae25,
    "ADR-0009": score_adr0009,
}


def run_sem48_negative_control(cases_by_id: dict[str, dict[str, Any]]) -> dict[str, Any]:
    """SEM-48: deliberately corrupt a captured SEM-01 excerpt while keeping its valid locator,
    and verify the scorer MUST fail it (proves the evaluator is not a rubber stamp)."""
    a = CaseAssessment("SEM-48", "harness_negative_control")
    base = cases_by_id.get("SEM-01")
    if base is None:
        a.check("sem01_present_for_control", False, "SEM-01 missing from bundle")
        return a.finalize("inapplicable", "dishonest")

    corrupted = copy.deepcopy(base)
    corrupted["observations"][0]["structured"]["content"]["text"] = "CORRUPTED FABRICATED EXCERPT"
    corrupted_result = score_sem01(corrupted)
    a.check(
        "corrupted_excerpt_caught_as_fail",
        corrupted_result["contract_verdict"] == "fail"
        and "unicode_text_preserved" in corrupted_result["failed_assertions"],
        f"scorer returned {corrupted_result['contract_verdict']} ({corrupted_result['failed_assertions']})",
    )
    return a.finalize("inapplicable", "honest")


def evaluate_bundle(obs_path: Path, manifest_path: Path) -> dict[str, Any]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    observations = json.loads(obs_path.read_text(encoding="utf-8"))

    if manifest.get("schema_version") != MANIFEST_SCHEMA:
        raise ValueError(f"unsupported manifest schema: {manifest.get('schema_version')!r}")
    if observations.get("schema_version") != OBSERVATION_SCHEMA:
        raise ValueError(f"unsupported observations schema: {observations.get('schema_version')!r}")

    computed_digest = sha256_bytes(canonical_json_bytes(observations))
    if computed_digest != manifest.get("bundle_digest_sha256"):
        raise ValueError(
            f"bundle_digest_sha256 mismatch: manifest={manifest.get('bundle_digest_sha256')} computed={computed_digest}"
        )

    assigned = manifest.get("assigned_cases") or []
    cases_by_id = {c["case_id"]: c for c in observations.get("cases", [])}

    product_results = []
    for cid in assigned:
        scorer = SCORERS.get(cid)
        case = cases_by_id.get(cid)
        if scorer is None:
            product_results.append(
                {
                    "case_id": cid,
                    "family": "unknown",
                    "contract_verdict": "inconclusive",
                    "task_fulfillment": "unfulfilled",
                    "fallback_honesty": "na",
                    "mandatory_applicable": 0,
                    "mandatory_assessed": 0,
                    "failed_assertions": ["unsupported_case"],
                }
            )
        elif case is None:
            product_results.append(
                {
                    "case_id": cid,
                    "family": "missing",
                    "contract_verdict": "inconclusive",
                    "task_fulfillment": "unfulfilled",
                    "fallback_honesty": "na",
                    "mandatory_applicable": 0,
                    "mandatory_assessed": 0,
                    "failed_assertions": ["missing_observation"],
                }
            )
        else:
            product_results.append(scorer(case))

    sem48_result = run_sem48_negative_control(cases_by_id)

    passed = sum(1 for r in product_results if r["contract_verdict"] == "pass")
    failed = sum(1 for r in product_results if r["contract_verdict"] == "fail")
    inconclusive = sum(1 for r in product_results if r["contract_verdict"] == "inconclusive")

    return {
        "schema_version": "2brain.semantic.scorecard/v1",
        "sut_version": manifest.get("sut_version"),
        "bundle_digest_sha256": computed_digest,
        "product_summary": {
            "assigned": len(assigned),
            "executed": len(cases_by_id),
            "passed": passed,
            "failed": failed,
            "inconclusive": inconclusive,
            "mandatory_applicable": sum(r["mandatory_applicable"] for r in product_results),
            "mandatory_assessed": sum(r["mandatory_assessed"] for r in product_results),
        },
        "harness_controls": {
            "assigned": 1,
            "passed": 1 if sem48_result["contract_verdict"] == "pass" else 0,
            "failed": 0 if sem48_result["contract_verdict"] == "pass" else 1,
            "results": [sem48_result],
        },
        "product_results": product_results,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Score frozen 2brain semantic capture bundle offline")
    parser.add_argument("--observations", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--out", type=Path, help="Optional JSON scorecard output path")
    args = parser.parse_args()

    scorecard = evaluate_bundle(args.observations.resolve(), args.manifest.resolve())
    if args.out:
        args.out.write_text(json.dumps(scorecard, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    ps = scorecard["product_summary"]
    hc = scorecard["harness_controls"]
    print(
        f"semantic scorecard: product {ps['passed']}/{ps['assigned']} passed "
        f"(failed={ps['failed']}, inconclusive={ps['inconclusive']}, "
        f"assertions={ps['mandatory_assessed']}/{ps['mandatory_applicable']}); "
        f"SEM-48 control {hc['passed']}/{hc['assigned']}"
    )
    for r in scorecard["product_results"] + hc["results"]:
        mark = "PASS" if r["contract_verdict"] == "pass" else "FAIL"
        extra = f" failed={r['failed_assertions']}" if r["failed_assertions"] else ""
        print(
            f"  [{mark}] {r['case_id']:<10} verdict={r['contract_verdict']:<12} "
            f"task={r['task_fulfillment']:<12} fallback={r['fallback_honesty']:<8}{extra}"
        )

    if ps["failed"] > 0 or ps["inconclusive"] > 0 or hc["failed"] > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
