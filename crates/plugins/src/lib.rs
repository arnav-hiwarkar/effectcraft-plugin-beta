//! EffectCraft's adapter for the shared Craft plugin platform (`craft-extensions`).
//!
//! - [`contract`]: what EffectCraft offers plugins (contracts, ops with their permission rules,
//!   events, slots); rendered by `effectcraft-cli plugin contract`.
//! - [`start`]: loads the enabled contributions of active releases from the shared store into a
//!   session: `raster.filter@1` effects register with `effectcraft_effects::external`, commands,
//!   panels and event hooks run as sandboxed sessions (or native helpers with full-access consent)
//!   on worker threads, presets become commands, LUTs are listed by `plugin.resources`.
//! - [`run_cli`]: `effectcraft-cli plugin …`, delegating to `craft_ext_cli::run_plugin`.
//!
//! Plugin code needs threads, processes and files; on wasm32 this crate is an empty adapter.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
pub mod contract;
#[cfg(not(target_arch = "wasm32"))]
mod effects;
#[cfg(not(target_arch = "wasm32"))]
mod grants;
#[cfg(not(target_arch = "wasm32"))]
mod ops;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};

    use craft_ext_store::Manager;
    use effectcraft_engine::Session;

    use crate::runtime::Runtime;

    fn slot() -> &'static Mutex<Option<Arc<Runtime>>> {
        static R: OnceLock<Mutex<Option<Arc<Runtime>>>> = OnceLock::new();
        R.get_or_init(Default::default)
    }

    /// The store root (`CRAFT_EXT_DIR`, else the per-OS default).
    pub fn store_root() -> Result<std::path::PathBuf, String> {
        craft_ext_store::location::default_root().map_err(|e| e.to_string())
    }

    /// Installs the plugin runtime in `s` and loads the enabled contributions. In `safe` mode no
    /// plugin code runs, but the manager and project checks work. Later sessions share the runtime.
    pub fn start(s: &mut Session, safe: bool) {
        let mut slot = slot().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(rt) = slot.as_ref() {
            s.plugins = Some(rt.clone());
            return;
        }
        let rt = match store_root().and_then(|root| Runtime::new(root, safe)) {
            Ok(rt) => Arc::new(rt),
            Err(e) => {
                log::warn!("plugins are unavailable: {e}");
                return;
            }
        };
        rt.load(s);
        s.plugins = Some(rt.clone());
        *slot = Some(rt);
    }

    /// The runtime [`start`] installed (the UI uses it for the manager and project prompts).
    pub fn runtime() -> Option<Arc<Runtime>> {
        slot().lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Stops every plugin worker and native helper (when the app quits).
    pub fn stop() {
        if let Some(rt) = runtime() {
            rt.unload("EffectCraft is closing.");
        }
    }

    /// A manager for this app over the shared store (works in safe mode and without [`start`]).
    pub fn manager() -> Result<Manager, String> {
        let app = craft_ext_core::id::AppId::new(crate::contract::APP).map_err(|e| e.to_string())?;
        Ok(Manager::new(craft_ext_store::Store::at(store_root()?), Some(app), crate::contract::provided(), crate::runtime::platform()))
    }

    /// Refreshes the catalogue and the kill list in the background when the last refresh is older
    /// than six hours (B8). The store change then reloads plugins on the next pump.
    pub fn refresh_in_background() {
        std::thread::spawn(|| {
            let Ok(m) = manager() else { return };
            let now = craft_ext_store::store::now();
            let last = m.store.state().map(|s| s.last_refresh).unwrap_or(0);
            if now.saturating_sub(last) < 6 * 3600 {
                return;
            }
            if let Err(e) = m.refresh(now) {
                log::info!("plugins: catalogue refresh failed: {e}");
            }
        });
    }

    /// `effectcraft-cli plugin …`. `plugin contract` prints the contract reference; everything else
    /// goes to the shared `plugin` command library. Returns the exit code.
    pub fn run_cli(args: &[String], out: &mut dyn std::io::Write, err: &mut dyn std::io::Write) -> i32 {
        if args.first().is_some_and(|a| a == "contract") {
            return match write!(out, "{}", crate::contract::contract().to_markdown()) {
                Ok(()) => 0,
                Err(_) => 1,
            };
        }
        match manager() {
            Ok(m) => craft_ext_cli::run_plugin(&m, args, out, err),
            Err(e) => {
                let _ = writeln!(err, "plugins are unavailable: {e}");
                1
            }
        }
    }
}

/// No plugins on the web build.
#[cfg(target_arch = "wasm32")]
pub fn start(_s: &mut effectcraft_engine::Session, _safe: bool) {}

/// No plugins on the web build.
#[cfg(target_arch = "wasm32")]
pub fn stop() {}
