# License

Copyright © 2026 Volvelle Labs GmbH i.G.

This repository is a monorepo: each member directory carries its own `LICENSE.md`, and that file governs
that directory. Nothing here re-licenses a member.

| directory | license |
|---|---|
| `tb-engine/` (the Rust engine and the `au` binary) | Sustainable Use License 1.0 — see `tb-engine/LICENSE.md` |
| `tb-mcp-rs/` (the `2brain` MCP server) | follows `tb-engine/LICENSE.md` |
| every other `tb-*` directory | MIT — see that directory's `LICENSE.md` |
| `upstream/` | material preserved from the arsumbris project, under its original terms |

The Sustainable Use License permits use, modification and redistribution for internal, personal and
non-commercial purposes, and does not permit offering the software to others as a paid service. It is not an
OSI-approved open-source license; read `tb-engine/LICENSE.md` before redistributing.

Third-party components keep their own licenses, listed by the crates and packages that pull them in.
