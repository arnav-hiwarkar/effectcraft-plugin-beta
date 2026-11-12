# Craft plugin migration

EffectCraft uses the shared schema-2 Craft plugin platform described in
[beta-design.md](../../craft-extensions/docs/plugins/beta-design.md).
The adapter lives in `crates/plugins`; the retired in-process plugin crates and
catalogue implementation have been removed. Old packages need a schema-2
`craft-plugin.toml` and the Craft guest ABI; they are not loaded automatically.

## Implemented

- Enabled compatible effect contributions register through `effects::external`,
  render through `wasm::Filter` with the B4 context, and unregister on reload.
  Render checks refresh GrantView and check contribution enablement, required
  permissions and the kill list, including during fuel slices.
- Sessions use worker queues and document-thread request/reply. Each event has
  one undo group, with rollback on cancellation, invalid invocation, stopped plugin,
  document replacement or revocation, and grant-generation checks at commit.
  A denied, unknown or invalid operation returns its error to the plugin without
  ending the event; the plugin can redraw its panel in that same event.
  Invocation permissions belong to the invoking contribution, including init.
- The operation contract describes selection/document permissions and captured
  targets. UI revisions returned by operations are the committed revisions;
  floating windows retain their originating contribution's worker and authority.
- Plugins > Manage Plugins opens the shared ManagerView in Settings. Host-drawn
  panels/windows, revision-tagged UI events and ProjectPrompt are wired.
  Continue applies per-row skips; Open without them suppresses all referenced
  plugins for that document, including effects (saved instances stay placeholders).
  Suppression survives store reloads and clears for a new document. Download and
  Enable open the shared setup flow; opening a project never grants consent.
- `effectcraft-cli plugin …` uses shared `run_plugin`; `plugin contract`
  prints the adapter reference; `plugin project <project-file> [--json]` reads
  JSON/XML projects and prints shared readiness diagnostics using every recorded
  contribution and state version. ext-cli defaults are disabled.
- Save writes project plugin references; open computes statuses without installing
  or enabling and offers the project prompt / CLI diagnostics.
- Missing effect property groups and parameters survive save. Preview passes
  through with status; final renders refuse and identify the missing plugin.
- `--safe-mode` or `EFFECTCRAFT_SAFE_MODE=1` prevents plugin code loading.
- The layer table assigns plugins to L4 with plugins→engine and host→plugins
  edges. No old implementation identifiers remain (the new plural crate is
  `effectcraft_plugins`).

## Named TODOs and evidence limits

- **B9-FEATURE-STATE:** effect property groups currently identify the contribution
  by `<plugin>/<effect>` and keep ordinary parameters. Add per-instance version,
  contract, state_version and opaque state, with downgrade placeholders. Project
  package readiness now checks recorded contribution state versions; per-instance
  opaque state remains deferred.
- **B4-DISPOSE:** deliver a bounded dispose event before stopping workers; current
  unload unregisters effects and queues Stop, without a guest dispose callback.
- **B4-WORKER-ORDER:** wasm workers are per plugin/module. Separate modules in
  one package do not share a package-wide event ordering queue.
- **B5-WINDOW-LIMIT:** surface commit silently skips excess floating windows;
  return a budget error before acknowledging a fifth window.
- Catalogue/kill-list refresh runs for the app lifetime every six hours, with
  failure retries after 5 minutes, 30 minutes, then 2 hours. Published store
  changes reload contributions on the next pump. An unreadable grant refresh
  denies all rights and stops work until the store can be read again.
- Native workers supervise idle helpers every 500 ms, withdrawing consent
  promptly even with no queued events.
- Tool contributions are unavailable: "Tools are not supported in EffectCraft yet";
  the adapter reports that reason through shared compatibility/manager status.
- **CONFORMANCE:** runtime, hostile modules, revocation races, GUI interaction,
  native-helper shutdown, undo/redo and project round trips need execution.
  Focused plugin and shared-core tests cover operation recovery, fail-closed
  grants, project references and document suppression; no app runs were used.
- Shared beta deferrals still apply: theme activation, native effects/pixel
  transport, temporal access, tiling, TUF and process leases.

The shared contract reference is transcribed from `contract().to_markdown()`'s
format and the adapter declarations, without building or executing a generator.

## Scoped validation

Scoped Linux checks: `cargo check -p effectcraft-effects` (native and
`--target wasm32-unknown-unknown`), `effectcraft-engine`, `effectcraft-plugins`,
`effectcraft-ui-egui`, `effectcraft-host`, and `effectcraft-cli`, one at a time.
`cargo test -p effectcraft-plugins` passes all 11 tests, including the point-default
assertion and the new regression tests. Shared hooks were checked with
`cargo check -p craft-ext-core -p craft-ext-ui --tests`; `cargo test -p craft-ext-core`
passes all 42 tests. No app runs, full builds, clippy or workspace gates were run.

### Transaction, UI and rendering safety

Queued events belong to the dispatching session and document replacement epoch. The
runtime drops events after Open/New or a surface revision change. On the document
thread, every operation is authorized with freshly read grants (the worker skips
argument-based permission rules via `authorizes_on_owner_thread`); contribution
enablement and the captured release version/digest are checked before service and
commit. An aborted event discards its journal entries and edits. A recoverable operation
error leaves the event open, including the color-notes PermissionNeeded redraw. A successful event
records exactly one undo step, even when its edits exceeded the history limit.

Text inputs retain edits per surface/revision/widget and submit on Enter or focus
loss. Sliders retain their drag value and submit on release.

Every effect id absent from the built-in registry is an external effect, including
legacy ids without a slash. Missing instances retain their parameter groups as
explicit placeholders. Previews pass through with a warning; final frame saves and
exports fail with the effect/plugin name on missing, incompatible, failed or revoked
filters. Final intent is supplied by the consumer's error channel, never inferred
from image resolution. Final renders bypass processed-layer caches to recheck rights.

Project plugin references retain the recorded version and digest on save, together
with contribution ids and state schema versions. Applying a new effect records its
provenance immediately. A different loaded release cannot upgrade or render these
instances silently: install the recorded release, or explicitly migrate a copy of
the project and its saved parameters/provenance. Automatic migration is not performed.
