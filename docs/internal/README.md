# `docs/internal/` — operations, not design

Everything here describes **this host**: its paths, its systemd units, its clients, its swap windows, and the
run logs of operations that actually happened. It is committed and reviewed like any other doc, but it is not
part of the public story of the project, and it is where a fork or a second machine would diverge first.

The rule that decides what belongs here:

| question | home |
|---|---|
| Does it describe *why* the system is built this way, in terms that survive a different machine? | `docs/` or `docs/research/` |
| Does it name `/root/vault`, a systemd unit, a socket path, the Hermes config, a backup directory, or the run log of one operator's evening? | **`docs/internal/`** |

| file | what it records |
|---|---|
| `hermes-2brain.md` | how the Hermes client is wired to the `2brain` server on this host |
| `hermes-brief.md` | the standing brief given to Hermes sessions about this vault |
| `hermes-switch.md` | the switch from the previous MCP bridge to the Rust server |
| `engine-swap-2026-11-14.md`, `engine-swap-2026-11-16.md` | swap runbooks: preconditions, artifacts, rollback, the measured result |
| `deploy-2026-11-16-batch.md` | the pending deploy batch: what a client sees, the swap steps, rollback; reconciled 2026-11-17, so the pending set is recomputed before a window |
| `detach-and-consolidate.md` | the git operations that turned fourteen repositories into one |
| `serving-parity.md` | the audit of what the retired Node/Python path served versus the Rust one |
| `w4.1-judgement-run-2026-11-16.md` | the run log of one judgement-layer evaluation |

Design documents stay outside: `docs/architecture-plan.md`, `docs/trajectory.md`, `docs/naming.md`,
`docs/research/*`, and every ADR-shaped record, because their claims are about the system rather than about a
machine.

A public reader can still follow the reasoning: every internal document says which design document it serves,
and the design documents do not depend on an internal one to be understood.
