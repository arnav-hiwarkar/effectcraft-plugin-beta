# Plugin takeover validation — 2026-11-13

Base: `ca706f0a` on `plugin-beta`, Linux x86_64. The takeover recovered uncommitted review fixes; no release workflows were enabled.

## Repairs

- Preserve an idle plugin tool while panels and unrelated commands are used.
- Cancel a resting gesture on document replacement or permission-generation changes.
- Keep drag updates and release delivery when Space is pressed mid-gesture.
- Remove project/footage paths and string property values from document queries.
- Exercise three traps, pause, Resume and worker re-instantiation with a synthetic Wasm guest.

The original session expansion is documented in [plugin-migration.md](plugin-migration.md). File-picker import/export operations remain unavailable; no unrestricted file operation was added.

## Executed checks

All Cargo checks use `CARGO_BUILD_JOBS=4 flock $ROOT/.cargo.lock`.

| Command | Result |
|---|---|
| `cargo test -p effectcraft-plugins` | PASS: 22 tests, zero failures or ignored tests; doc tests empty |
| `git diff --check` | PASS |
| `cargo test -p effectcraft-ui-egui plugin_tool_gesture_ends_even_if_space_is_pressed_mid_drag` | PASS: 1 test; other tests filtered |

## Remaining gates

Current app/CLI rebuild, real-example smoke rerun, desktop interaction and macOS/Windows checks are pending. The tests above prove adapter regressions, not GUI behavior or all sandbox protections. GitHub Actions and releases await user permission after final verification.

## Source-only follow-up

Deeply nested document-property redaction now fails closed by replacing over-depth branches with null. A regression test was added, but the follow-up suite has no confirmed result after the tool outage. The 22-test pass above predates this small follow-up. Future builds and broad suites are deferred; use source review and only cheap scoped checks.
