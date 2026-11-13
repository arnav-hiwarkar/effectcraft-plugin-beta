//! `raster.filter@1` effects: manifest parameters → EffectCraft parameters, the B4 context, and a
//! per-render-thread pool of sandboxed filter instances.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use craft_ext_core::form::{Param, ParamKind, PointSpace};
use craft_ext_core::id::ExtensionId;
use craft_ext_core::permission::Permission;
use craft_ext_core::version::Version;
use craft_ext_host::api::Grants;
use craft_ext_host::wasm::{Filter, LogFn, Plugin, PluginError};
use effectcraft_effects::external::ExternalRender;
use effectcraft_effects::{Buf, EffectCtx, ParamSpec};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::ParamUi;
use serde_json::{Map, Value, json};

use crate::grants::StoreGrants;

/// Leaks each distinct string once (parameter ids and names live as long as the specs).
fn intern(s: &str) -> &'static str {
    static SET: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = SET.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(v) = set.get(s) {
        return v;
    }
    let v: &'static str = Box::leak(s.to_string().into_boxed_str());
    set.insert(v);
    v
}

/// The EffectCraft parameter for one manifest parameter.
pub fn param_spec(p: &Param) -> ParamSpec {
    let slider = |min, max, decimals| ParamUi::Slider { min, max, slider_min: min, slider_max: max, decimals };
    let (default, ui) = match &p.kind {
        ParamKind::Section => (KV::Str(String::new()), ParamUi::Text),
        ParamKind::Range { default, min, max, .. } => (KV::Scalar(*default), slider(*min, *max, 2)),
        ParamKind::Int { default, min, max } => (KV::Scalar(*default as f64), slider(*min as f64, *max as f64, 0)),
        ParamKind::Number { default, min, max } => (KV::Scalar(*default), slider(min.unwrap_or(f64::MIN), max.unwrap_or(f64::MAX), 2)),
        ParamKind::Angle { default } => (KV::Scalar(*default), ParamUi::Angle),
        ParamKind::Bool { default } => (KV::Bool(*default), ParamUi::Checkbox),
        ParamKind::Choice { options, default } | ParamKind::Radio { options, default } => (
            KV::Enum(options.iter().position(|o| o.id == *default).unwrap_or(0) as u32),
            ParamUi::Popup { options: options.iter().map(|o| o.label.english.clone()).collect() },
        ),
        ParamKind::Color { default, .. } => (KV::Color(default.map(f64::from)), ParamUi::Color),
        ParamKind::Point { default, space } => (
            KV::Vec2(match space {
                PointSpace::ImagePercent => default.map(|n| n / 100.0),
                PointSpace::LayerPixels => *default,
            }),
            ParamUi::Point,
        ),
        ParamKind::Text { default, .. } => (KV::Str(default.clone()), ParamUi::Text),
    };
    let name = match &p.unit {
        Some(unit) => format!("{} ({})", p.label.english, unit_label(unit)),
        None => p.label.english.clone(),
    };
    ParamSpec { id: intern(&p.key), name: intern(&name), default, ui }
}

