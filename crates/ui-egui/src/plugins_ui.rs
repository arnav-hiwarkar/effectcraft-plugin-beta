//! Craft plugins in the desktop UI: the host-drawn plugin panels and windows (from
//! `craft_ext_core::ui::Node` trees), Settings ▸ Plugins (the shared `craft_ext_ui::ManagerView`)
//! and the "This project uses N plugins" prompt. Plugin code never runs here: widget changes go to
//! the plugin's worker as `ui` events (`plugin.uiEvent`), with the tree revision so stale events
//! are dropped.

use std::cell::RefCell;
use std::sync::Arc;

use craft_ext_core::ui::{DrawOp, Node};
use craft_ext_store::Manager;
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
        let id = egui::Id::unique(("craft-plugin-surface", &sf.key));
        egui::Window::new(&sf.title).id(id).open(&mut open).resizable(true).default_width(280.0).show(ctx, |ui| {
            let tree = sf.tree.as_ref().and_then(|t| serde_json::from_value::<Node>(t.clone()).ok());
            match tree {
                Some(node) => {
                    let mut out = Vec::new();
                    node_ui(app, ui, &sf.key, sf.rev, &node, &mut out);
                    events.extend(out.into_iter().map(|(w, v)| (sf.key.clone(), sf.rev, w, v)));
                }
                None => {
                    ui.label(crate::i18n::tr("The plugin has not drawn this panel yet."));
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
pub fn manager_page(ui: &mut egui::Ui) {
    if effectcraft_plugins::runtime().is_some_and(|r| r.safe_mode()) {
        ui.label(crate::i18n::tr("Safe mode: no plugin code runs this session. Changes apply after a restart without --safe-mode."));
    }
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        let Some(m) = manager(&mut st) else {
            ui.label(format!("{}: {}", crate::i18n::tr("Plugins are unavailable"), st.manager_error.clone().unwrap_or_default()));
            return;
        };
        let view = st.view.get_or_insert_with(ManagerView::new);
        view.show(ui, &m);
    });
}

fn rgba(c: [f32; 4]) -> egui::Color32 {
    let u = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgba_unmultiplied(u(c[0]), u(c[1]), u(c[2]), u(c[3]))
}

/// Draws one validated tree; widget changes are pushed to `out` as (widget id, value).
///
/// This is the only place panel trees are rendered ([`show`] calls it per open surface): swapping
/// in the craft-framework renderer later means replacing this one function with the same inputs.
fn node_ui(app: &mut EffectcraftApp, ui: &mut egui::Ui, key: &str, rev: u64, n: &Node, out: &mut Vec<(String, Value)>) {
    let auto = |app: &mut EffectcraftApp, id: &str, r: &egui::Response, label: &str| app.auto.add(&format!("plugin.{key}.{id}"), r.rect, label);
    match n {
        Node::Component { fallback, .. } => node_ui(app, ui, key, rev, fallback, out),
        Node::Form { form, values } => {
            use craft_ext_core::form::{ParamKind as K, Value as V};
            let mut current = form.defaults();
            current.extend(values.clone());
            for param in &form.params {
                if param.visible_when.as_ref().is_some_and(|c| !c.eval(&current)) {
                    continue;
                }
                let enabled = param.enabled_when.as_ref().is_none_or(|c| c.eval(&current));
                let id = egui::Id::unique(("plugin-form-edit", key, &form.key, &param.key));
                let source = current.get(&param.key).cloned();
                let (mut edit, old_source, focused, old_rev) =
                    ui.ctx().data_mut(|d| d.get_temp::<(Option<V>, Option<V>, bool, u64)>(id)).unwrap_or((source.clone(), source.clone(), false, rev));
                if !focused && (source != old_source || old_rev != rev) {
                    edit = source.clone();
                }
                let mut active = false;
                let mut submit = false;
                let label = match &param.unit {
                    Some(u) => format!("{} ({})", param.label.english, form_unit_label(u)),
                    None => param.label.english.clone(),
                };
                ui.add_enabled_ui(enabled, |ui| {
                    ui.push_id((key, &form.key, &param.key), |ui| {
                        let response = match (&param.kind, &mut edit) {
                            (K::Section, _) => {
                                ui.heading(&label);
                                None
                            }
                            (K::Bool { .. }, Some(V::Bool(v))) => Some(ui.checkbox(v, &label)),
                            (K::Range { min, max, step, .. }, Some(V::Number(v))) => {
                                let mut slider = egui::Slider::new(v, *min..=*max).text(&label);
                                if let Some(step) = step {
                                    slider = slider.step_by(*step);
                                }
                                Some(ui.add(slider))
                            }
                            (K::Int { min, max, .. }, Some(V::Number(v))) => {
                                Some(ui.add(egui::Slider::new(v, *min as f64..=*max as f64).integer().text(&label)))
                            }
                            (K::Number { min, max, .. }, Some(V::Number(v))) => {
                                Some(ui.add(egui::DragValue::new(v).range(min.unwrap_or(f64::MIN)..=max.unwrap_or(f64::MAX)).prefix(format!("{label}: "))))
                            }
                            (K::Angle { .. }, Some(V::Number(v))) => Some(ui.add(egui::DragValue::new(v).suffix("°").prefix(format!("{label}: ")))),
                            (K::Choice { options, .. } | K::Radio { options, .. }, Some(V::String(v))) => {
                                let before = v.clone();
                                let r = egui::ComboBox::from_id_salt("choice")
                                    .selected_text(options.iter().find(|o| o.id == *v).map(|o| o.label.english.as_str()).unwrap_or(""))
                                    .show_ui(ui, |ui| {
                                        for o in options {
                                            ui.selectable_value(v, o.id.clone(), &o.label.english);
                                        }
                                    })
                                    .response;
                                submit = *v != before;
                                Some(r)
                            }
                            (K::Text { multiline, .. }, Some(V::String(v))) => {
                                ui.label(&label);
                                Some(if *multiline { ui.text_edit_multiline(v) } else { ui.text_edit_singleline(v) })
                            }
                            (K::Color { .. }, Some(V::Color(v))) => {
                                ui.label(&label);
                                Some(ui.color_edit_button_rgba_unmultiplied(v))
                            }
                            (K::Point { .. }, Some(V::Point(v))) => {
                                ui.label(&label);
                                let mut r = None;
                                for n in v {
                                    let q = ui.add(egui::DragValue::new(n));
                                    submit |= q.changed();
                                    active |= q.has_focus() || q.dragged();
                                    r = Some(q);
                                }
                                r
                            }
                            _ => None,
                        };
                        if let Some(r) = response {
                            auto(app, &param.key, &r, &label);
                            active |= r.has_focus() || r.dragged();
                            submit |=
                                r.drag_stopped() || r.lost_focus() || (r.changed() && (!active || matches!(param.kind, K::Bool { .. } | K::Color { .. })));
                        }
                    });
                });
                if enabled
                    && submit
                    && edit != source
                    && let Some(v) = &edit
                {
                    current.insert(param.key.clone(), v.clone());
                    out.push((param.key.clone(), serde_json::to_value(v).unwrap_or(Value::Null)));
                }
                ui.ctx().data_mut(|d| d.insert_temp(id, (edit, if active { old_source } else { source }, active, if active { old_rev } else { rev })));
            }
        }
        Node::Column { children } => {
            ui.vertical(|ui| children.iter().for_each(|c| node_ui(app, ui, key, rev, c, out)));
        }
        Node::Row { children } => {
            ui.horizontal_wrapped(|ui| children.iter().for_each(|c| node_ui(app, ui, key, rev, c, out)));
        }
        Node::Group { label, children } => {
            ui.group(|ui| {
                ui.label(egui::RichText::new(label).strong());
                children.iter().for_each(|c| node_ui(app, ui, key, rev, c, out));
            });
        }
        Node::Label { text } => {
            ui.label(text);
        }
        Node::Separator => {
            ui.separator();
        }
        Node::Button { id, label } => {
            let r = ui.button(label);
            auto(app, id, &r, label);
            if r.clicked() {
                out.push((id.clone(), Value::Null));
            }
        }
        Node::Checkbox { id, label, value } => {
            let mut v = *value;
            let r = ui.checkbox(&mut v, label);
            auto(app, id, &r, label);
            if r.changed() {
                out.push((id.clone(), json!(v)));
            }
        }
        Node::Slider { id, label, value, min, max } => {
            let edit_id = egui::Id::unique(("plugin-slider-edit", key, id));
            let (mut v, dragging, old_rev) = ui.ctx().data_mut(|d| d.get_temp::<(f64, bool, u64)>(edit_id)).unwrap_or((*value, false, rev));
            if !dragging && old_rev != rev {
                v = *value;
            }
            let r = ui.push_id((key, id), |ui| ui.add(egui::Slider::new(&mut v, *min..=*max).text(label))).inner;
            let active = r.dragged() || r.is_pointer_button_down_on();
            ui.ctx().data_mut(|d| d.insert_temp(edit_id, (v, active, rev)));
            auto(app, id, &r, label);
            if r.drag_stopped() || (dragging && !active) || (r.changed() && !active) {
                out.push((id.clone(), json!(v)));
            }
        }
        Node::TextInput { id, label, value } => {
            let edit_id = egui::Id::unique(("plugin-text-edit", key, rev, id));
            let (mut v, mut sent) = ui.ctx().data_mut(|d| d.get_temp::<(String, String)>(edit_id)).unwrap_or((value.clone(), value.clone()));
            ui.horizontal(|ui| {
                ui.label(label);
                let r = ui.push_id((key, rev, id), |ui| ui.text_edit_singleline(&mut v)).inner;
                auto(app, id, &r, label);
                if (r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))) && v != sent {
                    out.push((id.clone(), json!(v)));
                    sent = v.clone();
                }
            });
            ui.ctx().data_mut(|d| d.insert_temp(edit_id, (v, sent)));
        }
        Node::Select { id, label, options, selected } => {
            let mut sel = *selected;
            let shown = options.get(sel as usize).cloned().unwrap_or_default();
            let r = egui::ComboBox::from_id_salt(("plugin-select", key, id)).selected_text(shown).show_ui(ui, |ui| {
                for (i, o) in options.iter().enumerate() {
                    ui.selectable_value(&mut sel, i as u32, o);
                }
            });
            auto(app, id, &r.response, label);
            if sel != *selected {
                out.push((id.clone(), json!(sel)));
            }
        }
        Node::Color { id, label, rgba: c } => {
            ui.horizontal(|ui| {
                ui.label(label);
                let mut v = *c;
                let r = ui.color_edit_button_rgba_unmultiplied(&mut v);
                auto(app, id, &r, label);
                if r.changed() {
                    out.push((id.clone(), json!(v)));
                }
            });
        }
        Node::Image { width, height, rgba: px } => {
            let img = egui::ColorImage::from_rgba_unmultiplied([*width as usize, *height as usize], px);
            let tex = ui.ctx().load_texture(format!("plugin-image-{key}-{width}x{height}"), img, egui::TextureOptions::LINEAR);
            ui.image((tex.id(), egui::vec2(*width as f32, *height as f32)));
        }
        Node::Canvas { id, label, width, height, ops } => {
            let (r, p) = ui.allocate_painter(egui::vec2(*width, *height), egui::Sense::click_and_drag());
            auto(app, id, &r, label);
            let o = r.rect.min;
            let at = |q: [f32; 2]| egui::pos2(o.x + q[0], o.y + q[1]);
            for op in ops {
                match op {
                    DrawOp::Line { from, to, width, rgba: c } => {
                        p.line_segment([at(*from), at(*to)], egui::Stroke::new(*width, rgba(*c)));
                    }
                    DrawOp::Rect { min, max, fill, stroke } => {
                        let rect = egui::Rect::from_min_max(at(*min), at(*max));
                        p.rect_filled(rect, 0.0, rgba(*fill));
                        if let Some(s) = stroke {
                            p.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, rgba(*s)), egui::StrokeKind::Inside);
                        }
                    }
                    DrawOp::Circle { center, radius, fill } => {
                        p.circle_filled(at(*center), *radius, rgba(*fill));
                    }
                    DrawOp::Text { at: a, text, size, rgba: c } => {
                        p.text(at(*a), egui::Align2::LEFT_TOP, text, egui::FontId::proportional(size.clamp(4.0, 96.0)), rgba(*c));
                    }
                }
            }
            let kind = if r.drag_started() {
                Some("down")
            } else if r.dragged() {
                Some("drag")
            } else if r.drag_stopped() || r.clicked() {
                Some("up")
            } else {
                None
            };
            if let (Some(kind), Some(pos)) = (kind, r.interact_pointer_pos()) {
                out.push((id.clone(), json!({"kind": kind, "at": [pos.x - o.x, pos.y - o.y]})));
            }
        }
    }
}

fn form_unit_label(unit: &craft_ext_core::form::Unit) -> &str {
    use craft_ext_core::form::Unit;
    match unit {
        Unit::Pixels => "px",
        Unit::Percent => "%",
        Unit::Degrees => "°",
        Unit::Seconds => "s",
        Unit::Frames => "frames",
        Unit::Custom(s) => s,
    }
}
