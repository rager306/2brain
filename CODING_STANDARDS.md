# Coding Standards — 2brain v2 (MIT)

Defect classes caught by adversarial review and fixed by structural invariants:

1. **Zero-TOCTOU Filesystem Confinement (`tb-fs-git`):**
   - Never validate a path with `canonicalize()` and then reopen it by pathname (`fs::read(&real)`).
   - Always resolve member-relative paths component-by-component from an open member-root directory FD using `rustix::fs::openat(..., OFlags::NOFOLLOW)` and inspect `File::metadata()` (`fstat`) on the opened FD before reading.
   - Apply the exact same confinement to `.2brain/repo.yaml`, `.2brain/ignore`, `type/*.type.yaml`, and markdown notes.

2. **Snapshot Freshness & Release Admission (`tb-app`, `two-brain`):**
   - Never serve unverified snapshot fragments alongside live disk reads (`tb_content`).
   - Run a debounced filesystem watcher (`notify`) to advance `Generation` on external edits, and verify live SHA-256 on selected `tb_context` fragments before releasing the bundle (auto-reloading once or failing closed with `SnapshotStale`).

3. **Slice-Safe Excerpt Windows (`tb-search`):**
   - When snapping a window's left edge forward to whitespace, restrict the scan to `body[from..end]` (never `body[from..]`) so whitespace-free punctuation-separated text cannot advance `from > to` and panic on `body[from..to]`.

4. **Strict Diagnostics & Unambiguous Link Resolution (`tb-fs-git`, `tb-schema`):**
   - Never silently drop malformed `*.type.yaml` files, `.2brain/ignore` entries, ambiguous duplicate stems, or repository-qualified wikilinks; emit explicit `Diagnostic` records.

5. **Pure-Domain Miri Invariant (`crates/domain/*`):**
   - All 6 domain crates (`tb-contracts`, `tb-syntax`, `tb-schema`, `tb-graph`, `tb-search`, `tb-cognition`) carry zero I/O, zero `tokio`, and must pass `cargo miri test`.

6. **Interprocess Write Locking & Zero-TOCTOU Dirfd Mutations (`tb-fs-git`):**
   - Hold both an in-process mutex and an interprocess `fs4` `write.lock` across CAS verification, filesystem mutation, and Git commit.
   - Perform all file creates, atomic temp-writes, unlinks, and renames strictly via dirfd-relative `rustix::fs::{openat, mkdirat, unlinkat, renameat, renameat_with}` (`O_NOFOLLOW`) — never pathname-based `fs::remove_file`/`fs::rename` after validation.

7. **Honest Post-Effect Failures & Durable Replay (`tb-app`, `tb-journal`, `tb-fs-git`):**
   - Never mark a mutation `Rejected` in the durable outcome registry after disk state has already changed; compensate/restore pre-effect bytes on Git failure, and report `OutcomeUnknown` (`Executing`) if rollback itself fails.
   - Never ignore `append_response` errors after writing `Committed`; propagate failure so callers know replayability is unconfirmed.

8. **Daemon Socket Ownership & CSPRNG Keys (`tb-ipc`, `tb-mcp`):**
   - Acquire an exclusive `fs4` `<socket>.lock` BEFORE unlinking any stale socket; set `0700` on the run directory and `0600` on the bound Unix socket.
   - Generate HMAC signing keys exclusively from the OS CSPRNG (`getrandom::fill`) and fail startup if entropy is unavailable.

9. **Strict Prefix Stripping, Generation Fences & Unambiguous Hash Encoding (`tb-mcp`, `tb-app`, `tb-cognition`):**
   - Never use `trim_start_matches` for MCP tool name dispatch (use `strip_prefix` after verifying exact tool membership in the advertised launch catalog).
   - Always recheck `self.snapshot.load().generation == gen_before` before caching or releasing derived analytics (`orient`, `analyze_topology`, `detect_gaps`, `audit_bias`, `audit_contradictions`, `fragility_report`, `diff_graphs`).
   - Always length-prefix variable-length fields in SHA-256 identifiers (`question_id`, `conflict_id`) rather than joining with literal delimiters.

10. **Strict Git-Object Parity, Atomic Concurrency Quotas & Explicit Ambiguity (`tb-fs-git`, `tb-mcp`, `tb-app`, `tb-cognition`):**
   - Historical Git snapshot loaders (`load_snapshot_at_git_ref`) must apply `.2brain/ignore` from the selected commit with the same path-and-parent rules as live scans, and must fail closed on premature EOF, malformed/mismatched `ls-tree` or `cat-file --batch` headers, writer errors, or non-zero child exit status — never returning a truncated partial snapshot.
   - Enforce concurrent async MCP task quotas with an atomic `tokio::sync::Semaphore` permit held until the blocking worker finishes, never a non-atomic check-then-spawn counter.
   - Never coerce an ambiguous duplicate stem in `conflicts_with` / `supersedes` into a synthetic fallback `Locator`; emit an explicit `AmbiguousReference` contradiction warning naming all candidate poles.



