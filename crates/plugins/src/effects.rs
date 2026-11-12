//! `raster.filter@1` effects: manifest parameters → EffectCraft parameters, the B4 context, and a
//! per-render-thread pool of sandboxed filter instances.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use craft_ext_core::id::ExtensionId;
use craft_ext_core::manifest::{Param, ParamKind};
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
    let (default, ui) = match &p.kind {
        ParamKind::Slider { default, min, max, integer } => {
            (KV::Scalar(*default), ParamUi::Slider { min: *min, max: *max, slider_min: *min, slider_max: *max, decimals: if *integer { 0 } else { 2 } })
        }
        ParamKind::Angle { default } => (KV::Scalar(*default), ParamUi::Angle),
        ParamKind::Checkbox { default } => (KV::Bool(*default), ParamUi::Checkbox),
        ParamKind::Select { options, default } => (KV::Enum(*default), ParamUi::Popup { options: options.clone() }),
        ParamKind::Color { default } => (KV::Color(*default), ParamUi::Color),
        ParamKind::Point { default } => (KV::Vec2(*default), ParamUi::Point),
        ParamKind::Text { default } => (KV::Str(default.clone()), ParamUi::Text),
    };
    ParamSpec { id: intern(&p.id), name: intern(&p.label), default, ui }
}

fn finite(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The B4 filter context for one render.
pub fn context(params: &[Param], ctx: &EffectCtx, buf: &Buf) -> Value {
    let mut out = Map::new();
    for p in params {
        let v = ctx.params.get(&p.id);
        let j = match &p.kind {
            ParamKind::Slider { default, .. } | ParamKind::Angle { default } => json!(finite(v.map_or(*default, KV::as_f64))),
            ParamKind::Checkbox { default } => json!(v.map_or(*default, KV::as_bool)),
            ParamKind::Select { options, default } => {
                let i = v.map_or(*default, KV::as_enum);
                json!(if (i as usize) < options.len() { i } else { *default })
            }
            ParamKind::Color { default } => {
                let c = v.map(KV::as_color).map_or(*default, |c| c.map(f64::from));
                json!(c.map(finite))
            }
            ParamKind::Point { default } => {
                let (x, y) = buf.to_px(v.map_or([default[0] * ctx.layer_size[0], default[1] * ctx.layer_size[1]], KV::as_vec2));
                json!([finite(x), finite(y)])
            }
            ParamKind::Text { default } => match v {
                Some(KV::Str(s)) => json!(s),
                _ => json!(default),
            },
        };
        out.insert(p.id.clone(), j);
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
    fn context_maps_params_points_and_space() {
        let params: Vec<Param> = serde_json::from_value(json!([
            {"id": "size", "label": "Size", "type": "slider", "default": 8, "min": 1, "max": 64},
            {"id": "at", "label": "At", "type": "point", "default": [0.5, 0.5]},
            {"id": "mode", "label": "Mode", "type": "select", "options": ["a", "b"], "default": 1},
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
        assert_eq!(j["params"]["at"], json!([10.0, 12.0]));
        assert_eq!(j["params"]["mode"], json!(1), "an out-of-range index falls back to the default");
        assert_eq!(j["origin"], json!([-5.0, -2.0]));
        assert_eq!(j["full"], json!([50, 25]));
        assert_eq!(j["quality"], "draft");
        assert_eq!(j["color"], "linear-srgb");
        let defaults = Params::default();
        let ctx = EffectCtx { params: &defaults, ..ctx };
        let j = context(&params, &ctx, &buf);
        assert_eq!(j["params"]["at"], json!([30.0, 14.5]), "point defaults are fractions of the full layer");
    }
}
