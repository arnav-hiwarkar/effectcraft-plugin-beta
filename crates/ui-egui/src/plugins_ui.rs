//! Craft plugins in the desktop UI: the host-drawn plugin panels and windows (from
//! `craft_ext_core::ui::Node` trees), Settings ▸ Plugins (the shared `craft_ext_ui::ManagerView`)
//! and the "This project uses N plugins" prompt. Plugin code never runs here: widget changes go to
//! the plugin's worker as `ui` events (`plugin.uiEvent`), with the tree revision so stale events
//! are dropped.

use std::cell::RefCell;
use std::sync::Arc;

use craft_ext_store::Manager;
use craft_ext_ui::plugin_ui::{Host, PluginUi};
use craft_ext_ui::{ManagerView, ProjectPrompt};
use serde_json::{Value, json};

use crate::EffectcraftApp;

#[derive(Default)]
struct State {
    view: Option<ManagerView>,
    manager: Option<Arc<Manager>>,
    manager_error: Option<String>,
    prompt: Option<ProjectPrompt>,
    prompt_document: Option<Arc<()>>,
    refreshed: bool,
    renderer: PluginUi,
    last_tool: Option<crate::state::Tool>,
    prompt_plugins: Vec<craft_ext_core::id::ExtensionId>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn manager(st: &mut State) -> Option<Arc<Manager>> {
    if st.manager.is_none() && st.manager_error.is_none() {
        match effectcraft_plugins::manager() {
            Ok(m) => st.manager = Some(Arc::new(m)),
            Err(e) => st.manager_error = Some(e),
        }
    }
    st.manager.clone()
}

/// Every frame: serve plugin requests, deliver app events, schedule recurring catalogue/kill-list refresh, and pick
/// up a project-open prompt.
pub fn frame(app: &mut EffectcraftApp, ctx: &egui::Context) {
    ctx.request_repaint_after(std::time::Duration::from_secs(1));
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if st.last_tool.is_some_and(|tool| tool != app.ui.tool) {
            if let Some(rt) = app.session.plugins.clone() {
                rt.tool_event(&mut app.session, "cancel", [0.0, 0.0], Value::Null);
            }
        }
        st.last_tool = Some(app.ui.tool);
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if let Some(rt) = app.session.plugins.clone() {
            rt.tool_event(&mut app.session, "cancel", [0.0, 0.0], Value::Null);
        }
    }
    app.session.pump_plugins();
    let Some(rt) = effectcraft_plugins::runtime() else { return };
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if !st.refreshed {
            st.refreshed = true;
            if !rt.safe_mode() {
                effectcraft_plugins::refresh_in_background();
            }
        }
        if st.prompt_document.as_ref().is_some_and(|d| !Arc::ptr_eq(d, &app.session.plugin_document_token())) {
            st.prompt = None;
            st.prompt_plugins.clear();
            st.prompt_document = None;
        }
        if let Some(statuses) = rt.take_project_prompt(&app.session) {
            st.prompt_document = Some(app.session.plugin_document_token());
            st.prompt_plugins = statuses.iter().map(|(r, _)| r.id.clone()).collect();
            st.prompt = Some(ProjectPrompt::new(statuses));
        }
    });
    // Plugin work arrives from workers: keep frames coming while plugins are loaded.
    if !app.session.plugins.as_ref().is_none_or(|p| p.surfaces().is_empty() && p.commands().is_empty()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Plugin panels and windows, and the project prompt (after the dialogs).
pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if st.prompt.is_some()
            && let Some(m) = manager(&mut st)
            && let Some(p) = st.prompt.as_mut()
            && let Some(outcome) = p.show(ctx, &m)
        {
            let ids = if outcome.open_without { st.prompt_plugins.clone() } else { outcome.skipped };
            if let Some(rt) = effectcraft_plugins::runtime() {
                rt.suppress_for_document(&mut app.session, ids);
            }
            st.prompt = None;
            st.prompt_document = None;
            st.prompt_plugins.clear();
        }
    });
    let Some(rt) = app.session.plugins.clone() else { return };
    let mut events: Vec<(String, u64, String, Value)> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    for sf in rt.surfaces().into_iter().filter(|s| s.open) {
        let mut open = true;
        STATE.with(|st| {
            let mut st = st.borrow_mut();
            let mut host = AppHost(app);
            for event in st.renderer.window(ctx, &sf.key, &sf.title, sf.rev, sf.tree.as_ref(), &mut open, |w| w, &mut host) {
                if let Ok(value) = serde_json::to_value(event.value) {
                    events.push((sf.key.clone(), event.rev, event.widget, value));
                }
            }
        });
        if !open {
            closed.push(sf.key.clone());
        }
    }
    for key in closed {
        rt.set_open(&key, false);
    }
    for (key, rev, widget, value) in events {
        let _ = app.session.execute("plugin.uiEvent", json!({"surface": key, "rev": rev, "widget": widget, "value": value}));
    }
}

