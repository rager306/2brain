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

11. **Inotify Watcher Self-Trigger Prevention (`two-brain`):**
   - Never reload snapshots on `notify::EventKind::Access(Open|Read|Close(NoWrite))` or `Modify(Metadata)` (`atime` updates); filter strictly to `Create`, `Remove`, `Modify(Data|Name|Any|Other)`, and `Access(Close(Write))` so snapshot reads cannot cause an infinite 100% CPU reload loop.

12. **Span-Exact Rewrites, Multi-Step Partial-Effect Honesty, Batch SEM-02 Gate & Live Member Revalidation (`tb-app`, `tb-mcp`, `tb-fs-git`):**
   - Rewrite wikilink anchors strictly by exact parsed `(byte_start, byte_end)` spans whose resolved target equals the renamed locator — never global string replacement across unrelated links sharing a block ID.
   - In multi-step mutations (`promote_block`, `rename_block_id`, `rename_type`, `batch_mutations`), never drop dependent errors with `.is_ok()` and never return a rejectable pre-effect error (`CasMismatch`/`InvalidArguments`) once an earlier step has durably mutated disk; compensate if possible or return `AppError::OutcomeUnknown` naming every uncompensated/failed locator.
   - Enforce the SEM-02 prior-read gate (`ReadLedger` or stateless HMAC `read_receipt`) on every existing-file operation inside `tb_batch` before executing step 0, and invalidate all modified `(repo, path)` ledger entries afterward.
   - Revalidate every workspace member root relative to `workspace_root` with `O_NOFOLLOW` directory traversal on every operation (`root_for`) so a post-load member-dir symlink swap cannot redirect writes outside the workspace.

13. **IPC Envelope Field Isolation, Strict JSONL Logs & Commit Boundaries (`tb-ipc`, `tb-mcp`, `tb-fs-git`):**
   - Never use top-level IPC envelope field names (`id`, `method`, `verb`) as unaliased tool arguments across `IpcClient`; translate `id` -> `target_id` before crossing IPC so frame fields cannot clobber payload keys.
   - Never silently skip malformed JSONL lines in `review-decisions.jsonl`; fail closed on both read and pre-append validation so corrupted Negative Memory cannot silently restore dismissed items.
   - In SEP-2663 tasks that commit state (`tb_record_decision`), check cancellation before dispatch and never turn an already-committed write into `TaskExit::Cancelled`.

14. **Incremental Refresh Exactness & Overflow-Safe Watcher Queues (`tb-fs-git`, `two-brain`):**
   - Incremental snapshot refreshes (`refresh_snapshot_incremental`) must update ALL containing members when member roots overlap, and must refresh tracked symlink aliases (`indexed_symlinks`) whenever any workspace file changes so aliases never hold stale content.
   - Never discard `TrySendError::Full` in a bounded filesystem watcher channel; set an atomic `overflowed` flag + `Notify` wakeup and atomically clear it before running a forced full reload so bursts can never drop an update.

15. **HTTP Slow-Body Bounds, Incremental Search Parity & Pre-Indexed Fast-Path Guards (`tb-mcp`, `tb-search`, `tb-cognition`):**
   - Never rely on `header_read_timeout` alone on Streamable HTTP listeners; enforce a bounded per-request `POST` timeout (`408 Request Timeout` + `Connection: close`) so stalled request bodies cannot pin connection semaphore permits indefinitely.
   - Break BM25 score ties deterministically by canonical `Locator` (not insertion-order `doc_id`) and compact incremental `SearchIndex` tombstones once dead slots cross compaction thresholds so long-running watchers never drift in ranking order or leak vocabulary.
   - Never trust locator membership alone when taking a pre-indexed fast path (`detect_structural_gaps` over `SearchIndex`); verify exact body digest parity (`body_matches`) and fall back to exact `u32` tokenization if any body digest differs or any `u16` posting hits `u16::MAX`.

16. **Ingest Zero-TOCTOU Reads, Atomic Batch Commits & Lossless Safe Conversion (`tb-app`, `tb-fs-git`, `two-brain`):**
   - Open ingest source files strictly via dirfd-relative `openat(..., OFlags::NOFOLLOW)`, verify `fstat` regular-file type, and bound reads with `Read::take(MAX_SOURCE_BYTES + 1)` — never `symlink_metadata` followed by pathname `fs::read`.
   - Bind `--refresh` CAS `expected_hash` to the gated `VaultSnapshot` (never re-reading disk before commit) and execute multi-note ingest batches atomically under one `write.lock` + one Git commit with compensating rollback (`FsWorkspaceStore::commit_ingest_batch`).
   - Reject malformed Markdown frontmatter and malformed CSV quoting fail-closed; resolve batch links against a merged post-batch `(path_index, stem_index)`; preserve unconsumed JSON fields; re-escape `<`/`>` and Markdown link punctuation after HTML entity decoding so active HTML cannot be reintroduced.

