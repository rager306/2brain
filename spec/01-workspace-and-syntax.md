# Spec v1.0 — Workspace Layout, Corpus Boundary, and Markdown Syntax

**Status:** Normative clean-room specification (Phase 0) · **License:** MIT
**Scope:** On-disk workspace discovery, member resolution, corpus ignore rules, symlink safety, exact byte receipts, and Markdown/frontmatter/wikilink/block-id syntax.

---

## 1. Workspace and Member Discovery

A 2brain workspace is either:
1. **Single folder repository:** a directory containing `.2brain/repo.yaml`.
2. **Multi-member workspace:** a directory containing `workspace.yaml` declaring member paths and roles (`own` vs `dependency`).

### 1.1. Repository Manifest (`.2brain/repo.yaml`)

```yaml
type: 2brain.repo                 # optional self-descriptor
name: vault                       # canonical repository identifier (slug)
description: "..."                # optional human description
deps:                             # optional dependency list
  - name: mcp-bundle              # resolved via device-global repos.yaml or workspace.yaml
```

- **Folder-name check:** if the directory basename differs from declared `name`, the declared `name` wins and the engine emits a `drift` diagnostic `repo-folder-name-mismatch`.
- **Device-global repo registry:** `<2BRAIN_HOME>/engine/config/repos.yaml` (where `2BRAIN_HOME` defaults to `$HOME/.2brain`) maps `{ name, path }` entries for resolving declared `deps`. `2BRAIN_HOME` always takes precedence over `$HOME/.2brain`.

### 1.2. Corpus Ignore Boundary (`.2brain/ignore`)

- Always excluded from indexing and graph nodes: `.git/`, `.2brain/engine/`, binary files (NUL byte in first 8 KiB).
- Optional `.2brain/ignore` (gitignore-style patterns, UTF-8) excludes operational dotfiles (e.g. `.claude/`, `.serena/`, `*.bak-*`) from node graphs, orphan counts, and search ranking while leaving them untouched on disk.
- Excluded files are never returned through `tb_search`, `tb_grep`, `tb_files`, or `tb_context`.

### 1.3. Symlink & Decode Safety (SEM-04, SEM-18)

1. **Member confinement (SEM-18):** every resolved file read or scan must verify via canonical path resolution (`realpath`) that the target stays inside the member's root directory. Any symlink resolving outside its owning member MUST be refused with an explicit boundary error (never silently read or returned as empty).
2. **Strict UTF-8 decoding (SEM-04):** files with invalid UTF-8 byte sequences MUST fail explicitly with a decode error rather than returning lossy replacement text or an empty body.
3. **Unavailable vs Empty:** any I/O permission error or unreadable directory MUST surface as `Unavailable` / diagnostic error, never coerced into an empty list (`[]`).

---

## 2. Exact Content Addressing & Conditional Reads (D02)

Every content read returns an `EvidenceReceipt` derived from **one in-memory buffer** read from disk:

- `sha256`: lowercase 64-hex SHA-256 of the **entire file's** exact bytes.
- `hash`: 16-hex whole-file write-guard digest (FNV-1a 64-bit over full file bytes, preserved for wire compatibility with `expected_hash`).
- `byte_start`, `byte_end`, `total_bytes`, `total_chars`: half-open byte/char bounds validated against the same buffer.
- `truncated`: `true` iff returned text is a strict slice of the full file.
- `retention`: `{ "retained": false }` (historical dirty bytes are never silently fabricated).

### Conditional Hydration (`expected.sha256`)

When a caller passes `expected: { sha256 }`:
- If current full-file `sha256 == expected.sha256`: return `revision.status = "match"` with requested text slice.
- If current full-file `sha256 != expected.sha256`: return `revision.status = "stale"`, `expected_sha256`, `served_sha256`, and **omit text** (preventing mixed-revision stitching).
- Zero-length, inverted, or out-of-range windows are rejected at the boundary.

---

## 3. Markdown, Frontmatter, Wikilinks, and Block IDs

### 3.1. YAML Frontmatter

- Recognized only when file begins at byte 0 with `---\n` (or `---\r\n`) and closes on a line `---` followed by newline or EOF.
- Parsed strictly as YAML 1.2 mapping (`saphyr`). Non-mapping or malformed YAML emits a parse diagnostic and marks frontmatter invalid (never silently skipped).
- Key ordering is preserved for lossless diagnostics; body starts immediately after closing delimiter line.

### 3.2. Wikilinks (`[[...]]`)

Scanned outside fenced code blocks (` ``` ` / `~~~`) and inline code spans (`` `...` ``) using byte-level `[[` scanning:

```ebnf
wikilink    := "[[" target [ "::" repo ] [ "#" anchor ] [ ":" field ] [ "|" alias ] "]]"
anchor      := heading_slug | "^" block_id
block_id    := [A-Za-z0-9_-]+
```

- **Edge classification (KTD6):**
  1. `field` — wikilink inside a typed frontmatter field value (`topics: ["[[memory-systems]]"]`).
  2. `contributing` — body wikilink carrying `:field` (`[[target:grounds]]`).
  3. `navigational` — plain prose wikilink in the markdown body (`[[target]]`, `[[target|label]]`, `[[target#sec]]`, `[[target#^id]]`).
- **Target resolution:** stem match (case-sensitive exact path or unique filename stem within `own` + declared `deps` scope). Ambiguous stems emit a diagnostic; unresolved targets emit a `broken-link` diagnostic (never silently dropped).

### 3.3. Block IDs (`^id`) and Anchors

- A trailing `^<block_id>` at the end of a paragraph or list item (preceded by whitespace, outside code fences) defines a block anchor addressable via `[[note#^<block_id>]]`.
- ATX Markdown headings (`#` .. `######`) define heading anchors addressable via `[[note#<slug>]]`.
- `tb_anchors` enumerates all headings and block IDs of a note with exact byte/line spans and file `sha256`.

---

## 4. Tokenizer Contract (EN + RU Parity)

For BM25 search and co-word projections:
- Token = maximal contiguous run of Unicode alphanumeric characters (`char::is_alphanumeric()`), lowercased (`to_lowercase()`).
- **Parity vector:** `Error->Fix, ERROR_fix!` → `["error", "fix", "error", "fix"]` (works identically for Latin and Cyrillic).
- No destructive stemming and no silent stopword deletion in exact/BM25 retrieval (exact queries must distinguish singular/plural and domain terms).
- Per-field token positions are retained for proximity scoring (5-token window) and 240-char contextual excerpts.
