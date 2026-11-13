# Parity with the existing Python bridge

Measured by `scripts/bridge_parity.py` on 2026-11-13. The script builds a
disposable workspace, starts a private engine, the Node `au-mcp` daemon, the
Python bridge, and the Rust adapter, then compares what each surface states.

## The surfaces are not the same shape

| | count | what it is |
|---|---|---|
| Python bridge tools | 19 | tools the Node daemon discovered from `mcp.tool` plugins in the served workspace |
| Rust adapter tools | 16 | direct engine reads |

Byte-for-byte parity is the wrong target: the daemon **projects** engine data.
Its `au_diagnostics` tool returns a summary, paging, and a severity filter on top
of the read, while the adapter passes the engine payload through unchanged.

## What was compared, and the result

| bridge tool | adapter tool | result |
|---|---|---|
| `mcp__au_diagnostics` | `au_diagnostics` | **match**: 3 identical findings (code, severity, file, line), collected by the daemon across its severity filter and identical to the engine read |

The daemon's diagnostics tool defaults to one severity and offers no "all"
value, so the script calls it once per severity (`error`, `drift`, `warning`,
`hint`) and unions the facts. The adapter's answer equals the engine read
exactly.

## Surface differences that a cutover must decide on

- **Daemon-only** (18): `bash`, `glob`, `grep_files`, `read_file_pinned`,
  `write_file`, `edit_file`, `delete_file`, `rename`, `rename_block_id`,
  `assign_block_id`, `inline`, `promote`, `add_dep`, `remove_dep`,
  `add_workspace_member`, `remove_workspace_member`, `set_workspace_member_role`,
  `rename_type`. Several are host actions or workspace mutation, not engine
  reads. The adapter deliberately does not forward them.
- **Adapter-only** (15): `au_overview`, `au_type`, `au_types`, `au_type_tree`,
  `au_subtypes`, `au_hubs`, `au_imports`, `au_files`, `au_content`, `au_members`,
  `au_top_level_dirs`, `au_instances_of`, `au_validate_value`,
  `au_preview_mutation`, `au_reconcile`.
- The daemon compiles in **no tools**: without `au-mcp-core` mounted as a
  workspace dependency it serves an empty tool list. That is a deployment fact a
  cutover has to carry over, not an adapter gap.

## Reproduce

```sh
cd au-mcp-rs
AU_ENGINE_BIN=../au-engine/target/debug/au python3 scripts/bridge_parity.py
```

The script requires `bun` for the Node daemon, uses a throwaway `HOME`, and
touches neither the production vault nor any real socket.