17. **MCP Ingest Target Derivation Parity, Replay-First Outcomes & Writable Default Repo (`tb-mcp`, `tb-app`, `two-brain`):**
   - Derive refresh target paths in `TwoBrainMcp::handle_ingest` via `tb_app::ingest::convert_source` (the exact same converter as `App::ingest_payload`) before enforcing SEM-02 prior-read checks and ledger invalidation so JSON/JSONL `slug`/`title` overrides can never bypass SEM-02.
   - Run snapshot-dependent admission and CAS checks inside `execute_with_outcomes` so `request_id` retries replay committed outcomes instead of failing on post-commit disk state, and bound durable outcome responses under the 64 KiB journal frame limit.
   - Select the default writable ingest repository strictly from `own`/`primary`/`editable` members (failing closed on zero or ambiguous matches) AND refuse explicit `--repo <dependency>` targets across both `App::ingest_payload` and CLI `2brain ingest` so writes never land in a read-only `dependency` member.

18. **Complete Revocation/Egress Coverage, Audit Pre-Check & Release-Time Context Fence (`tb-mcp`, `tb-journal`, `tb-app`):**
   - When repository restrictions or egress blocks are active (`--policy`), enforce them across EVERY MCP surface — tools (requiring explicit allowed `repo`, blocking workspace-wide tools), `skills/*`, `resources/*`, `prompts/*`, and scoped claim extraction in `tb_context` — never on a partial tool allowlist.
   - Validate audit log readiness (`check_ready`, inode identity after locking, trailing-newline check) BEFORE dispatching any mutation so a corrupt/symlinked audit log fails closed rather than allowing unaudited writes.
   - Run the `tb_context` `Generation` release admission fence immediately before returning the completed response (after companion-pole retrieval, bidirectional claim assembly, and contradiction assessment).

19. **Descriptor-Relative Persistence, Atomic Staging, Locked Scope Authorization & Honest Postconditions (`tb-journal`, `tb-app`, `tb-mcp`, `tb-cognition`, `two-brain`):**
   - Perform temporary-file creation, atomic publication, and directory syncs strictly relative to the locked/trusted directory descriptor (`openat(..., O_CREAT|O_EXCL|O_NOFOLLOW)` with unique names + `renameat` + `fsync(dir_fd)`), and stage workspace restores inside a private `0o700` sibling directory that cleans up on failure before atomic rename so failed restores never leave half-populated targets.
   - Authorize BOTH the persisted `TaskPacket` scope and the proposed scope inside the same locked checkpoint transaction (`TaskScopeGuard`), and NEVER allow an MCP caller to drop critical obligations (`allow_drop_critical` must never be taken from MCP request arguments).
   - Separate post-effect `OutcomeUnknown` (`operation_outcome: "unknown"`) from definite `failed` rejections in `RunReceipt`, require a successfully reloaded snapshot plus verified file/link existence for `ZeroDiagnostics` and `LinkResolves` postconditions (marking unavailable evidence `"unverified"`), and block `InvestigationSession::decide_next` on `pending_reconciliation` unconditionally before criterion evaluation or route selection.

20. **G5 Statistical Integrity, Checked Accounting & Fail-Closed Origin/Seed Validation (`tb-cognition`, `two-brain`):**
   - Reject duplicate `(task_id, arm)` observations, duplicate `task_id` experiment rows, conflicting `task_id -> family` assignments, reused holdout `episode_id`s, zero sample minimums, non-finite thresholds/metrics (`min_null_z_score`, rank scores), and undefined zero-baseline token overheads before any promotion decision; exclude the entire `task_id` whenever any of its arms is training-contaminated, and evaluate critical regressions before pairing.
   - Accumulate token totals with checked `u128` arithmetic so large counts can neither panic in debug nor wrap in release.
   - Never silently drop unresolved PPR seed locators (`SolverStopReason::UnresolvedSeeds`), and fall back to normalized content SHA-256 origin digests when explicit provenance metadata is omitted so unlabelled duplicate copies cannot amplify graph rank.

