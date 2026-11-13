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
- **B4-DISPOSE (deferred):** unload unregisters effects and stops workers immediately,
  without calling guest code. Revocation must not wait for guest cleanup. The
  reserved dispose event is not a beta delivery guarantee.
- **B4-WORKER-ORDER (specified):** workers serialize events per plugin/module; the
  document thread serializes transactions. Separate modules have no package-wide
  ordering or shared guest state guarantee. Authors must not depend on one.
- **B5-WINDOW-LIMIT (source fixed):** window calls count existing and staged unique
  windows and return a budget error before acknowledging a fifth. Replacing a
  window keeps its slot. The commit guard remains defensive. Runtime unverified.
- Catalogue/kill-list refresh runs for the app lifetime every six hours, with
  failure retries after 5 minutes, 30 minutes, then 2 hours. Published store
  changes reload contributions on the next pump. An unreadable grant refresh
  denies all rights and stops work until the store can be read again.
- Native workers supervise idle helpers every 500 ms, withdrawing consent
  promptly even with no queued events.
- Tool contributions are supported by session contract 1.1 (see below).
- **CONFORMANCE:** desktop interaction, hostile-module coverage and native-helper
  shutdown still need broader execution. Adapter tests cover rollback, scopes,
  malformed trees, grants, project references and suppression. The session smoke
  script exercises installed Wasm panels/tools/effects, save/reopen, undo/cancel
  and frame exports (see plugin-testing.md).
- Shared beta deferrals still apply: theme activation, native effects/pixel
  transport, temporal access, tiling, TUF and process leases.

The shared contract reference is generated from `contract().to_markdown()`.

## Scoped validation

Scoped Linux checks: `cargo check -p effectcraft-effects` (native and
`--target wasm32-unknown-unknown`), `effectcraft-engine`, `effectcraft-plugins`,
`effectcraft-ui-egui`, `effectcraft-host`, and `effectcraft-cli`, one at a time.
`cargo test -p effectcraft-plugins` passes all 13 tests, including declared point spaces, typed form values, atomic
patch rejection, and a reset patch sharing one undo step with another value edit. Shared hooks were checked with
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

## Beta runs (2026-11-13)

The CLI flows in [plugin-testing.md](plugin-testing.md) were run against the local signed test
catalogue: refresh/search/install/setup/enable; halftone applied, rendered, exported, edited,
saved and reopened; disable/remove/kill-list refusals and reinstall; color-notes command and panel
event with the optional permission denied (recoverable) and granted (one undo step per event);
film-looks presets and LUT, update 1.0.0 → 1.1.0 and rollback; `plugin contract` against the
shared contract page. GUI interaction, revocation races and hostile modules remain unrun.

## Form schema v1

Contribution controls now come from `contribution.form` (or the validated optional
`craft_describe` export). Parameters use `key`, `label` Words and a nested `kind`.
Range, int and number controls use sliders; bool uses a checkbox; choice/radio use
popups whose stored form values are option IDs. Units appear beside labels and
sections appear as headings. EffectCraft keeps popup indices internally and maps
these to IDs at the plugin boundary. Colors cross that boundary as RGBA, angles as
degrees, and points as declared image percentages or full layer pixels, independent
of buffer scale and origin. Layer-pixel defaults are never multiplied by layer size.

Effect Controls evaluates visible/enabled conditions from current property values.
A property change calls the optional budgeted `craft_update`, validates its patch
against the form and revision, and groups value edits into one undo step. Hide/show
and enable/disable are transient instance UI state; parameter data remains intact.
Panels render Form nodes with conditional widgets and retained editing state;
unknown Component nodes render their fallback. Preset effect entries now use
`{ "effect": "…", "values": { "key": value } }`, with typed values and choice IDs.
No migration or compatibility path for the removed contribution params is provided.

## Session contract 1.1

The app keeps one shared `craft_ext_ui::plugin_ui::PluginUi` for panels,
windows and effect forms. Settings ▸ Plugins chooses the host engine (Egui by
default; Framework currently falls back to Egui). Plugins cannot choose it.
Native effect property rows remain available for animation and expressions.

Tool contributions select from Plugins. Primary pointer gestures send begin,
update, commit and cancel in composition pixels, with the current time and
modifiers. Escape cancels. The gesture captures composition/selection and grant
generation, folds edits into one undo entry, and rolls back on failure or
revocation. Middle-button and Space pan and wheel navigation retain the viewer's
host implementation. `plugin.toolEvent` drives the same runtime from the CLI.
Paused wasm workers appear with Resume in Settings ▸ Plugins.
`plugin.resume {id}` restarts that package's session workers and sends init.
`effectcraft-cli plugin resume <id>` explains that pauses are session-local.

The generated contract lists the new operations and permission rules. Document
grants never authorize arbitrary file access: property/effect writes accept only
numeric, boolean and numeric-vector values. File-valued properties are excluded
until picked-file tokens are implemented.

Capability audit: layers (list, rename, solid/null/text/shape creation, switches, select and property tree),
timeline (time query/set), keyframes (property read/add), markers
(list/add/set/delete), effects (apply/remove/reorder), composition creation/settings, existing assets as layers and
project/asset metadata are exposed. Asset import and render/export from a plugin
remain unavailable: they need a host-picked-file API before they can safely
accept destinations/sources. They remain available to the person through the
app and CLI. No raw OS, process or shell operation is exposed.
