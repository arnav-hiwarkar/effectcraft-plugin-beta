//! What EffectCraft offers plugins: provided contracts, operations (with the permission each one
//! needs for its real arguments), events and where contributions appear. Rendered to
//! `craft-extensions/docs/plugins/contracts/effectcraft.md` (`effectcraft-cli plugin contract`).

use std::cell::RefCell;
use std::sync::OnceLock;

use craft_ext_core::contract::Provided;
use craft_ext_core::id::AppId;
use craft_ext_core::permission::{DocScope, Permission};
use craft_ext_host::api::{AppContract, EventSpec, OpKind, OpSpec, SlotSpec, none};
use serde_json::Value;

pub const APP: &str = "effectcraft";

/// The contracts this build provides (B2). Keep `craft-ext-cli`'s `known_apps` table in step.
pub fn provided() -> Provided {
    Provided::new(&[
        ("raster.filter", 1, 0),
        ("effectcraft.session", 1, 0),
        ("resource.preset", 1, 0),
        ("resource.lut", 1, 0),
        ("craft-theme", 1, 0),
    ]).without(craft_ext_core::manifest::ContributionKind::Tool, "Tools are not supported in EffectCraft yet")
}

thread_local! {
    /// Layer ids selected when the current event's transaction began (set on the worker thread
    /// that runs the plugin, read by `requires`): a rename of a selected layer needs only
    /// `document.edit:selection`.
    pub(crate) static CAPTURED_SELECTION: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

fn read_selection(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentRead(DocScope::Selection)]
}

fn read_document(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentRead(DocScope::Document)]
}

fn edit_document(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentEdit(DocScope::Document)]
}

fn edit_selection(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentEdit(DocScope::Selection)]
}

fn rename(args: &Value) -> Vec<Permission> {
    let target = args.get("layer").and_then(Value::as_u64);
    let selected = target.is_some_and(|t| CAPTURED_SELECTION.with(|s| s.borrow().contains(&t)));
    vec![Permission::DocumentEdit(if selected { DocScope::Selection } else { DocScope::Document })]
}

fn window(_: &Value) -> Vec<Permission> {
    vec![Permission::UiWindow]
}

/// The EffectCraft contract (built once).
pub fn contract() -> &'static AppContract {
    static C: OnceLock<AppContract> = OnceLock::new();
    C.get_or_init(build)
}

fn op(
    id: &'static str,
    kind: OpKind,
    summary: &'static str,
    args: &'static str,
    result: &'static str,
    requires: fn(&Value) -> Vec<Permission>,
    permission_rule: &'static str,
) -> OpSpec {
    OpSpec { id, kind, summary, args, result, requires, permission_rule }
}