21. **Session-Bounded Subscription Fan-Out, Honest Capability Advertising, Non-Inverted Polarity Guard & Safe Enrichment Splicing (`tb-mcp`, `tb-search`, `tb-app`, `two-brain`):**
    - Bind every per-session `resources/subscribe` fan-out task to a `Weak` subscription registry plus session `DropGuard`/`CancellationToken` so disconnected HTTP sessions never leak tasks or `Peer` handles; advertise `resources.subscribe` and accept `subscribe`/`listen` ONLY when a live `ResourceUpdateHub` is attached and egress is allowed.
    - In hybrid score fusion (`rerank_hits`), keep lexical weight fixed at `(1 - alpha) * norm_bm25` and zero ONLY the semantic boost (`+ semantic_weight * raw_cos`) on `SEM-48` negation polarity mismatches (including apostrophe contractions such as `don't`/`can't`), and filter non-finite scores/freshness multipliers before normalization.
    - When splicing frontmatter (`apply_enrichment_to_note`), replace only the keys actually supplied, handle multiline flow arrays and quoted keys, match `tb-syntax`'s strict `---` closing delimiter, refuse read-only `dependency` members, and persist offline cache/plan files via dirfd-relative `openat(O_CREAT|O_EXCL|O_NOFOLLOW)` + `sync_all` + `renameat` + `fsync(dir_fd)`.

22. **IPC Push Stream Isolation, Monotonic Snapshot Delta & Honest Capability Probing (`tb-contracts`, `tb-ipc`, `tb-mcp`, `two-brain`):**
    - Reject `/`, `\`, `.`, and `..` in `Locator::new` repository slugs; check `"subscribe"` on the IPC daemon socket ONLY when `"read"` and `"mutation"` are absent so tool arguments can never hijack RPC dispatch; enforce write timeouts on every subscribe reply and cancel all active connections via `DropGuard` on server shutdown.
    - Serialize daemon resource-update delta publishing behind a shared monotonic `last_published` snapshot cursor (`current.generation > prev.generation`) invoked on watcher reloads and non-replayed mutations only, and disable duplicate local mutation echoes in socket-backed `TwoBrainMcp` (`with_remote_resource_updates`).
    - Probe and claim the daemon's `resources` event stream BEFORE advertising `resources.subscribe`, and never advertise or open a daemon subscriber slot when `EgressPolicy::is_egress_blocked()` is true.

23. **Sparse-Field BM25 Normalization & Evaluation Simulation Parity (`tb-search`, `tb-app`, `two-brain`):**
    - Normalize each BM25 field's average length against `non_empty_docs[field]` (never corpus-wide `live_docs`) so optional fields populated on a subset of notes (`keywords`, `search_questions`) do not suffer length-normalization collapse; keep `keywords`/`aliases` separate from `search_questions` with configurable `SearchWeights` and `FreshnessPolicy`.
    - In `build_simulated_enriched_index`, mirror `build_enrichment_apply_plan` (skip `dependency` members, replace rather than double-count `keywords`/`search_questions`, preserve `snapshot.search.default_reference_days()` and `weights()`), and embed full note metadata (`title`, `description`, `keywords`, `search_questions`, `body`) into `HybridDocVectors` so both cosine similarity and `SEM-48` negation detection cover frontmatter.
24. **Non-destructive manifest splicing & strict config diagnostics (no silent drops or field-disabling coercion):**
    - Manifest writers (`update_repo_deps` on `.2brain/repo.yaml`) must splice only their owned section and preserve all unrelated sections (such as `search:`), comments, and custom metadata; fail closed on invalid UTF-8 or malformed YAML instead of overwriting.
    - Workspace config parsers (`parse_search_config_from_yaml` / `resolve_workspace_search_config`) must emit explicit `Diagnostic` entries for malformed YAML, invalid UTF-8, unknown sections/fields, out-of-range or non-finite (`NaN`/`Inf`) numbers, and duplicate `own`-member configs — preserving defaults on invalid fields instead of coercing negative/`NaN` weights to `0.0`.
    - Both `SearchWeights` and `FreshnessPolicy` must be stored sanitized (`FreshnessPolicy::sanitized()`) inside `SearchIndex`, and multi-member `workspace.yaml` workspaces must resolve `own` member `.2brain/repo.yaml` configs while ignoring `dependency` members.