/// Settings ▸ Plugins: the shared manager (Browse / Installed / Updates / Problems / setup).
pub fn manager_page(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    if effectcraft_plugins::runtime().is_some_and(|r| r.safe_mode()) {
        ui.label(crate::i18n::tr("Safe mode: no plugin code runs this session. Changes apply after a restart without --safe-mode."));
    }
    if let Some(rt) = app.session.plugins.clone() {
        for id in rt.paused() {
            ui.horizontal(|ui| {
                ui.label(format!("{id}: paused after repeated failures"));
                let r = ui.button(crate::i18n::tr("Resume"));
                app.auto.add(&format!("plugin.resume.{id}"), r.rect, "Resume");
                if r.clicked() {
                    if let Err(e) = rt.resume(&mut app.session, &id) {
                        app.ui.status = e;
                    }
                }
            });
        }
    }
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        let Some(m) = manager(&mut st) else {
            ui.label(format!("{}: {}", crate::i18n::tr("Plugins are unavailable"), st.manager_error.clone().unwrap_or_default()));
            return;
        };
        let mut choice = st.renderer.choice();
        let engine = egui::ComboBox::from_id_salt("plugin-ui-engine").selected_text(format!("{choice:?}")).show_ui(ui, |ui| {
            ui.selectable_value(&mut choice, craft_ext_ui::plugin_ui::EngineChoice::Egui, "Egui");
            ui.selectable_value(&mut choice, craft_ext_ui::plugin_ui::EngineChoice::Framework, "Framework (Egui fallback)");
        });
        app.auto.add("plugins.uiEngine", engine.response.rect, "Plugin UI engine");
        if choice != st.renderer.choice() {
            st.renderer = PluginUi::new(choice);
        }
        let view = st.view.get_or_insert_with(ManagerView::new);
        view.show(ui, &m);
    });
}

struct AppHost<'a>(&'a mut EffectcraftApp);
impl Host for AppHost<'_> {
    fn widget(&mut self, surface: &str, id: &str, rect: egui::Rect, label: &str) {
        self.0.auto.add(&format!("plugin.{surface}.{id}"), rect, label);
    }
    fn tr(&self, english: &str) -> String {
        crate::i18n::tr(english).to_string()
    }
}

/// Effect forms share the panel renderer and automation bridge.
pub fn effect_form(app: &mut EffectcraftApp, ui: &mut egui::Ui, key: &str, tree: &Value) -> Vec<(String, Value)> {
    // Values include evaluated animation; drawing a parsed tree avoids caching it across time changes.
    let Ok(node) = craft_ext_core::ui::tree::Node::parse(tree) else { return Vec::new() };
    STATE.with(|st| {
        st.borrow_mut()
            .renderer
            .draw(ui, craft_ext_ui::plugin_ui::Surface { key, rev: 0, tree: &node }, &mut AppHost(app))
            .into_iter()
            .filter_map(|e| serde_json::to_value(e.value).ok().map(|v| (e.widget, v)))
            .collect()
    })
}
