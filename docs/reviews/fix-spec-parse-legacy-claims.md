# Fix spec: type-def legacy claims must earn `engine-schema-legacy-spelling` too

Review record: `docs/reviews/20261119-024022-72fef655/validated-findings.json`, finding **#25**
(P2, confirmed, `public-contract`).

## Where

- `tb-engine/crates/tb-engine/src/parse.rs:253-255` in `parse_type_def_file` (starts at line 199)
- `tb-engine/crates/tb-engine/src/engine_schema.rs:287-292`, `canonical_schema_type_def`

## Current behaviour

```rust
// parse.rs:253-255
if let Some(td) = res.type_def.as_mut() {
    crate::engine_schema::canonical_schema_type_def(td);
}
```

```rust
// engine_schema.rs:287-292
pub(crate) fn canonical_schema_type_def(td: &mut TypeDef) {
    td.name = TypeName(canonical_schema_name(&td.name.0).to_string());
    canonicalize_claims(&mut td.parents);
    canonicalize_claims(&mut td.sealed);
    canonicalize_claims(&mut td.required_meta);
}
```

The function returns `()` and rewrites the claims in place. Nothing reaches
`FileParse::TypeDef { diagnostics }`, so a type-def that writes `extends: au.engine.meta::au-engine`, a legacy
`sealed:` entry, or a legacy `meta: - required:` entry is fixed in memory and silent on disk.

The instance path has the opposite shape: `canonical_schema_instance` (engine_schema.rs:300-320) calls
`legacy_spelling_diag` per claim and returns `Vec<Diagnostic>`; parse.rs pushes that vector at lines 483 and
603. The same authored spelling is therefore reported on an instance `type:` claim and not on a type-def
claim.

## Why it matters

- `identity_migration::every_engine_schema_file_kind_moves` (`identity_migration.rs:244-271`) asserts the
  migration rewrites `extends: au.engine.meta::au-engine` to `tb.engine.meta::tb-engine`. The bytes can
  therefore be stale in exactly the files the note is meant to find.
- `ENGINE_SCHEMA_LEGACY_SPELLING`'s doc comment (engine_schema.rs:240-253) says the note exists so "a migration
  pass reads the vault, takes every file this fires on, and rewrites exactly those". With type-defs silent,
  zero hints cannot establish that the compatibility can be removed.
- `docs/research/release-2-window-2026-11-17.md:9` advertises the hint on "any written claim that spells
  `au.engine.*` or `::au-engine`". A type-def claim is a written claim.
- No scenario input writes a legacy type-def claim: scanning `tb-engine/scenarios/**/*.yaml` for
  `extends:`/`sealed:`/`required:` lines containing `au.engine` finds 0 files, so the scenario snapshots
  should not move.
- Two in-crate fixtures do write one and will now carry the hint: `build.rs:2959-2962` (`meta_conflict_fs`)
  and `build.rs:3054-3056` (`IN_SYNC`, `DIVERGED`). Their tests filter diagnostics by code
  (`duplicates`, line 2996; `conflicts`, line 3048), so they are unaffected; do not add diagnostic-count
  assertions to them.

## Required behaviour

Make `canonical_schema_type_def` return `Vec<Diagnostic>`:

1. For each claim in `td.parents`, then `td.sealed`, then `td.required_meta`, call
   `legacy_spelling_diag(&td.source_path, claim)` **before** canonicalizing, and keep the `Some`s. Each note
   keeps the authored claim span.
2. Canonicalize the three vectors as today.
3. Return the notes.
4. Emit no note for `td.name`: it is path-derived, never written, and `canonical_schema_name` is the identity
   on it.

parse.rs:253-255 becomes:

```rust
if let Some(td) = res.type_def.as_mut() {
    diagnostics.extend(crate::engine_schema::canonical_schema_type_def(td));
}
```

This is the order the instance path already uses: schema notes first, then `res.diagnostics`.

## Tests

1. `the_type_def_wrapper_canonicalizes_the_name_and_every_claim` (engine_schema.rs:801) parses
   `extends: au.engine.meta::au-engine\nfields:\n  x: String\n` and currently ignores diagnostics via `..`.
   Extend it: exactly one diagnostic, code `ENGINE_SCHEMA_LEGACY_SPELLING`, severity `Severity::Hint`, span
   equal to the authored `extends` claim's span, and `td.parents[0].authored() == "tb.engine.meta::tb-engine"`.
2. A legacy `sealed:` entry. The flow form is the one covered by `sealed_list_extracts_entries`
   (`tb-core/src/typedef.rs:2543-2549`): `sealed: [au.engine.repo::au-engine]\nfields:\n  x: String\n` yields
   one `engine-schema-legacy-spelling` note and `td.sealed[0].authored() == "tb.engine.repo::tb-engine"`.
3. A legacy required-meta claim. The single-name form is covered by `required_meta_single_name_parses`
   (`tb-core/src/typedef.rs:1992-2003`). A def whose source is `meta:\n  - required: au.engine.meta::au-engine\n`
   yields one note and `td.required_meta[0].authored() == "tb.engine.meta::tb-engine"`.
4. A canonical type-def (`extends: tb.engine.meta::tb-engine`) yields no `engine-schema-legacy-spelling`.
5. After the finding-#24 fix, `extends: au.engine.custom` yields no such diagnostic and the claim is kept.

Add the tests in `engine_schema.rs` `mod tests` next to `the_type_def_wrapper...`, or as integration coverage
in `tb-engine/crates/tb-engine/tests/it/engine_schema.rs` (`an_engine_schema_claim_resolves_in_either_spelling`,
line 477, is the existing window test).

## Companion decision (do not skip)

The advisory fix says "`au migrate identity` applies this for a whole tree", but
`identity_migration::is_claim_key` (`identity_migration.rs:197-202`) rewrites only `type:` and `extends:`.
`sealed:` and `required:` are not claim keys there, and the block-list splice only follows an open
`type:`/`extends:` key (`splice_line`'s dash branch, lines 157-173); a `- required: <claim>` meta item is a
list item, not an open claim key.

Once the note fires for sealed and required-meta claims, its suggested fix is false for them unless one of
these is done in the same change:

- extend `is_claim_key` and the splice to `sealed:` and the `required:` item form, or
- omit the migration sentence for claims `au migrate identity` cannot rewrite.

Otherwise the diagnostic tells an operator to run a command that leaves those bytes alone, which is the same
defect finding #24 fixes on the instance side.
