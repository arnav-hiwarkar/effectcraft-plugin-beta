//! Live grant state over the shared store (B3): every check refreshes the view first (one cheap
//! metadata read, throttled to 250 ms by `GrantView`), so a revoke or kill-list entry written by
//! another app or the CLI takes effect on the next host call or render.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use craft_ext_core::id::ExtensionId;
use craft_ext_core::permission::Permission;
use craft_ext_core::version::Version;
use craft_ext_host::api::Grants;
use craft_ext_store::view::GrantView;

pub struct StoreGrants {
    view: Mutex<GrantView>,
    /// Moves each time the store changed on disk (the runtime reloads contributions then).
    changes: AtomicU64,
}

impl StoreGrants {
    pub fn new(view: GrantView) -> Self {
        Self { view: Mutex::new(view), changes: AtomicU64::new(0) }
    }

    /// The refreshed view. A store that cannot be read keeps the last good state.
    pub fn view(&self) -> MutexGuard<'_, GrantView> {
        let mut v = self.view.lock().unwrap_or_else(PoisonError::into_inner);
        match v.refresh_if_changed() {
            Ok(true) => {
                self.changes.fetch_add(1, Ordering::AcqRel);
            }
            Ok(false) => {}
            Err(e) => log::warn!("plugins: cannot read the plugin store: {e}"),
        }
        v
    }

    /// How often the store changed since start ([`StoreGrants::view`] refreshes it).
    pub fn changes(&self) -> u64 {
        drop(self.view());
        self.changes.load(Ordering::Acquire)
    }
}

impl Grants for StoreGrants {
    fn generation(&self, plugin: &ExtensionId) -> u64 {
        self.view().generation(plugin)
    }
    fn allowed(&self, plugin: &ExtensionId, permission: &Permission) -> bool {
        self.view().allowed(plugin, permission)
    }
    fn stopped(&self, plugin: &ExtensionId, version: Version) -> bool {
        self.view().stopped(plugin, version)
    }
    fn full_access(&self, plugin: &ExtensionId) -> bool {
        self.view().full_access(plugin)
    }
}
