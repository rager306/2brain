# tb-cli

The `au` binary: the per-workspace daemon and its management commands.

## Commands

| Command | What it does |
|---|---|
| `au daemon start <entry>` | boot a daemon, serve in the foreground until stopped |
| `au daemon status <entry>` | report whether a daemon is serving the entry |
| `au daemon stop <entry>` | ask a serving daemon to shut down (over the socket) |
| `au daemon stop --force <entry>` | also reclaim a booting or wedged daemon by signalling its recorded pid |
| `au open <name>` | resolve a named workspace folder, then serve it like `daemon start <folder>` |
| `au members <entry> [--json]` | resolve the entry's declared members without a daemon: role, resolution tier, path, unmounted cause |

The `<entry>` is a folder-repo: a directory carrying a valid, named identity manifest (`.2brain/repo.yaml`; `.arsumbris/repo.yaml` before migration). A non-repo directory is refused (exit 5). An optional `.2brain/workspace.yaml` beside it composes `edit` and `discover` members; without one, the workspace is the entry repo plus its own `deps`.

`au open <name>` resolves `<name>` through the per-user workspaces index, `~/.2brain/au-engine/config/workspaces.yaml` (a user-authored list of `- path: <folder>` entries, each named by an optional `name` alias or the folder's basename), to its workspace-repo folder, then enters it exactly as `au daemon start <folder>`. The index only points at folders; the committed `.2brain/workspace.yaml` is the composition. An unknown name is a usage error (exit 2) that lists the known workspaces.

`au daemon start` boots the engine, watches every member tree, and serves over a Unix socket at a short hashed path outside the knowledge base, `<state root>/au-engine/run/<hash-of-abs-entry-folder-path>.sock` (the state root is `~/.2brain` on a migrated or fresh home, `~/.arsumbris` otherwise), until stopped; a consumer spawns and owns it as a child process. The out-of-knowledge base location keeps a deep knowledge base path from overrunning the socket-path limit. One daemon per entry: a second `start` is refused while one is live, and a stale socket from an ungraceful exit is reclaimed. `stop` connects and asks it to shut down; `status` reports liveness and lifecycle.

The socket binds only after the cold build, so a daemon that is still booting (or wedged) holds its entry without answering the socket. To reach it, `start` records its pid in a `<hash>.pid` file beside the socket the instant it begins (removed on a clean stop). `status` reads it to report a booting daemon ("present but not yet serving", exit 0) rather than "no daemon running"; `stop --force` reads it to reclaim a booting or wedged daemon by signalling the pid (a graceful window, then SIGTERM, then SIGKILL). Plain `stop` stays socket-only, so it never signals a mid-build daemon.

`au members` answers, before any daemon runs, which declared members would mount and through which tier: `entry`, a co-present `sibling`, the per-user `registry`, the package `cache`, `unmounted`, or `disabled`. It runs the build's own composition with the inputs a daemon over the same entry would use, so it cannot disagree with `au daemon start`. Reads only: no write, no registry change, no fetch. An unmounted member is an answer, not a failure (exit 0); a non-repo entry exits 5. `--json` prints the contract in `crates/tb-engine/WIRE.md`, §Daemon-less member resolution.

Beyond that one topology check, the binary carries no read capabilities of its own. Consumers start a daemon and speak the wire themselves. The contract — transport, framing, the closed read and subscription catalogs, byte-offset encoding, and the schema-evolution policy — lives in `crates/tb-engine/WIRE.md`; the serde structs in `crates/tb-engine/src/wire.rs` are the schema. `examples/watch.py` is a reference external consumer.

## Tracing

`TB_TRACE=1 au daemon start <repo>` records per-operation timing.

- off by default. Unset, no subscriber runs and the engine pays nothing.
- two outputs land under `<repo>/<identity dir>/au-engine/logs/trace/`, per run, named by start time.
- the trace directory ignores itself in git (a `.gitignore` holding `*`), so traces are never committed.
  - `trace-<stamp>.json`, a Chrome/Perfetto trace, open it at [ui.perfetto.dev](https://ui.perfetto.dev) or `chrome://tracing`.
  - `timing-<stamp>.log`, one line per operation close with its wall-clock busy/idle split.
- spans cover builds, incremental recomputes, reads (by verb), and mutations, each carrying the fields that shape its cost.
- a build is broken into a span per stage, so a slow build names its slow stage rather than reading as one bar.
- the seams around a build are spanned too, the fingerprint pass, the fast-path attempt, and the commit where new state becomes visible to a subscriber.
- both flush on daemon shutdown.

To diagnose a stall, note the wall-clock time you saw it, then find the long span at that time in the trace.

If a write feels slow, look first at `try_incremental_fast_path`. Its
recorded return says whether the write took the incremental path; `false`
means it fell through to a whole-knowledge-base rebuild, which is usually
the whole explanation.

Trace files are written into the entry repo. Check they are gitignored
before running against a repo you commit from; `trace-*.json` is easy to
miss.
