//! Effects contributed at run time by Craft plugins (`effectcraft-plugins`).
//!
//! An external effect is an ordinary [`EffectSpec`]: instances are property groups like every
//! built-in, so its parameters animate, take expressions and show in Effect Controls. Its spec's
//! `render` is a pass-through; [`crate::apply`] sends the work to the registered
//! [`ExternalRender`] by id.
//!
//! Invariants:
//! - Effect ids are `<plugin id>/<contribution>` (never `ec.`), so they cannot shadow a built-in.
//! - [`register`] leaks one spec per id and reuses it while its name, category and parameters are
//!   unchanged; [`unregister`] hides the effect from [`crate::find`] and listings without a restart.
//!   Instances of a hidden effect keep their property groups untouched and render as a
//!   pass-through (the renderer skips effects it cannot find), with [`status`] for the badge.
//! - Preview errors pass through with a status badge; final consumers receive named failures.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use crate::{Buf, EffectCtx, EffectSpec, ParamSpec};

/// The most distinct specs one process may leak (re-registering with new parameters leaks another).
pub const MAX_SPECS: usize = 4096;

/// Renders one external effect. Runs on render threads, possibly several at once.
pub trait ExternalRender: Send + Sync {
    fn render(&self, ctx: &EffectCtx, buf: Buf) -> Result<Buf, String>;
}

struct Entry {
    spec: &'static EffectSpec,
    identity: Option<(String, Option<String>, u32)>,
    /// `None`: unregistered (hidden, passes through).
    render: Option<Arc<dyn ExternalRender>>,
    /// The last render error (cleared by a good render) or why the effect is off.
    status: Option<String>,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, Entry>,
    leaked: usize,
}

fn registry() -> &'static RwLock<Registry> {
    static R: OnceLock<RwLock<Registry>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn same_params(a: &[ParamSpec], b: &[ParamSpec]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.id == y.id && x.name == y.name && x.default == y.default && x.ui == y.ui)
}

/// The spec's render function: external work goes through [`crate::apply`].
fn pass(_: &EffectCtx, buf: Buf) -> Buf {
    buf
}

/// Whether `id` can name an external effect (`<plugin>/<contribution>`, not `ec.`).
pub fn valid_id(id: &str) -> bool {
    !id.starts_with("ec.") && id.contains('/') && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// Registers (or replaces) external effect `id`. Returns its spec; existing instances pick up the
/// new renderer on their next render.
pub fn register(id: &str, name: &str, category: &str, params: Vec<ParamSpec>, render: Arc<dyn ExternalRender>) -> Result<&'static EffectSpec, String> {
    if !valid_id(id) {
        return Err(format!("`{id}` is not a valid plugin effect id (<plugin id>/<contribution>)"));
    }
    if crate::registry().iter().any(|s| s.id == id) {
        return Err(format!("`{id}` is a built-in effect"));
    }
    let mut r = registry().write().unwrap_or_else(PoisonError::into_inner);
    let reuse = r.entries.get(id).map(|e| e.spec).filter(|s| s.name == name && s.category == category && same_params(&s.params, &params));
    let spec = match reuse {
        Some(s) => s,
        None => {
            if r.leaked >= MAX_SPECS {
                return Err("too many plugin effects were loaded in this session; restart EffectCraft".into());
            }
            r.leaked += 1;
            let spec: &'static EffectSpec =
                Box::leak(Box::new(EffectSpec { id: leak(id), name: leak(name), category: leak(category), params, render: pass, gpu: false, float: true }));
            spec
        }
    };
    r.entries.insert(id.to_string(), Entry { spec, identity: None, render: Some(render), status: None });
    drop(r);
    crate::file_cache::bump_generation();
    Ok(spec)
}

/// Hides external effect `id` (plugin turned off, removed or kill-listed): [`crate::find`] no longer
/// returns it, so instances pass through; `why` is shown as the badge.
pub fn unregister(id: &str, why: &str) {
    let mut r = registry().write().unwrap_or_else(PoisonError::into_inner);
    if let Some(e) = r.entries.get_mut(id) {
        e.render = None;
        e.status = Some(why.to_string());
    }
    drop(r);
    crate::file_cache::bump_generation();
}

/// The active external effects.
pub fn active() -> Vec<&'static EffectSpec> {
    let r = registry().read().unwrap_or_else(PoisonError::into_inner);
    let mut v: Vec<&'static EffectSpec> = r.entries.values().filter(|e| e.render.is_some()).map(|e| e.spec).collect();
    v.sort_by(|a, b| a.id.cmp(b.id));
    v
}

