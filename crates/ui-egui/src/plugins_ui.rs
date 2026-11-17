use std::cell::RefCell;
use std::sync::Arc;

use craft_ext_ui::craft_ext_manager::{Action, Config, Tab, Word};
use craft_ext_ui::plugin_ui::{Host, PluginUi};
use craft_ext_ui::{PluginManager, ProjectPrompt};
use serde_json::{Value, json};

use crate::EffectcraftApp;

#[derive(Default)]
struct State {
    view: Option<PluginManager>,
    manager_error: Option<String>,
    prompt: Option<ProjectPrompt>,
    prompt_document: Option<Arc<()>>,
    refreshed: bool,
    renderer: PluginUi,
    last_tool: Option<crate::state::Tool>,
    unclean_shown: bool,
    open_problems: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn config() -> Result<Config, String> {
    let app = craft_ext_core::id::AppId::new(effectcraft_plugins::contract::APP).map_err(|e| e.to_string())?;
    Ok(Config::app(effectcraft_plugins::store_root()?, app, effectcraft_plugins::contract::provided(), craft_ext_ui::craft_ext_host::native::platform()))
}

fn manager_view(config: Config) -> PluginManager {
    let mut view = PluginManager::new(config.clone()).with_install_file();
    if let (Some(rt), Some(app)) = (effectcraft_plugins::runtime(), config.scope()) {
        view.model_mut().set_unclean_exit(app.clone(), rt.unclean_exit());
    }
    view
}

fn words(word: Word) -> String {
    crate::i18n::tr(word.english()).to_string()
}

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
            st.prompt_document = None;
        }
        if let Some(statuses) = rt.take_project_prompt(&app.session) {
            match config() {
                Ok(config) => {
                    st.prompt_document = Some(app.session.plugin_document_token());
                    st.prompt = Some(ProjectPrompt::new(config, statuses));
                }
                Err(e) => log::warn!("plugins: {e}"),
            }
        }
    });
    let crashed = STATE.with(|st| !st.borrow().unclean_shown) && !rt.unclean_exit().is_empty();
    if crashed {
        STATE.with(|st| {
            let mut st = st.borrow_mut();
            st.unclean_shown = true;
            st.open_problems = true;
            st.view = None;
        });
        crate::panels::settings::open(app, "plugins");
    }
    if !app.session.plugins.as_ref().is_none_or(|p| p.surfaces().is_empty() && p.commands().is_empty()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if let Some(p) = st.prompt.as_mut()
            && let Some(outcome) = p.show(ctx, &words)
        {
            if let Some(rt) = effectcraft_plugins::runtime() {
                rt.suppress_for_document(&mut app.session, outcome.skipped);
            }
            st.prompt = None;
            st.prompt_document = None;
        }
    });
    if let Some(rt) = effectcraft_plugins::runtime() {
        let names = rt.plugin_names();
        let name = |id: &craft_ext_core::id::ExtensionId| names.iter().find(|(p, _)| p == id).map_or_else(|| id.to_string(), |(_, n)| n.clone());
        craft_ext_ui::jobs::jobs_window(ctx, &rt.jobs, &name, &|english| crate::i18n::tr(english).to_string());
    }
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

pub fn manager_page(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    if effectcraft_plugins::runtime().is_some_and(|r| r.safe_mode()) {
        ui.label(crate::i18n::tr("Safe mode: no plugin code runs this session. Changes apply after a restart without --safe-mode."));
    }
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        let mut choice = st.renderer.choice();
        let engine = egui::ComboBox::from_id_salt("plugin-ui-engine").selected_text(format!("{choice:?}")).show_ui(ui, |ui| {
            ui.selectable_value(&mut choice, craft_ext_ui::plugin_ui::EngineChoice::Egui, "Egui");
            ui.selectable_value(&mut choice, craft_ext_ui::plugin_ui::EngineChoice::Framework, "Framework (Egui fallback)");
        });
        app.auto.add("plugins.uiEngine", engine.response.rect, "Plugin UI engine");
        if choice != st.renderer.choice() {
            st.renderer = PluginUi::new(choice);
        }
        if st.view.is_none() && st.manager_error.is_none() {
            match config() {
                Ok(config) => st.view = Some(manager_view(config)),
                Err(e) => st.manager_error = Some(e),
            }
        }
        let open_problems = std::mem::take(&mut st.open_problems);
        let Some(view) = st.view.as_mut() else {
            ui.label(format!("{}: {}", crate::i18n::tr("Plugins are unavailable"), st.manager_error.clone().unwrap_or_default()));
            return;
        };
        if open_problems {
            view.model_mut().apply(Action::SetTab(Tab::Problems));
        }
        if let (Some(rt), Some(app_id)) = (effectcraft_plugins::runtime(), view.model().config().scope().cloned()) {
            view.model_mut().set_load_problems(app_id, rt.load_problems());
        }
        view.show(ui, &words);
        if view.take_install_file_request()
            && let Some(path) = app.hooks.pick_files.as_ref().and_then(|pick| pick(&["craftplugin"]).into_iter().next())
        {
            view.install_file(path.into());
        }
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
