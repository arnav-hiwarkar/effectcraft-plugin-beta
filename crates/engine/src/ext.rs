//! The engine side of the Craft plugin runtime (`effectcraft-plugins` implements it).
//!
//! The engine knows nothing about packages, grants or wasm: it offers
//! - [`PluginRuntime`], which the host installs in [`Session::plugins`]: plugin commands
//!   (`plugin.<plugin id>.<contribution>`) dispatch to it, frontends call [`Session::pump_plugins`]
//!   every frame, panels and windows are read from it, and project save/open ask it for the
//!   project's plugin list and statuses;
//! - [`GroupMark`]: one plugin event = one labelled undo step, rolled back on failure (the same
//!   fold `engine.batch` uses);
//! - [`missing_rendering_effects`]: final renders refuse while an effect from a missing plugin is
//!   in the project (its instance is kept byte for byte and previews pass through).

use serde_json::{Value, json};
use std::sync::Arc;

use crate::{CommandSpec, EngineError, Event, Result, Session, cmd};
use effectcraft_project::{GroupKind, Project};

/// A command a plugin contributes (`plugin.<plugin id>.<contribution>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCommand {
    pub id: String,
    pub label: String,
    /// The plugin's display name: the command sits in Plugins ▸ `plugin` ▸ `label`.
    pub plugin: String,
    pub category: Option<String>,
}

/// A host-drawn plugin panel or window (`ui.panel.set`, `ui.window.open`).
#[derive(Clone, Debug, PartialEq)]
pub struct PluginSurface {
    /// `<plugin id>/<panel or window id>`.
    pub key: String,
    pub title: String,
    /// The validated `craft_ext_core::ui::Node` tree as JSON; `None` until the plugin draws.
    pub tree: Option<Value>,
    /// Tree revision: widget events carry it back so stale events are dropped.
    pub rev: u64,
    /// A floating window (`ui.window.open`) rather than a panel.
    pub window: bool,
    /// Shown now (panels open from Plugins ▸ <plugin>; windows are open until closed).
    pub open: bool,
}

/// What a host that runs Craft plugins provides. Every method runs on the thread that owns the
/// session; plugin code itself runs on workers and reaches the document only through
/// [`PluginRuntime::pump`].
pub trait PluginRuntime: Send + Sync {
    /// Commands of enabled contributions.
    fn commands(&self) -> Vec<PluginCommand>;
    /// Runs a plugin command (one transaction, one undo step).
    fn run_command(&self, s: &mut Session, id: &str, params: &Value) -> Result<Value>;
    /// Serves pending plugin requests (document ops, UI updates) and delivers app events.
    fn pump(&self, s: &mut Session);
    /// Panels and open windows.
    fn surfaces(&self) -> Vec<PluginSurface>;
    /// A widget in a plugin panel or window changed (`value` is `null` for a click).
    fn ui_event(&self, s: &mut Session, key: &str, rev: u64, widget: &str, value: Value);
    /// Shows or hides a panel; hiding a window closes it.
    fn set_open(&self, key: &str, open: bool);
    /// The plugin list to save with `project` (keeps entries of plugins that are missing here).
    fn project_plugins(&self, project: &Project) -> Vec<effectcraft_project::PluginRef>;
    /// A project was opened: check its plugins and tell the person what is missing.
    fn project_opened(&self, s: &mut Session);
    /// `plugin.list`: what is loaded, for agents and diagnostics.
    fn describe(&self) -> Value;
    /// `plugin.resources`: presets and LUT files contributed by enabled plugins.
    fn resources(&self) -> Value;
}

impl Session {
    /// Distinguishes sessions even when their project replacement epochs happen to match.
    pub fn plugin_document_token(&self) -> Arc<()> {
        self.plugin_document_token.clone()
    }
    /// Frontends call this every frame: serves plugin requests and delivers app events.
    pub fn pump_plugins(&mut self) {
        if let Some(rt) = self.plugins.clone() {
            rt.pump(self);
        }
    }

