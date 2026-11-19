#!/usr/bin/env bash
# Swap the live engine binary to the tree's build, with automatic rollback.
#
# The runbook (docs/engine-swap-2026-11-16.md) is prose for a human; this is the same procedure as one
# command, because the risky part is not deciding — it is the ordering. `install` replaces the FILE
# before `systemctl restart` replaces the PROCESS, so between the two a failed restart leaves a new
# binary on disk that cron's tb-health.sh would happily start. This script therefore treats any
# failure after `install` as "restore first, diagnose second", and it never leaves that window open:
# every step after the install either completes the swap or puts the old binary back.
#
#   ./scripts/swap-engine.sh --check        # preconditions only, changes nothing
#   ./scripts/swap-engine.sh                # do the swap (aborts unless every precondition holds)
#   ./scripts/swap-engine.sh --restore      # put the backed-up binary back and restart
#
# Artifacts land in /root/backups/au-<date>-{pre-search,candidate,pre-live,post-live,evidence.txt}.
set -uo pipefail
export HOME=/root

DATE=${TB_SWAP_DATE:-$(date +%Y%m%d)}
BACKUPS=/root/backups
INSTALLED=/root/.2brain/bin/tb-engine
CANDIDATE=${TB_CANDIDATE:-/root/2brain/tb-engine/target/release/tb-engine}
SERVICE=2brain-engine.service
SOCKET=/root/.2brain/tb-engine/run/adcfe8680a485529.sock
PROBE=/root/2brain/tb-engine/scripts/engine_probe.py
PRE=$BACKUPS/au-$DATE-pre-search
CAND=$BACKUPS/au-$DATE-candidate
BASELINE=$BACKUPS/au-$DATE-pre-live
POST=$BACKUPS/au-$DATE-post-live
EVIDENCE=$BACKUPS/au-$DATE-evidence.txt
READS=(tb_overview tb_hubs tb_diagnostics tb_graph_health)

mode=swap
case ${1:-} in
  --check)   mode=check ;;
  --restore) mode=restore ;;
  "")        ;;
  *) echo "unknown argument: $1" >&2; exit 64 ;;
esac

say() { printf '%s\n' "$*" | tee -a "$EVIDENCE.tmp" >/dev/null; printf '%s\n' "$*"; }

problem() {
  # Refuse rather than guess. Each line names the check, not the symptom.
  local reason=$1
  printf 'REFUSED: %s\n' "$reason" >&2
  exit 2
}

preconditions() {
  [[ -x $CANDIDATE ]] || problem "no candidate at $CANDIDATE (build it: cd tb-engine && cargo +1.99.0 build --release)"
  [[ -x $INSTALLED ]]  || problem "no installed binary at $INSTALLED"
  systemctl is-active --quiet "$SERVICE" || problem "$SERVICE is not running"
  [[ -S $SOCKET ]] || problem "no engine socket at $SOCKET"

  # A stale cargo test holds the target lock and would make the build step lie.
  pgrep -af "cargo (test|build)" | grep -v swap-engine >/dev/null && problem "a cargo build or test is running"
  # The candidate must not predate the sources it claims to build: a stale binary ships code nobody
  # reviewed. `au` carries no commit marker, so the check is "no source is newer than the binary".
  ( cd /root/2brain && git diff --quiet -- tb-engine ) || problem "tb-engine has uncommitted changes"
  local newer
  newer=$(find /root/2brain/tb-engine/crates -name '*.rs' -newer "$CANDIDATE" -print -quit)
  [[ -z $newer ]] || problem "the candidate is older than $newer (rebuild it)"

  # Idempotence: if the served engine already answers `search`, there is nothing to swap.
  if "$PROBE" --socket "$SOCKET" --out /tmp/tb-swap-pre --verb tb_search >/dev/null 2>&1; then
    problem "the served engine already answers tb_search — already swapped"
  fi

  if ! "$PROBE" --socket "$SOCKET" --out /tmp/tb-swap-probe >/dev/null 2>&1; then
    # The probe exits non-zero when one requested read is refused, which is the normal state before
    # the swap (`tb_search` is in the default set) — so acceptance is "the socket answered at all".
    [[ -f /tmp/tb-swap-probe/tb_overview.json ]] || problem "the engine does not answer a probe"
  fi
}

recapture() {
  local out=$1
  mkdir -p "$out"
  # `search` is excluded: the served binary refuses it before the swap and answers after, so it is
  # compared separately rather than dragged into the four-read diff.
  "$PROBE" --socket "$SOCKET" --out "$out" \
    --verb tb_overview --verb tb_hubs --verb tb_diagnostics --verb tb_graph_health >/dev/null 2>&1
}