fn build() -> AppContract {
    #[allow(clippy::expect_used)]
    let app = AppId::new(APP).expect("`effectcraft` is a valid app id");
    AppContract {
        app,
        display_name: "EffectCraft",
        provided: provided(),
        ops: vec![
            op(
                "selection.get",
                OpKind::Query,
                "The layers selected in the active composition when this event started.",
                "{}",
                r#"{"layers": [{"id": 12, "name": "Title", "index": 1}]}"#,
                read_selection,
                "`document.read:selection`",
            ),
            op(
                "comp.info",
                OpKind::Query,
                "The active composition: id, name, size, frame rate, duration, work area and its layers.",
                "{}",
                r#"{"id": 3, "name": "Main", "width": 1920, "height": 1080, "frameRate": 30, "duration": 10, "layers": [...]}"#,
                read_document,
                "`document.read:document`",
            ),
            op(
                "layer.list",
                OpKind::Query,
                "Every layer of the active composition, top to bottom.",
                "{}",
                r#"{"layers": [{"id": 12, "name": "Title", "index": 1}]}"#,
                read_document,
                "`document.read:document`",
            ),
            op(
                "layer.rename",
                OpKind::Edit,
                "Renames a layer of the active composition (name: 1–256 characters).",
                r#"{"layer": 12, "name": "Lower third"}"#,
                "null",
                rename,
                "`document.edit:selection` when the layer was selected when the event started, else `document.edit:document`",
            ),
            op(
                "marker.add",
                OpKind::Edit,
                "Adds a composition marker to the active composition, at `time` (seconds) or the current time.",
                r#"{"label": "Note", "time": 2.5}"#,
                r#"{"time": 2.5}"#,
                edit_document,
                "`document.edit:document`",
            ),
            op(
                "effect.apply",
                OpKind::Edit,
                "Applies an effect (built-in id such as `ec.blur.gaussian`, or `<plugin id>/<effect>`) to the selected layers, then sets the given parameters.",
                r#"{"effect": "ec.blur.gaussian", "params": {"blurriness": 12}}"#,
                r#"{"effects": [41], "paths": ["effects/#2"]}"#,
                edit_selection,
                "`document.edit:selection`",
            ),
            op("time.get", OpKind::Query, "The current time of the active composition, in seconds.", "{}", r#"{"time": 1.5, "comp": 3}"#, none, "free"),
            op(
                "ui.panel.set",
                OpKind::Ui,
                "Replaces the content of one of this plugin's declared panels with a UI tree (see craft-ext-core `ui::Node`).",
                r#"{"panel": "notes", "tree": {"type": "column", "children": [{"type": "label", "text": "Hi"}]}}"#,
                r#"{"rev": 4}"#,
                none,
                "free (own panels only)",
            ),
            op(
                "ui.window.open",
                OpKind::Ui,
                "Opens (or replaces) a host-drawn floating window with a UI tree. At most 4 windows per plugin.",
                r#"{"id": "about", "title": "About", "tree": {"type": "label", "text": "Hello"}}"#,
                r#"{"rev": 1}"#,
                window,
                "`ui.window`",
            ),
            op(
                "ui.notify",
                OpKind::Ui,
                "Shows a short message (at most 300 characters), prefixed with the plugin's name.",
                r#"{"message": "Done", "error": false}"#,
                "null",
                none,
                "free",
            ),
        ],
        events: vec![
            EventSpec { name: "init", summary: "Once after the plugin loads: draw your panels.", payload: r#"{"event":"init"}"#, needs_app_events: false },
            EventSpec { name: "command", summary: "A command contribution ran (one undo step).", payload: r#"{"event":"command","id":"<contribution>"}"#, needs_app_events: false },
            EventSpec {
                name: "ui",
                summary: "A widget in your panel or window changed; stale revisions are dropped.",
                payload: r#"{"event":"ui","surface":"<panel>","rev":4,"widget":"<id>","value":null}"#,
                needs_app_events: false,
            },
            EventSpec {
                name: "app",
                summary: "`selection.changed`, `time.changed` (at most 4 per second), `document.opened`, `render.finished`.",
                payload: r#"{"event":"app","name":"selection.changed","payload":{"layers":[12]}}"#,
                needs_app_events: true,
            },
            EventSpec { name: "dispose", summary: "The plugin is being turned off or reloaded.", payload: r#"{"event":"dispose"}"#, needs_app_events: false },
        ],
        slots: vec![
            SlotSpec { kind: "effect", summary: "Effects & Presets and Effect ▸ under the contribution's category (default **Plugins**); instances are ordinary effect property groups (animatable, expressions, Effect Controls)." },
            SlotSpec { kind: "command", summary: "**Plugins ▸ <plugin name> ▸ <label>**, and the command id `plugin.<plugin id>.<contribution>` for agents and the CLI." },
            SlotSpec { kind: "panel", summary: "A host-drawn panel opened from **Plugins ▸ <plugin name> ▸ <label>** (beta: a floating panel)." },
            SlotSpec { kind: "event-hook", summary: "Receives `app` events (needs `app.events`)." },
            SlotSpec { kind: "preset", summary: "Each preset becomes a command in **Plugins ▸ <plugin name>** that applies its effects to the selected layers (one undo step); also listed by `plugin.resources`." },
            SlotSpec { kind: "lut", summary: "Listed by `plugin.resources` (absolute `.cube` paths for Apply Color LUT's file parameter)." },
            SlotSpec { kind: "theme", summary: "Stored and listed; applied once craft-theme lands (deferred)." },
        ],
        notes: "Tools are not supported in EffectCraft yet. Pixels: premultiplied RGBA f32 in the project's working space (`color` is `linear-srgb` when Linearize Working Space is on, else `srgb`); values may exceed 1 in 32 bpc projects. \
`time` is layer time in seconds. `scale` is buffer pixels per layer pixel (previews at reduced resolution have scale < 1 and `quality: \"draft\"`). \
`origin` is the buffer's top-left inside the layer's full frame and `full` the layer's size, both in buffer pixels. \
Point parameters: defaults are fractions of the layer size ([0.5, 0.5] = centre); values reach the filter in buffer pixels. Angle parameters are degrees. Select parameters are 0-based indices. \
Plugin effects are treated as time-dependent by the frame caches. Ops act on the composition and selection captured when the event started. Effect ids are `<plugin id>/<contribution>`.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contract_is_complete_and_renders() {
        let c = contract();
        for op in ["selection.get", "comp.info", "layer.list", "layer.rename", "marker.add", "effect.apply", "time.get", "ui.panel.set", "ui.window.open", "ui.notify"] {
            assert!(c.op(op).is_some(), "{op}");
        }
        let md = c.to_markdown();
        assert!(md.contains("# EffectCraft plugin contract") && md.contains("`effectcraft.session`"));
        // `EFFECTCRAFT_CONTRACT_OUT=path cargo test -p effectcraft-plugins contract` writes the reference page.
        if let Some(out) = std::env::var_os("EFFECTCRAFT_CONTRACT_OUT") {
            std::fs::write(out, &md).unwrap();
        }
    }

    #[test]
    fn rename_needs_selection_scope_only_for_captured_layers() {
        CAPTURED_SELECTION.with(|s| *s.borrow_mut() = vec![7]);
        assert_eq!(rename(&json!({"layer": 7})), vec![Permission::DocumentEdit(DocScope::Selection)]);
        assert_eq!(rename(&json!({"layer": 8})), vec![Permission::DocumentEdit(DocScope::Document)]);
        CAPTURED_SELECTION.with(|s| s.borrow_mut().clear());
    }
}