pub fn unit_label(unit: &craft_ext_core::form::Unit) -> &str {
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

fn finite(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The B4 filter context for one render.
pub fn context(params: &[Param], ctx: &EffectCtx, buf: &Buf) -> Value {
    let mut out = Map::new();
    for p in params {
        let v = ctx.params.get(&p.key);
        let j = match &p.kind {
            ParamKind::Section => continue,
            ParamKind::Range { default, .. } | ParamKind::Number { default, .. } | ParamKind::Angle { default } => {
                json!(finite(v.map_or(*default, KV::as_f64)))
            }
            ParamKind::Int { default, .. } => json!(finite(v.map_or(*default as f64, KV::as_f64)).round()),
            ParamKind::Bool { default } => json!(v.map_or(*default, KV::as_bool)),
            ParamKind::Choice { options, default } | ParamKind::Radio { options, default } => {
                json!(v.and_then(|v| options.get(v.as_enum() as usize)).map_or(default.as_str(), |o| o.id.as_str()))
            }
            ParamKind::Color { default, .. } => json!(v.map(KV::as_color).unwrap_or(*default)),
            ParamKind::Point { default, space } => {
                let point = v
                    .map(KV::as_vec2)
                    .map(|v| match space {
                        PointSpace::LayerPixels => v,
                        PointSpace::ImagePercent => [
                            if ctx.layer_size[0] > 0.0 { v[0] * 100.0 / ctx.layer_size[0] } else { 0.0 },
                            if ctx.layer_size[1] > 0.0 { v[1] * 100.0 / ctx.layer_size[1] } else { 0.0 },
                        ],
                    })
                    .unwrap_or(*default);
                json!(point.map(finite))
            }
            ParamKind::Text { default, .. } => match v {
                Some(KV::Str(s)) => json!(s),
                _ => json!(default),
            },
        };
        out.insert(p.key.clone(), j);
    }
    let full = [(ctx.layer_size[0] * buf.scale).round().max(0.0), (ctx.layer_size[1] * buf.scale).round().max(0.0)];
    json!({
        "params": out,
        "time": finite(ctx.time),
        "scale": finite(buf.scale),
        "origin": [finite(-buf.offset[0]), finite(-buf.offset[1])],
        "full": [finite(full[0]) as u32, finite(full[1]) as u32],
        // The final consumer supplies an error sink; resolution does not determine intent.
        "quality": if ctx.env.errors.is_some() { "final" } else { "draft" },
        "seed": ctx.seed,
        "color": if ctx.env.working_linear { "linear-srgb" } else { "srgb" },
    })
}

/// Pooled filter instances on this render thread, by `<plugin>@<version>/<contribution>`.
const MAX_POOLED: usize = 16;
thread_local! {
    static POOL: RefCell<HashMap<Arc<str>, Filter>> = RefCell::new(HashMap::new());
}

/// One plugin effect: renders through a sandboxed `raster.filter@1` instance.
pub struct FilterEffect {
    pub key: Arc<str>,
    pub plugin: ExtensionId,
    pub version: Version,
    pub contribution: String,
    pub required: Vec<Permission>,
    pub module: Plugin,
    pub params: Vec<Param>,
    pub form: Option<craft_ext_core::form::Form>,
    pub states: Mutex<HashMap<u64, craft_ext_core::form::FormState>>,
    pub grants: Arc<StoreGrants>,
    pub log: LogFn,
}

impl FilterEffect {
    fn run(&self, ctx: &EffectCtx, buf: &mut Buf) -> Result<(), PluginError> {
        let json = context(&self.params, ctx, buf);
        let (w, h) = (buf.img.width, buf.img.height);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let pixels: &mut [f32] = bytemuck::cast_slice_mut(&mut buf.img.data);
        let grants = self.grants.clone();
        let (plugin, version) = (self.plugin.clone(), self.version);
        let check = move || {
            let view = grants.view().map_err(PluginError::Stopped)?;
            let enabled = view.setup(&plugin).is_some_and(|s| s.enabled.contains(&self.contribution));
            if view.stopped(&plugin, version) || !enabled || !self.required.iter().all(|p| view.allowed(&plugin, p)) {
                Err(PluginError::Stopped("this plugin effect has been turned off or its permissions revoked".into()))
            } else {
                Ok(())
            }
        };
        POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            if !pool.contains_key(&self.key) {
                if pool.len() >= MAX_POOLED {
                    pool.clear();
                }
                pool.insert(self.key.clone(), Filter::new(&self.module, self.log.clone())?);
            }
            let Some(f) = pool.get_mut(&self.key) else { return Err(PluginError::Load("instance missing".into())) };
            let r = f.run(&json, pixels, w, h, &check);
            if r.is_err() {
                // The host may re-instantiate at any time; after a failure it must (B4).
                pool.remove(&self.key);
            }
            r
        })
    }
}

impl ExternalRender for FilterEffect {
    fn preset_value(&self, key: &str, value: &Value, size: [f64; 2]) -> Result<Value, String> {
        let form = self.form.as_ref().ok_or("effect has no form")?;
        let p = form.params.iter().find(|p| p.key == key).ok_or("unknown form parameter")?;
        let v: craft_ext_core::form::Value = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        form.validate_values(&[(key.to_string(), v.clone())].into_iter().collect())?;
        Ok(host_value(p, &v, size).to_json())
    }
    fn absolute_point(&self, key: &str) -> bool {
        self.params.iter().any(|p| p.key == key && matches!(p.kind, ParamKind::Point { space: PointSpace::LayerPixels, .. }))
    }
    fn heading(&self, key: &str) -> bool {
        self.params.iter().any(|p| p.key == key && matches!(p.kind, ParamKind::Section))
    }