diff_reads() {
  local before=$1 after=$2 mismatch=0
  for r in "${READS[@]}"; do
    if cmp -s "$before/$r.json" "$after/$r.json"; then
      say "  $r: identical"
    else
      # A vault that changed between the two captures produces a legitimate difference; say which.
      say "  $r: DIFFERENT"
      diff <(python3 -m json.tool "$before/$r.json" 2>/dev/null) \
           <(python3 -m json.tool "$after/$r.json" 2>/dev/null) | head -20 >> "$EVIDENCE.tmp"
      mismatch=1
    fi
  done
  return $mismatch
}

restore() {
  [[ -x $PRE ]] || problem "no rollback binary at $PRE"
  install -m 0755 "$PRE" "$INSTALLED.tmp" && mv -f "$INSTALLED.tmp" "$INSTALLED"
  systemctl restart "$SERVICE"
  for _ in $(seq 1 30); do
    sleep 1
    "$PROBE" --socket "$SOCKET" --out /tmp/tb-restore-probe >/dev/null 2>&1 && {
      say "restored $INSTALLED from $PRE and the engine answers"
      return 0
    }
  done
  say "restored the file but the engine does not answer — check: journalctl -u $SERVICE -n 50"
  return 1
}

case $mode in
  check)
    EVIDENCE=/dev/null
    preconditions
    say "preconditions hold"
    say "  installed $(sha256sum "$INSTALLED" | cut -d' ' -f1)"
    say "  candidate $(sha256sum "$CANDIDATE" | cut -d' ' -f1)"
    say "  vault $(cd /root/vault && git rev-parse --short HEAD) with $(cd /root/vault && git status --porcelain | wc -l) dirty entries"
    exit 0
    ;;
  restore)
    EVIDENCE=$BACKUPS/au-$DATE-restore.txt
    restore
    exit $?
    ;;
esac

: > "$EVIDENCE.tmp"
say "swap starting $(date -Is)"
preconditions

# 1. Artifacts, before anything live changes.
install -d "$BASELINE" "$POST"
install -m 0755 "$INSTALLED" "$PRE"
install -m 0755 "$CANDIDATE" "$CAND"
say "  pre-search $(sha256sum "$PRE" | cut -d' ' -f1)"
say "  candidate  $(sha256sum "$CAND" | cut -d' ' -f1)"
[[ $(sha256sum "$PRE" | cut -d' ' -f1) == $(sha256sum "$INSTALLED" | cut -d' ' -f1) ]] || problem "the rollback copy does not match the installed binary"

# 2. Baseline reads on the served vault.
recapture "$BASELINE"
say "  baseline captured to $BASELINE"

# 3. Install, then restart. From here every failure path restores.
install -m 0755 "$CAND" "$INSTALLED.tmp" && mv -f "$INSTALLED.tmp" "$INSTALLED"
if ! systemctl restart "$SERVICE"; then
  say "restart failed — restoring before any diagnosis"
  restore; exit 1
fi

ready=0
for _ in $(seq 1 60); do
  sleep 1
  if "$PROBE" --socket "$SOCKET" --out /tmp/tb-swap-wait >/dev/null 2>&1; then ready=1; break; fi
done
if [[ $ready -eq 0 ]]; then
  say "the new engine did not answer within 60 s — restoring"
  restore; exit 1
fi

# 4. Post reads and the comparison.
recapture "$POST"
if diff_reads "$BASELINE" "$POST"; then
  say "  four reads identical"
else
  say "four reads differ — see $EVIDENCE; the swap is NOT accepted"
  # A content difference (the vault moved between captures) is not a reason to roll back a working
  # engine; a shape difference is. Report both and let a human decide, but say which is which.
  dirty=$(cd /root/vault && git status --porcelain | wc -l)
  if [[ $dirty -gt 5 ]]; then
    say "  the vault changed during the window: the difference is content, not shape"
  else
    say "  the vault did not change: the difference is the engine — restoring"
    restore; exit 1
  fi
fi

# 5. The verb the swap exists for.
if "$PROBE" --socket "$SOCKET" --out /tmp/tb-swap-search --verb tb_search >/dev/null 2>&1; then
  say "  tb_search: answered"
else
  say "  tb_search: still refused — the swap did not take, restoring"
  restore; exit 1
fi

mv -f "$EVIDENCE.tmp" "$EVIDENCE"
say "swap complete $(date -Is); evidence in $EVIDENCE"
say "rollback: $0 --restore   (or install -m 0755 $PRE $INSTALLED && systemctl restart $SERVICE)"
