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
