use std::cell::RefCell;
use std::sync::Arc;

use craft_ext_ui::craft_ext_manager::{Config, Word};
use craft_ext_ui::plugin_ui::{Host, PluginUi};
use craft_ext_ui::{PluginManager, ProjectPrompt};
use effectcraft_plugins::runtime::{JobInfo, JobState};
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
    prompt_plugins: Vec<craft_ext_core::id::ExtensionId>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn config() -> Result<Config, String> {
    let app = craft_ext_core::id::AppId::new(effectcraft_plugins::contract::APP).map_err(|e| e.to_string())?;
    Ok(Config::app(effectcraft_plugins::store_root()?, app, effectcraft_plugins::contract::provided(), effectcraft_plugins::runtime::platform()))
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
            st.prompt_plugins.clear();
            st.prompt_document = None;
        }
        if let Some(statuses) = rt.take_project_prompt(&app.session) {
            match config() {
                Ok(config) => {
                    st.prompt_document = Some(app.session.plugin_document_token());
                    st.prompt_plugins = statuses.iter().map(|(r, _)| r.id.clone()).collect();
                    st.prompt = Some(ProjectPrompt::new(config, statuses));
                }
                Err(e) => log::warn!("plugins: {e}"),
            }
        }
    });
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
            let ids = if outcome.open_without { st.prompt_plugins.clone() } else { outcome.skipped };
            if let Some(rt) = effectcraft_plugins::runtime() {
                rt.suppress_for_document(&mut app.session, ids);
            }
            st.prompt = None;
            st.prompt_document = None;
            st.prompt_plugins.clear();
        }
    });
    if let Some(rt) = effectcraft_plugins::runtime() {
        unclean_exit_notice(&rt, ctx);
        jobs_window(&rt, ctx);
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

fn unclean_exit_notice(rt: &effectcraft_plugins::runtime::Runtime, ctx: &egui::Context) {
    let names = rt.after_unclean_exit();
    if names.is_empty() {
        return;
    }
    let mut answer = None;
    egui::Window::new(crate::i18n::tr("Plugins after an unexpected close"))
        .id(egui::Id::unique("plugin-unclean-exit"))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!("{} {}", crate::i18n::tr("EffectCraft closed unexpectedly while these plugins were running:"), names.join(", ")));
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr("Start without these plugins")).clicked() {
                    answer = Some(true);
                }
                if ui.button(crate::i18n::tr("Keep them on")).clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(start_without) = answer
        && let Err(e) = rt.answer_unclean_exit(start_without)
    {
        log::warn!("plugins: {e}");
    }
}

fn job_status(job: &JobInfo) -> String {
    match &job.state {
        JobState::Running if job.note.is_empty() => crate::i18n::tr("Working…").to_string(),
        JobState::Running => job.note.clone(),
        JobState::Done(_) => crate::i18n::tr("Done").to_string(),
        JobState::Failed(why) => format!("{}: {why}", crate::i18n::tr("Failed")),
        JobState::Cancelled => crate::i18n::tr("Cancelled").to_string(),
    }
}

fn jobs_window(rt: &effectcraft_plugins::runtime::Runtime, ctx: &egui::Context) {
    let jobs = rt.jobs.list();
    if jobs.is_empty() {
        return;
    }
    if jobs.iter().any(|j| !j.state.finished()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let names = rt.plugin_names();
    egui::Window::new(crate::i18n::tr("Plugin jobs"))
        .id(egui::Id::unique("plugin-jobs"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -36.0))
        .default_open(true)
        .resizable(false)
        .show(ctx, |ui| {
            for job in &jobs {
                let plugin = names.iter().find(|(id, _)| id == &job.plugin).map_or_else(|| job.plugin.to_string(), |(_, n)| n.clone());
                ui.horizontal(|ui| {
                    ui.strong(&job.label);
                    ui.weak(plugin);
                });
                ui.horizontal(|ui| {
                    if !job.state.finished() {
                        match job.progress {
                            Some(p) => {
                                ui.add(egui::ProgressBar::new(p).desired_width(140.0).show_percentage());
                            }
                            None => {
                                ui.spinner();
                            }
                        }
                    }
                    ui.label(job_status(job));
                    if !job.state.finished() && job.cancellable && ui.small_button(crate::i18n::tr("Cancel")).clicked() {
                        rt.jobs.cancel(job.id, false);
                    }
                });
            }
            if jobs.iter().any(|j| j.state.finished()) && ui.button(crate::i18n::tr("Clear finished")).clicked() {
                rt.jobs.clear_finished();
            }
        });
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
                Ok(config) => st.view = Some(PluginManager::new(config)),
                Err(e) => st.manager_error = Some(e),
            }
        }
        let Some(view) = st.view.as_mut() else {
            ui.label(format!("{}: {}", crate::i18n::tr("Plugins are unavailable"), st.manager_error.clone().unwrap_or_default()));
            return;
        };
        view.show_with_app_problems(ui, &words, |ui| paused_problems(app, ui));
    });
}

fn paused_problems(app: &mut EffectcraftApp, ui: &mut egui::Ui) -> bool {
    let Some(rt) = app.session.plugins.clone() else { return false };
    let paused = rt.paused();
    let turned_off = effectcraft_plugins::runtime().map(|r| r.turned_off()).unwrap_or_default();
    for id in &paused {
        let reason = turned_off
            .iter()
            .find(|t| &t.plugin == id)
            .map_or_else(|| crate::i18n::tr("paused after repeated failures").to_string(), |t| format!("{}: {}", t.name, t.reason));
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(format!("{id}: {reason}"));
                let r = ui.button(crate::i18n::tr("Resume"));
                app.auto.add(&format!("plugin.resume.{id}"), r.rect, "Resume");
                if r.clicked()
                    && let Err(e) = rt.resume(&mut app.session, id)
                {
                    app.ui.status = e;
                }
            });
        });
    }
    !paused.is_empty()
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