/// The active external effect `id`.
pub fn find(id: &str) -> Option<&'static EffectSpec> {
    let r = registry().read().unwrap_or_else(PoisonError::into_inner);
    r.entries.get(id).filter(|e| e.render.is_some()).map(|e| e.spec)
}

/// What Effect Controls shows on an external effect: its last render error, why it is off, or
/// that its plugin is missing. `None` for built-ins and healthy externals.
pub fn status(id: &str) -> Option<String> {
    if crate::registry().iter().any(|s| s.id == id) {
        return None;
    }
    let r = registry().read().unwrap_or_else(PoisonError::into_inner);
    match r.entries.get(id) {
        Some(e) => e.status.clone(),
        None => Some(format!(
            "The plugin for this effect ({}) is not installed or is turned off. Its settings are kept; it passes the image through.",
            id.split('/').next().unwrap_or(id)
        )),
    }
}

fn set_status(id: &str, status: Option<String>) {
    let mut r = registry().write().unwrap_or_else(PoisonError::into_inner);
    if let Some(e) = r.entries.get_mut(id)
        && e.status != status
    {
        e.status = status;
    }
}

/// Runs external effect `spec` (called by [`crate::apply`]); `Err` hands the buffer back when
/// `spec` is not an external effect.
pub(crate) fn run(spec: &EffectSpec, ctx: &EffectCtx, buf: Buf) -> Result<Buf, Buf> {
    let render = {
        let r = registry().read().unwrap_or_else(PoisonError::into_inner);
        match r.entries.get(spec.id) {
            Some(e) if std::ptr::eq(e.spec, spec) => e.render.clone(),
            Some(_) => {
                report(ctx.env.errors, spec.id, "plugin release or parameter schema changed during rendering");
                return Ok(buf);
            }
            None => return Err(buf),
        }
    };
    let Some(render) = render else {
        report(ctx.env.errors, spec.id, "plugin was disabled or removed");
        return Ok(buf);
    };
    let input = buf.clone();
    match render.render(ctx, buf) {
        Ok(out) => {
            if status_present(spec.id) {
                set_status(spec.id, None);
            }
            Ok(out)
        }
        Err(e) => {
            log::warn!("plugin effect `{}`: {e}", spec.id);
            report(ctx.env.errors, spec.id, &e);
            set_status(spec.id, Some(e));
            Ok(input)
        }
    }
}

/// Attach the loaded release identity for project provenance checks.
pub fn set_identity(id: &str, version: String, digest: Option<String>, state_version: u32) {
    if let Some(e) = registry().write().unwrap_or_else(PoisonError::into_inner).entries.get_mut(id) {
        e.identity = Some((version, digest, state_version));
    }
}

/// A recorded release must never be silently substituted, even by a newer release.
pub fn incompatible(id: &str, project: &effectcraft_project::Project) -> Option<String> {
    let plugin = id.split('/').next().unwrap_or(id);
    let recorded = project.plugins.iter().find(|p| p.id == plugin)?;
    let r = registry().read().unwrap_or_else(PoisonError::into_inner);
    let e = r.entries.get(id)?;
    let matches = e.identity.as_ref().is_some_and(|(v, sha, state_version)| {
        let contribution = id.split_once('/').map_or(id, |(_, c)| c);
        v == &recorded.version
            && recorded.sha256.as_ref().is_none_or(|h| sha.as_ref() == Some(h))
            && recorded.contributions.get(contribution).is_none_or(|saved| saved == state_version)
    });
    (!matches).then(|| format!("plugin {plugin} requires recorded version {} and digest {:?}; explicit migration required", recorded.version, recorded.sha256))
}

/// Record a final-render failure; previews expose errors through the status badge.
pub fn report(errors: Option<&std::sync::Mutex<Vec<String>>>, id: &str, why: &str) {
    if let Some(errors) = errors {
        let mut errors = errors.lock().unwrap_or_else(PoisonError::into_inner);
        let message = format!("plugin effect `{id}`: {why}");
        if errors.len() < 256 && !errors.contains(&message) {
            errors.push(message);
        }
    }
}

