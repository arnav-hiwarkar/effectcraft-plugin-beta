//! Live grant state over the shared store (B3): every check refreshes the view first (one cheap
//! metadata read, throttled to 250 ms by `GrantView`), so a revoke or kill-list entry written by
//! another app or the CLI takes effect on the next host call or render.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use craft_ext_core::id::ExtensionId;
use craft_ext_core::permission::Permission;
use craft_ext_core::version::Version;
use craft_ext_host::api::Grants;
use craft_ext_store::view::GrantView;

pub struct StoreGrants {
    view: Mutex<GrantView>,
    root: std::path::PathBuf,
    app: craft_ext_core::id::AppId,
    readable: AtomicBool,
    /// Moves each time the store changed on disk (the runtime reloads contributions then).
    changes: AtomicU64,
}

impl StoreGrants {
    pub fn new(view: GrantView, root: std::path::PathBuf, app: craft_ext_core::id::AppId) -> Self {
        Self { view: Mutex::new(view), root, app, readable: AtomicBool::new(true), changes: AtomicU64::new(0) }
    }

    /// Never expose stale grants after a failed refresh. Reopen until the store is readable.
    pub fn view(&self) -> Result<MutexGuard<'_, GrantView>, String> {
        let mut v = self.view.lock().unwrap_or_else(PoisonError::into_inner);
        let refresh = if self.readable.load(Ordering::Acquire) {
            v.refresh_if_changed().map_err(|e| e.to_string())
        } else {
            GrantView::open(&self.root, self.app.clone())
                .map(|fresh| {
                    *v = fresh;
                    true
                })
                .map_err(|e| e.to_string())
        };
        match refresh {
            Ok(changed) => {
                let recovered = !self.readable.swap(true, Ordering::AcqRel);
                if changed || recovered {
                    self.changes.fetch_add(1, Ordering::AcqRel);
                }
                Ok(v)
            }
            Err(e) => {
                if self.readable.swap(false, Ordering::AcqRel) {
                    self.changes.fetch_add(1, Ordering::AcqRel);
                    log::warn!("plugins: cannot read the plugin store: {e}");
                }
                Err(e)
            }
        }
    }

    /// How often the store changed since start ([`StoreGrants::view`] refreshes it).
    pub fn changes(&self) -> u64 {
        drop(self.view());
        self.changes.load(Ordering::Acquire)
    }
}

impl Grants for StoreGrants {
    fn generation(&self, plugin: &ExtensionId) -> u64 {
        self.view().map_or(u64::MAX, |v| v.generation(plugin))
    }
    fn allowed(&self, plugin: &ExtensionId, permission: &Permission) -> bool {
        self.view().is_ok_and(|v| v.allowed(plugin, permission))
    }
    fn stopped(&self, plugin: &ExtensionId, version: Version) -> bool {
        self.view().map_or(true, |v| v.stopped(plugin, version))
    }
    fn full_access(&self, plugin: &ExtensionId) -> bool {
        self.view().is_ok_and(|v| v.full_access(plugin))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_ext_core::id::AppId;
    use craft_ext_store::state::{AppSetup, STATE_FILE, StoreState};
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn unreadable_refresh_denies_until_readable_again() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppId::new("effectcraft").unwrap();
        let id = ExtensionId::new("io.github.tests.grants").unwrap();
        let mut state = StoreState::default();
        state.apps.insert(
            app.clone(),
            BTreeMap::from([(id.clone(), AppSetup { grants: BTreeSet::from([Permission::UiWindow]), full_access: true, ..Default::default() })]),
        );
        let path = dir.path().join(STATE_FILE);
        let bytes = serde_json::to_vec(&state).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let grants = StoreGrants::new(GrantView::open(dir.path(), app.clone()).unwrap(), dir.path().to_path_buf(), app);
        assert!(grants.allowed(&id, &Permission::UiWindow));
        std::fs::write(&path, b"corrupt").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(260));
        assert!(!grants.allowed(&id, &Permission::UiWindow));
        assert!(!grants.full_access(&id));
        assert!(grants.stopped(&id, Version::parse("1.0.0").unwrap()));
        assert!(!grants.allowed(&id, &Permission::UiWindow), "a throttled refresh must not restore stale rights");
        std::fs::write(&path, bytes).unwrap();
        assert!(grants.allowed(&id, &Permission::UiWindow));
        assert!(grants.full_access(&id));
        assert!(grants.changes() >= 3);
    }
}
