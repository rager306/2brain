# Fix spec: `engine-schema-legacy-spelling` must fire only when canonicalization changes the claim

Review record: `docs/reviews/20261119-024022-72fef655/validated-findings.json`, finding **#24**
(P2, confirmed, `public-contract`).

## Where

`tb-engine/crates/tb-engine/src/engine_schema.rs`, `legacy_spelling_diag`, line **257**; the predicate is
lines 258-266.

## Current predicate

```rust
let name_is_legacy = claim
    .name
    .0
    .strip_prefix(LEGACY_ENGINE_NAMESPACE)
    .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'));
let repo_is_legacy = claim.repo.as_deref() == Some(LEGACY_BUILTIN_ENGINE_REPO);
if !name_is_legacy && !repo_is_legacy {
    return None;
}
let canonical = canonical_query_name(&claim.authored());
```

`LEGACY_ENGINE_NAMESPACE = "au.engine"` is line 102, `LEGACY_BUILTIN_ENGINE_REPO = "au-engine"` is line 156.
`canonical_query_name` is line 187. `TypeNameClaim::authored` renders `name::repo` or `name`
(`tb-core/src/typedef.rs:89-94`).

## Why it is wrong

`name_is_legacy` tests only the written prefix, not whether canonicalization changes anything.
`au.engine.custom` matches it (`rest == ".custom"`), so the function emits the hint. But
`canonical_schema_claim` (line 219) leaves the claim unchanged: `canonical_schema_name` (line 134) states that
a name which is not one of the hardwired defs, "including a knowledge base's own `au.engine.custom` or
`tb.engine.custom`, is returned unchanged".

For that claim the emitted message reads `it reads as 'au.engine.custom'` (authored equals canonical), and the
advisory fix says "write 'au.engine.custom' instead; `au migrate identity` applies this for a whole tree" — a
rewrite of the same bytes. The migration agrees it is a no-op:
`identity_migration::a_knowledge_base_engine_named_type_keeps_its_name` (`identity_migration.rs:273-288`)
asserts `yaml("type: au.engine.custom\n") == None`, "an unqualified KB name is untouched". So the diagnostic
contradicts the migration and makes the gate permanently nonzero on a supported tree.

## Required predicate

Fire exactly when the in-memory claim differs from the authored bytes:

```rust
let authored = claim.authored();
let canonical = canonical_query_name(&authored);
if canonical == authored {
    return None;
}
```

Everything after it stays as it is: code `ENGINE_SCHEMA_LEGACY_SPELLING`, `Severity::Hint`, the authored span,
the message, and the `SuggestedFix::advisory`. `canonical` is already the value both the message and the fix
use.

Truth table, from `canonical_schema_name` (line 134) and `canonical_query_name` (line 187):

| authored claim | canonical | note |
|---|---|---|
| `au.engine.custom` | `au.engine.custom` | no |
| `au.engine.widget` | `au.engine.widget` | no |
| `tb.engine.repo` | `tb.engine.repo` | no |
| `au.engine.repo` | `tb.engine.repo` | yes |
| `au.engine.repo::au-engine` | `tb.engine.repo::tb-engine` | yes |
| `au.engine.repo::myrepo` | `tb.engine.repo::myrepo` | yes |
| `au.engine.custom::au-engine` | `au.engine.custom::tb-engine` | yes, the repo identity moves |
| `note::au-engine` | `note::tb-engine` | yes |

## Tests

Add them in `engine_schema.rs` `mod tests`. The `claim(raw)` helper is line 710, and `legacy_spelling_diag` is
`pub(crate)`, so the module can call it directly.

1. **No note for a retained custom namespace.**
   `legacy_spelling_diag(Path::new("note.md"), &claim("au.engine.custom"))` returns `None`.
   End to end: `parse_engine_schema_instance(&PathBuf::from("repo.yaml"),
   b"type: au.engine.custom\nname: r\n", "tb.engine.repo", Some(BUILTIN_ENGINE_REPO))` yields no diagnostic
   with code `ENGINE_SCHEMA_LEGACY_SPELLING`, and the claim still authors as `au.engine.custom`.
2. **The note still fires for a legacy hardwired claim.**
   `legacy_spelling_diag(..., &claim("au.engine.repo::au-engine"))` is `Some`, with
   `code == ENGINE_SCHEMA_LEGACY_SPELLING`, `severity == Severity::Hint`, and a fix whose description contains
   `tb.engine.repo::tb-engine`. `the_instance_wrapper_canonicalizes_the_type_claim` (line 823) already asserts
   this through the parser; keep it green.
3. Keep `a_knowledge_base_type_keeps_its_own_identity` (line 782) green. It pins `canonical_schema_claim`, the
   function whose identity is the reason for this predicate.
4. Optional table-driven test over the truth table above, which also covers the repo-qualifier-only move
   (`au.engine.custom::au-engine` must still earn a note, because its bytes are stale).

## Do not change

- `canonical_schema_name`, `canonical_schema_claim`, or the constants. This is a predicate fix in one function.
- `engine_name_reservation_diag` (line 338) still emits `engine-name-forward-reserved` for a type-def named
  `au.engine.custom`. That is a different, intended note about the reserved namespace, not a stale-spelling
  claim.