fn status_present(id: &str) -> bool {
    registry().read().unwrap_or_else(PoisonError::into_inner).entries.get(id).is_some_and(|e| e.status.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, Image, Params};
    use effectcraft_keyframe::Value;
    use effectcraft_project::ParamUi;

    struct Fill(Result<f32, String>);
    impl ExternalRender for Fill {
        fn render(&self, _: &EffectCtx, mut buf: Buf) -> Result<Buf, String> {
            let v = self.0.clone()?;
            for p in &mut buf.img.data {
                *p = [v, v, v, 1.0];
            }
            Ok(buf)
        }
    }

    fn run_once(spec: &EffectSpec) -> f32 {
        let params = Params::default();
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [2.0, 1.0], seed: 1, adjustment: false, env: EffectEnv::default() };
        let img = Image { width: 2, height: 1, data: vec![[0.25, 0.25, 0.25, 1.0]; 2] };
        crate::apply(spec, &ctx, Buf { img, offset: [0.0; 2], scale: 1.0 }).img.data[0][0]
    }

    #[test]
    fn external_provenance_refuses_release_and_state_substitution() {
        let id = "io.github.tests.provenance/fx";
        register(id, "Provenance", "Plugins", vec![], Arc::new(Fill(Ok(1.0)))).unwrap();
        set_identity(id, "2.0.0".into(), Some("b".repeat(64)), 2);
        let mut project = effectcraft_project::Project::default();
        project.plugins.push(effectcraft_project::PluginRef {
            id: "io.github.tests.provenance".into(),
            version: "1.0.0".into(),
            sha256: Some("a".repeat(64)),
            source: "local".into(),
            name: "Provenance".into(),
            contributions: [("fx".into(), 1)].into(),
        });
        assert!(incompatible(id, &project).is_some());
        set_identity(id, "1.0.0".into(), Some("a".repeat(64)), 2);
        assert!(incompatible(id, &project).is_some(), "state schema is also provenance");
        set_identity(id, "1.0.0".into(), Some("a".repeat(64)), 1);
        assert!(incompatible(id, &project).is_none());
    }

    #[test]
    fn external_final_error_channel_reports_failure_and_revocation() {
        let spec = register("io.github.tests.final/fx", "Final", "Plugins", vec![], Arc::new(Fill(Err("revoked".into())))).unwrap();
        let errors = std::sync::Mutex::new(Vec::new());
        let params = Params::default();
        let ctx = EffectCtx {
            params: &params,
            time: 0.0,
            layer_size: [1.0; 2],
            seed: 0,
            adjustment: false,
            env: EffectEnv { errors: Some(&errors), ..Default::default() },
        };
        let buf = || Buf { img: Image { width: 1, height: 1, data: vec![[0.25; 4]] }, offset: [0.0; 2], scale: 0.5 };
        crate::apply(spec, &ctx, buf());
        assert!(errors.lock().unwrap()[0].contains("io.github.tests.final/fx"));
        unregister(spec.id, "disabled");
        crate::apply(spec, &ctx, buf());
        assert_eq!(errors.lock().unwrap().len(), 2);
    }

    #[test]
    fn register_replace_unregister_and_errors_pass_through() {
        let params = || vec![ParamSpec { id: "size", name: "Size", default: Value::Scalar(1.0), ui: ParamUi::Angle }];
        assert!(register("ec.blur.gaussian", "X", "Blur", params(), Arc::new(Fill(Ok(1.0)))).is_err());
        assert!(register("no-slash", "X", "Blur", params(), Arc::new(Fill(Ok(1.0)))).is_err());
        let a = register("io.github.t.x/fx", "Fx", "Plugins", params(), Arc::new(Fill(Ok(1.0)))).unwrap();
        assert!(crate::find("io.github.t.x/fx").is_some());
        assert!(crate::categories().contains(&"Plugins"));
        assert_eq!(run_once(a), 1.0);
        // Replaced without a restart: the same spec, the new renderer.
        let b = register("io.github.t.x/fx", "Fx", "Plugins", params(), Arc::new(Fill(Err("boom".into())))).unwrap();
        assert!(std::ptr::eq(a, b));
        assert_eq!(run_once(b), 0.25, "an error passes the input through");
        assert_eq!(status("io.github.t.x/fx").as_deref(), Some("boom"));
        unregister("io.github.t.x/fx", "turned off");
        assert!(crate::find("io.github.t.x/fx").is_none());
        assert_eq!(status("io.github.t.x/fx").as_deref(), Some("turned off"));
        assert!(status("io.github.t.y/missing").is_some());
        assert!(status("ec.blur.gaussian").is_none());
    }
}