    /// The plugin runtime when `id` is a plugin command it knows.
    pub(crate) fn plugin_command(&self, id: &str) -> Option<Arc<dyn PluginRuntime>> {
        if !id.starts_with("plugin.") || crate::commands::find(id).is_some() {
            return None;
        }
        self.plugins.clone().filter(|rt| rt.commands().iter().any(|c| c.id == id))
    }

    /// Starts a group of edits that becomes one undo step (see [`Session::finish_group`]).
    pub fn begin_group(&mut self) -> GroupMark {
        GroupMark {
            journal: std::mem::take(&mut self.journal),
            project: self.project.clone(),
            history: self.history.clone(),
            state: self.state.clone(),
            events: self.events.len(),
        }
    }

    /// Ends a group: `commit` folds the edits made since `mark` into one undo step named `label`;
    /// otherwise the project, history and editor state return to `mark` and the events the group
    /// published are dropped.
    pub fn finish_group(&mut self, mark: GroupMark, label: &str, commit: bool) {
        let pending = std::mem::replace(&mut self.journal, mark.journal);
        if commit {
            self.journal.extend(pending);
            if self.journal.len() > 10_000 {
                self.journal.drain(..self.journal.len() - 10_000);
            }
            self.history = mark.history;
            if !Arc::ptr_eq(&self.project, &mark.project) {
                self.history.record(label, mark.project, self.prefs.general.undo_levels.max(1) as usize);
            }
            return;
        }
        self.history = mark.history.clone();
        if std::sync::Arc::ptr_eq(&self.project, &mark.project) {
            self.state = mark.state;
            self.events.truncate(mark.events);
            return;
        }
        let edited = crate::commands::text_edit::active_editor_snapshot(self);
        let interim_revision = self.ime_revision;
        self.project = mark.project;
        self.history = mark.history;
        self.state = mark.state;
        self.events.truncate(mark.events);
        self.sanitize_after_replacement(edited.as_ref());
        self.ime_revision = self.ime_revision.max(interim_revision);
        self.bump();
    }
}

/// Where a plugin event's undo group started ([`Session::begin_group`]).
pub struct GroupMark {
    journal: Vec<(String, Value)>,
    project: Arc<Project>,
    history: crate::History,
    state: crate::EditorState,
    events: usize,
}

/// Every id absent from the built-in registry is external, including legacy plugin ids.
pub fn is_plugin_effect(id: &str) -> bool {
    !effectcraft_effects::registry().iter().any(|s| s.id == id)
}