    fn controls(&self, instance: u64, key: &str, values: &effectcraft_effects::Params, size: [f64; 2]) -> (bool, bool) {
        let Some(form) = &self.form else { return (true, true) };
        let Ok(typed) = form_values(form, values, size) else { return (false, false) };
        let states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let state = states.get(&instance);
        let Some(p) = form.params.iter().find(|p| p.key == key) else { return (true, true) };
        (
            p.visible_when.as_ref().is_none_or(|c| c.eval(&typed)) && state.is_none_or(|s| !s.hidden.contains(key)),
            p.enabled_when.as_ref().is_none_or(|c| c.eval(&typed)) && state.is_none_or(|s| !s.disabled.contains(key)),
        )
    }
    fn update(&self, instance: u64, values: &effectcraft_effects::Params, size: [f64; 2]) -> Result<effectcraft_effects::Params, String> {
        let Some(form) = &self.form else { return Ok(values.clone()) };
        let typed = form_values(form, values, size)?;
        form.validate_values(&typed)?;
        let mut states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let mut state = states.get(&instance).cloned().unwrap_or_default();
        state.values = typed;
        let check = || {
            let view = self.grants.view().map_err(PluginError::Stopped)?;
            if view.stopped(&self.plugin, self.version)
                || !view.setup(&self.plugin).is_some_and(|s| s.enabled.contains(&self.contribution))
                || !self.required.iter().all(|p| view.allowed(&self.plugin, p))
            {
                return Err(PluginError::Stopped("plugin disabled or permission revoked".into()));
            }
            Ok(())
        };
        if let Some(patch) = self.module.update(&state.values, state.rev, self.log.clone(), &check).map_err(|e| e.to_string())? {
            form.apply_patch(&mut state, &patch)?;
        }
        let mut result = values.clone();
        for p in &form.params {
            if let Some(v) = state.values.get(&p.key) {
                result.values.insert(p.key.clone(), host_value(p, v, size));
            }
        }
        if states.len() >= 4096 && !states.contains_key(&instance) {
            return Err("too many plugin form instances".into());
        }
        states.insert(instance, state);
        Ok(result)
    }
    fn render(&self, ctx: &EffectCtx, mut buf: Buf) -> Result<Buf, String> {
        // Revocation and the kill list are checked before every render (and between fuel slices).
        if self.grants.stopped(&self.plugin, self.version) {
            return Err("this plugin has been turned off".into());
        }
        self.run(ctx, &mut buf).map_err(|e| e.to_string())?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_effects::{EffectEnv, Image, Params};

    #[test]
    fn update_reset_and_other_values_share_one_undo_step() {
        struct Updating;
        impl ExternalRender for Updating {
            fn render(&self, _: &EffectCtx, buf: Buf) -> Result<Buf, String> {
                Ok(buf)
            }
            fn update(&self, _: u64, values: &Params, _: [f64; 2]) -> Result<Params, String> {
                let mut next = values.clone();
                next.values.insert("x".into(), KV::Scalar(0.0));
                next.values.insert("y".into(), KV::Scalar(42.0));
                Ok(next)
            }
        }
        let id = "io.github.tests.form/reset";
        effectcraft_effects::external::register(
            id,
            "Reset",
            "Tests",
            vec![
                ParamSpec { id: "x", name: "X", default: KV::Scalar(0.0), ui: ParamUi::Number },
                ParamSpec { id: "y", name: "Y", default: KV::Scalar(0.0), ui: ParamUi::Number },
            ],
            Arc::new(Updating),
        )
        .unwrap();
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"name":"Test"})).unwrap();
        s.execute("layer.newSolid", json!({"name":"Layer"})).unwrap();
        let applied = s.execute("effect.apply", json!({"effect":id})).unwrap();
        let path = applied["paths"][0].as_str().unwrap();
        let x = format!("{path}/x");
        let y = format!("{path}/y");
        s.execute("prop.set", json!({"path":x,"value":10})).unwrap();
        assert_eq!(s.execute("prop.get", json!({"path":x})).unwrap()["value"], json!(0.0));
        assert_eq!(s.execute("prop.get", json!({"path":y})).unwrap()["value"], json!(42.0));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.execute("prop.get", json!({"path":y})).unwrap()["value"], json!(0.0));
        effectcraft_effects::external::unregister(id, "test complete");
    }

    #[test]
    fn form_values_and_patch_use_ids_and_declared_space() {
        use craft_ext_core::form::{Form, FormState, Patch, Value as V};
        let form: Form = serde_json::from_value(json!({"schema":1,"key":"controls","title":"Controls","params":[
            {"key":"mode","label":"Mode","kind":{"type":"choice","options":[{"id":"on","label":"On"},{"id":"off","label":"Off"}],"default":"on"}},
            {"key":"at","label":"At","kind":{"type":"point","default":[20,30],"space":"layer-pixels"}},
            {"key":"color","label":"Color","kind":{"type":"color","default":[2,0,0,1],"alpha":true}}
        ]}))
        .unwrap();
        let mut state = FormState { values: form.defaults(), ..Default::default() };
        form.apply_patch(&mut state, &Patch::none(0).set("mode", V::String("off".into()))).unwrap();
        let mut values = Params::default();
        for p in &form.params {
            values.values.insert(p.key.clone(), host_value(p, &state.values[&p.key], [200.0, 100.0]));
        }
        assert_eq!(values.values["mode"], KV::Enum(1));
        assert_eq!(values.values["at"], KV::Vec2([20.0, 30.0]));
        assert_eq!(form_values(&form, &values, [200.0, 100.0]).unwrap(), state.values);
        let before = state.clone();
        assert!(form.apply_patch(&mut state, &Patch::none(1).set("mode", V::String("unknown".into()))).is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn context_maps_params_points_and_space() {
        let params: Vec<Param> = serde_json::from_value(json!([
            {"key": "size", "label": "Size", "kind":{"type": "range", "default": 8, "min": 1, "max": 64}},
            {"key": "at", "label": "At", "kind":{"type": "point", "default": [50, 50], "space":"image-percent"}},
            {"key": "mode", "label": "Mode", "kind":{"type": "choice", "options": [{"id":"a","label":"A"}, {"id":"b","label":"B"}], "default": "b"}},
        ]))
        .unwrap();
        let specs: Vec<ParamSpec> = params.iter().map(param_spec).collect();
        assert_eq!(specs[0].id, "size");
        assert!(matches!(specs[2].ui, ParamUi::Popup { .. }));
        let mut values = Params::default();
        values.values.insert("at".into(), KV::Vec2([10.0, 20.0]));
        values.values.insert("mode".into(), KV::Enum(7));
        let env = EffectEnv { working_linear: true, ..Default::default() };
        let ctx = EffectCtx { params: &values, time: 1.5, layer_size: [100.0, 50.0], seed: 9, adjustment: false, env };
        let buf = Buf { img: Image { width: 60, height: 30, data: vec![] }, offset: [5.0, 2.0], scale: 0.5 };
        let j = context(&params, &ctx, &buf);
        assert_eq!(j["params"]["size"], json!(8.0));
        assert_eq!(j["params"]["at"], json!([10.0, 40.0]));
        assert_eq!(j["params"]["mode"], json!("b"), "an out-of-range index falls back to the default");
        assert_eq!(j["origin"], json!([-5.0, -2.0]));
        assert_eq!(j["full"], json!([50, 25]));
        assert_eq!(j["quality"], "draft");
        assert_eq!(j["color"], "linear-srgb");
        let defaults = Params::default();
        let ctx = EffectCtx { params: &defaults, ..ctx };
        let j = context(&params, &ctx, &buf);
        assert_eq!(j["params"]["at"], json!([50.0, 50.0]), "point defaults are fractions of the full layer");
    }
}

pub fn host_value(p: &Param, v: &craft_ext_core::form::Value, size: [f64; 2]) -> KV {
    use craft_ext_core::form::Value as V;
    match v {
        V::Number(n) => KV::Scalar(*n),
        V::Bool(b) => KV::Bool(*b),
        V::Color(c) => KV::Color(c.map(f64::from)),
        V::Point(q) => KV::Vec2(if matches!(p.kind, ParamKind::Point { space: PointSpace::ImagePercent, .. }) {
            [q[0] * size[0] / 100.0, q[1] * size[1] / 100.0]
        } else {
            *q
        }),
        V::String(s) => match &p.kind {
            ParamKind::Choice { options, .. } | ParamKind::Radio { options, .. } => KV::Enum(options.iter().position(|o| o.id == *s).unwrap_or(0) as u32),
            _ => KV::Str(s.clone()),
        },
    }
}
fn form_values(form: &craft_ext_core::form::Form, params: &effectcraft_effects::Params, size: [f64; 2]) -> Result<craft_ext_core::form::Values, String> {
    let ctx = EffectCtx { params, time: 0.0, layer_size: size, seed: 0, adjustment: false, env: Default::default() };
    let buf = Buf { img: effectcraft_effects::Image { width: 0, height: 0, data: vec![] }, offset: [0.0; 2], scale: 1.0 };
    serde_json::from_value(context(&form.params, &ctx, &buf)["params"].clone()).map_err(|e| e.to_string())
}
