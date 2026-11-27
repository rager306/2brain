# Spec v1.0 — Type System, Schema Grammar, and Diagnostics

**Status:** Normative clean-room specification (Phase 0) · **License:** MIT
**Scope:** Complete grammar of `type/*.type.yaml`, inheritance, cross-repo type references, field validation, and deterministic diagnostics.

---

## 1. Type Definition Files (`type/<name>.type.yaml`)

Every `*.type.yaml` file inside a member's `type/` directory defines one named type whose local name is the filename stem (e.g., `type/task.type.yaml` → `task`, `type/mcp.skill.type.yaml` → `mcp.skill`).

### 1.1. Supported Top-Level Keys (7 keys)

```yaml
#: Optional doc comments attached to the type or field
extends: parent_type              # optional single inheritance (local or `parent::dep_repo`)
abstract: true                    # optional; cannot be instantiated directly
sealed: true                      # optional; prevents external subtypes
meta: true                        # optional; marks a metadata facet type
location: "claims/*.md"           # optional path convention glob
body: freeform                    # optional body contract descriptor
fields:
  required_field: String
  optional_field?: String[]
```

- A field name ending with `?` is **optional**; without `?` it is **required**.
- Inheritance (`extends`): child inherits all parent fields; redefining a field narrows or overrides it. Cycles in `extends` emit a `type-cycle` error diagnostic.
- Cross-member type reference: `type_name::repo_name` is permitted **only** when `repo_name` is declared in `deps` of the owning member's `.2brain/repo.yaml`. Undeclared cross-repo type references emit an `undeclared-dependency` diagnostic.

### 1.2. Field Type Expression Grammar

Covers 100% of observed schemas across production vaults, `project-memory`, `tb-mcp-sdk`, and `tb-weave`:

```ebnf
type_expr   := base_type [ cardinality ]
cardinality := "[]"        # list of 0..N items
             | "[+]"       # non-empty list of 1..N items

base_type   := "String"
             | "Boolean"
             | "DateTime"                     # ISO-8601 or colon-free YYYY-MM-DDThhmmssZ
             | "Number" [ "{" constraint "}" ]
             | "[" enum_val ("," enum_val)* "]"
             | "type<" type_ref ">" "*"       # reference to a type definition (subtype of type_ref)
             | type_ref "*"                   # wikilink reference to an instance of type_ref
             | type_ref "&"                   # embedded / owned reference to type_ref
             | type_ref                       # embedded structured object matching type_ref's fields

type_ref    := ident [ "::" repo_ident ]
constraint  := clause ( "&" clause )*
clause      := ">=" number | ">" number | "<=" number | "<" number | "integer"
```

### 1.3. Temporal & Epistemic Standard Fields (Issue #7 R14 / D03)

Standard optional fields recognized natively by the engine when present on `claim` or domain notes (without breaking existing notes that omit them):

- `status`: enum (`current | aging | stale | superseded`, or type-defined enum)
- `valid_from` / `valid_until`: `DateTime` (valid time — when the statement holds in the world)
- `observed_at` / `created`: `DateTime` (observation time — when the statement was recorded)
- `polarity`: `asserted | negated | conditional | disputed`
- `supersedes` / `conflicts_with`: wikilink list to related claims/notes

---

## 2. Instance Validation Rules

A Markdown file with frontmatter key `type: <ref>` is validated against the resolved type definition:

1. **Unknown type:** `type` does not resolve in `own` + declared `deps` → `error` (`unknown-type`).
2. **Abstract type:** `type` has `abstract: true` → `error` (`abstract-type-instantiated`).
3. **Missing required field:** required field absent or null → `error` (`missing-required-field`).
4. **Type mismatch:** value does not match `String` / `Number` (including numeric constraints) / `Boolean` / `DateTime` / enum / array / non-empty `[+]` / nested struct fields → `error` (`invalid-field-value`).
5. **Typed link target mismatch (`T*`):** wikilink target exists but its `type` is not `T` (or subtype of `T`) → `error` (`invalid-link-target-type`).
6. **Broken wikilink:** any `[[target]]` in frontmatter or body that fails to resolve → `error` (`broken-link`).
7. **Location mismatch (`location:`):** file path does not match declared glob → `warning` / `drift` (`location-mismatch`).

---

## 3. Diagnostic Contract

Every diagnostic carries:
- `code`: stable kebab-case machine identifier
- `severity`: `error | warning | drift`
- `repo`: owning member name
- `path`: member-relative POSIX path
- `span`: optional `{ byte_start, byte_end, line, col }`
- `message`: concise English explanation

**Determinism invariant:** diagnostics are sorted canonically by `(repo, path, span.byte_start, code, message)` so repeated builds over identical bytes produce byte-identical `tb_diagnostics` outputs.

---

## 4. Graph & Health Metrics Contract

From validated files and resolved edges, the engine maintains a read-only `VaultSnapshot` answering:

- **`tb_overview`**: members, top-level dirs, type counts, diagnostic counts (`by_code`, `by_severity`, `total`), `graph_shape` (`node_count`, `edge_count`, `edges_structural`, `edges_navigational`, `components`, `largest_component`, `density`, `orphans { isolated, no_inbound }`, in/out degree histograms), and top hubs.
- **`tb_hubs`**: nodes ranked by total inbound degree (split `structural` vs `navigational`), deterministically tie-broken by path.
- **`tb_graph_health`**: directed cycles, dead ends, sources, broken links, bridges, articulation points, triangle count, global transitivity, and central nodes (capped by `limit`, exact `counts` over full scope).
- **`tb_neighborhood`**: bounded N-hop traversal (`direction: out|in|both`, optional `edge_kinds` filter, `max_nodes`, explicit `truncated` flag).
