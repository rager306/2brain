# Changelog

Notable changes to `@arsumbris/au-mcp`, newest first.

Release markers (`## 0.0.2-alpha`) group the changes that shipped in that arsumbris release.
Changes above the newest marker are not released yet.

## 0.0.2-alpha

mcp wire stays **0**, plugin contract stays **0**.

### Added

- The broker dispatches the five engine composition verbs: `add_dep`, `remove_dep`,
  `add_workspace_member`, `remove_workspace_member`, `set_workspace_member_role`.

### Changed

- The capabilities response no longer carries `redirects` (it was always empty).
- A mediator's action preview maps the engine's `rename`, `move_dir` and `delete_dir`
  preview ops to their wire args.

## 0.0.1-alpha

The first public release.
