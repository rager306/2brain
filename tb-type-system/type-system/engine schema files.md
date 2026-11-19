The hardwired `tb.engine.*` type-defs, and the engine's own files typed as their instances.

See [[cross-repo file set]] for the files themselves.

# Properties

## Hardwired defs, a builtin repo
The `tb.engine.*` type-defs are compiled into the engine, not authored in a knowledge base.
- owned by a builtin `tb-engine` repo, present in every build.
- `::tb-engine` resolves as a universal peer, no `deps` declaration needed.
- each carries a `(name, closure-hash)` identity like any def, so it is wire-serializable and drift-comparable.
- the set is the file types below, plus the record element types their list fields reference, e.g. `tb.engine.dep`.

## The typed files
Each engine-owned file is a typed instance of one hardwired def.
- assigned by kind, no `type:` key is written, the file stays clean.
- the file-to-type mapping is in the Structure below.

## Two validation surfaces
- an in-repo file, `repo.yaml`, `workspace.yaml`, the locks, is a first-class node.
  - queryable via `instances_of`, validated against its def like any instance.
- a device-global file, `repos.yaml`, `workspaces.yaml`, sits outside every knowledge base.
  - field-shape validated on the `device_config` read, its diagnostics land there.

## The namespace is reserved by convention
`tb.engine.*` is reserved, coexistence not annihilation.
- a repo def named in the namespace keeps its own `(name, hash)` identity and resolves normally, `::tb-engine` scopes the engine's.
- a repo def named `tb.engine.foo` the engine does not hardwire is [[spec - diagnostic codes^engine-name-forward-reserved]].
- a repo def named like a hardwired type is [[spec - diagnostic codes^engine-name-live-shadow]], both coexist.
- such a shadow diverged from the engine's copy adds [[spec - diagnostic codes^engine-name-shadow-drift]].
- each is advisory, never a block.

# Structure
The engine-owned files and their assigned types.
- `<repo>/.2brain/repo.yaml`, [[repo yaml]], `tb.engine.repo`.
- `<repo>/.2brain/repo.lock`, `tb.engine.repo-lock`.
- `<repo>/.2brain/workspace.yaml`, the [[workspace manifest]], `tb.engine.workspace`, `edit?: String[]` plus `discover?: String[]`.
- `<repo>/.2brain/workspace.lock`, `tb.engine.workspace-lock`, the pinned discover closure, parallel to `tb.engine.repo-lock`.
- `~/.2brain/engine/config/repos.yaml`, [[repos yaml]], `tb.engine.repos`.
- `~/.2brain/engine/config/workspaces.yaml`, [[workspaces yaml]], `tb.engine.workspaces`.