/// Plugin effects used in `project` whose plugin is not loaded (missing, off or kill-listed):
/// `(effect id, plugin id)`, sorted and unique.
pub fn missing_rendering_effects(project: &Project) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (_, c) in project.comps() {
        for l in &c.layers {
            for g in l.effects().into_iter().flat_map(|fx| fx.groups()) {
                if let GroupKind::Effect { effect } = &g.kind
                    && is_plugin_effect(effect)
                    && (effectcraft_effects::find(effect).is_none() || effectcraft_effects::external::incompatible(effect, project).is_some())
                {
                    let plugin = effect.split('/').next().unwrap_or(effect).to_string();
                    out.push((effect.clone(), plugin));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The final-render refusal for [`missing_rendering_effects`], naming the install command.
pub fn missing_effects_error(project: &Project) -> Option<String> {
    for (_, comp) in project.comps() {
        for layer in &comp.layers {
            for group in layer.effects().into_iter().flat_map(|fx| fx.groups()) {
                for prop in group.props() {
                    if let effectcraft_project::Value::Str(reference) = &prop.value
                        && reference.starts_with("plugin://")
                        && let Err(reason) = effectcraft_effects::resolve_resource(reference)
                    {
                        return Some(reason);
                    }
                }
            }
        }
    }
    let missing = missing_rendering_effects(project);
    if missing.is_empty() {
        return None;
    }
    let mut plugins: Vec<&str> = missing.iter().map(|(_, p)| p.as_str()).collect();
    plugins.dedup();
    let effects: Vec<&str> = missing.iter().map(|(e, _)| e.as_str()).collect();
    let install: Vec<String> = plugins.iter().map(|p| format!("effectcraft-cli plugin install {p}` then `effectcraft-cli plugin setup {p} --all")).collect();
    Some(format!(
        "this project uses effects from plugins that are not installed, are turned off, or were stopped (withdrawn, kill-listed or changed) ({}); the render would differ, so it was not started. Install and enable them (Settings ▸ Plugins, or `{}`); `effectcraft-cli plugin project <file>` explains each one",
        effects.join(", "),
        install.join("`; `")
    ))
}

fn runtime(s: &Session, c: &str) -> Result<Arc<dyn PluginRuntime>> {
    s.plugins.clone().ok_or_else(|| EngineError::Other(format!("{c}: plugins are not available in this build")))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(match s.plugins.clone() {
        Some(rt) => {
            let commands: Vec<Value> = rt.commands().into_iter().map(|c| json!({"id": c.id, "label": c.label, "plugin": c.plugin})).collect();
            let surfaces: Vec<Value> = rt
                .surfaces()
                .into_iter()
                .map(|p| json!({"key": p.key, "title": p.title, "window": p.window, "open": p.open, "rev": p.rev, "tree": p.tree}))
                .collect();
            json!({"available": true, "loaded": rt.describe(), "commands": commands, "surfaces": surfaces})
        }
        None => json!({"available": false}),
    })
}

fn resources(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(runtime(s, "plugin.resources")?.resources())
}

fn missing(s: &mut Session, _: &Value) -> Result<Value> {
    let m: Vec<Value> = missing_rendering_effects(&s.project).into_iter().map(|(e, p)| json!({"effect": e, "plugin": p})).collect();
    Ok(json!({"missing": m}))
}

fn show_panel(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "plugin.showPanel";
    let rt = runtime(s, c)?;
    let key = p.get("surface").and_then(Value::as_str).ok_or_else(|| crate::commands::bad(c, "missing `surface`"))?;
    if !rt.surfaces().iter().any(|x| x.key == key) {
        return Err(crate::commands::bad(c, format!("no plugin panel `{key}`")));
    }
    rt.set_open(key, p.get("open").and_then(Value::as_bool).unwrap_or(true));
    Ok(Value::Null)
}

fn ui_event(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "plugin.uiEvent";
    let rt = runtime(s, c)?;
    let key = p.get("surface").and_then(Value::as_str).ok_or_else(|| crate::commands::bad(c, "missing `surface`"))?.to_string();
    let widget = p.get("widget").and_then(Value::as_str).ok_or_else(|| crate::commands::bad(c, "missing `widget`"))?.to_string();
    let rev = p.get("rev").and_then(Value::as_u64).ok_or_else(|| crate::commands::bad(c, "missing `rev`"))?;
    rt.ui_event(s, &key, rev, &widget, p.get("value").cloned().unwrap_or(Value::Null));
    Ok(Value::Null)
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        crate::query!(
            "plugin.list",
            "List Plugins",
            "{} → {available, loaded: [{id, version, name, contributions}], commands: [{id, label, plugin}], surfaces: [{key, title, window, rev}]}",
            list
        ),
        crate::query!(
            "plugin.resources",
            "Plugin Resources",
            "{} → {presets: [{plugin, contribution, id, label, effects}], luts: [{plugin, contribution, label, path}]} — data contributed by enabled plugins",
            resources
        ),
        crate::query!(
            "plugin.missing",
            "Missing Plugin Effects",
            "{} → {missing: [{effect, plugin}]} — effects from plugins that are not loaded (final renders refuse while any remain)",
            missing
        ),
        cmd!(
            "plugin.showPanel",
            "Show Plugin Panel",
            [],
            None,
            "{surface: \"<plugin id>/<panel>\", open?: bool (default true)}",
            crate::commands::always,
            show_panel
        ),
        cmd!(
            "plugin.uiEvent",
            "Plugin Widget Event",
            [],
            None,
            "{surface: \"<plugin id>/<panel>\", rev, widget, value?} — what a host-drawn plugin panel sends when a widget changes",
            crate::commands::always,
            ui_event
        ),
    ]
}

/// Tells the frontend something about plugins (a toast).
pub fn notify(s: &mut Session, message: impl Into<String>, error: bool) {
    s.events.push(Event::Toast { message: message.into(), error });
}
