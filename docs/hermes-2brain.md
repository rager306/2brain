# Pointing Hermes at 2brain

The Rust adapter is the serving path; the Node daemon and the Python bridge are retired
(`docs/detach-and-consolidate.md`). This is the configuration that replaces them.

## 1. Install the binary

```sh
/root/arsumbris/au-mcp-rs/scripts/install.sh
# installed /root/.local/bin/2brain
# 2brain 0.1.0+<commit>
# <sha256>  /root/.local/bin/2brain
```

`~/.local/bin` is on `PATH` here, and the path is outside the build tree on purpose: a
`cargo clean` or a rebuilt release profile must not be able to leave the configured command
pointing at nothing. That is exactly what happened to `au-mcp-rs/target/release/au-mcp-rs` on
2026-11-16 — the config was right, the file was gone.

## 2. The stanza

```yaml
  mcp_servers:
    2brain:
      command: /root/.local/bin/2brain
      args: ["mcp", "/root/vault", "--allow-mutations"]
```

`mcp` takes a **target**, not a socket: a path ending in `.sock` is used as given, anything else
is a workspace entry whose socket is derived exactly as `au daemon start` derives it
(`au_engine::socket_path`: FNV-1a of the canonical entry path, `~/.arsumbris/au-engine/run/<hash>.sock`).
So the configuration carries no hash and survives any change of the entry path's spelling, as long
as it still canonicalizes to the same directory.

Grants, and what each adds to the tool list:

| args | tools |
|---|---:|
| `--allow-mutations` | 47 |
| `--allow-mutations --allow-composition` | 49 (`au_add_dep`, `au_remove_dep`) |
| `--allow-mutations --allow-composition --allow-workspace-shape` | 52 (`au_add_workspace_member`, `au_remove_workspace_member`, `au_set_workspace_member_role`) |

Composition and workspace shape stay **off** in this configuration: changing dependencies or
workspace membership changes the shape of the knowledge base, and `policy.rs` asks for an explicit
grant. The Node path exposed those five tools ungated, which was a defect, not a feature
(`docs/serving-parity.md` §2b).

## 3. What the client sees

- server name `2brain`, version `0.1.0+<commit>` — the commit is in the version string, so a client
  log identifies the build;
- 47 tools with `--allow-mutations`; `au_search` included;
- resources under `arsumbris://<member>/<path>` — unchanged: the URI scheme is a contract and moves
  only with a versioned alias (`docs/naming.md`, phase N4).

## 4. Verify before trusting it

```sh
/root/.local/bin/2brain --version
/root/.local/bin/2brain info /root/vault          # surface + a probe read
/root/.allspark-home/.prime/agent/kernel-venv/bin/python \
  /root/arsumbris/au-mcp-rs/scripts/strict-client.py   # validates a mutation answer client-side
```

Then one session that does a read and a write. A read proves the socket resolution; the write
proves the gate and the answer schema, which is where the 2026-11-15 failure lived.

## 5. Rollback

Revert the stanza. Nothing is deleted before the old path is proven unused: `arsumbris-mcp.service`
and `au-mcp-bridge/bridge.py` stay in place until the new stanza has served a session, and
`mcp_servers.arsumbris` is removed only after that (`docs/serving-parity.md` §3).

## 6. One id, not two

Do not configure `2brain` and `arsumbris-rs` in the same client: both would register the same tools
under different prefixes, and the model would see two names for one capability. The switch is a
replacement, not an addition.
