use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::commands::scripts::{OPEN_PANELS_FILE, PANELS_DIR, SCRIPT_SETTINGS_FILE};
use effectcraft_engine::config::DirConfig;
use serde_json::{Value, json};

use crate::Error;

pub const SCRIPT: &str = "Corpus Script.jsx";
pub const SUBMENU_SCRIPT: &str = "Corpus Tools/Corpus Hello.jsx";
pub const PANEL: &str = "Corpus Panel.jsx";
pub const SECTION: &str = "Corpus";

pub const SCRIPT_CODE: &str = "app.project.items.addComp(\"Corpus Script Comp\", 4, 2, 1, 1, 10);\n";
pub const SUBMENU_CODE: &str = "app.settings.saveSetting(\"Corpus\", \"hello\", \"from a submenu\");\n";
pub const PANEL_CODE: &str = "this.add(\"button\", undefined, \"Corpus Button\");\n";
pub const STARTUP_CODE: &str = "app.settings.saveSetting(\"Corpus\", \"startup\", \"ran\");\n";
pub const SHUTDOWN_CODE: &str = "app.settings.saveSetting(\"Corpus\", \"shutdown\", \"ran\");\n";

fn settings_error(e: impl std::fmt::Display) -> Error {
    Error::Settings(e.to_string())
}

fn with_settings(prepare: impl FnOnce(&std::path::Path) -> Result<(), Error>, run: &[(&str, Value)], read: &str) -> Result<Vec<u8>, Error> {
    let dir = tempfile::tempdir().map_err(settings_error)?;
    prepare(dir.path())?;
    let mut s = Session { config: Some(Arc::new(DirConfig::new(dir.path()))), ..Default::default() };
    effectcraft_script::install(&mut s);
    for (id, params) in run {
        s.execute(id, params.clone()).map_err(|e| Error::Settings(format!("{id}: {e}")))?;
    }
    std::fs::read(dir.path().join(read)).map_err(settings_error)
}

pub fn open_panels() -> Result<Vec<u8>, Error> {
    with_settings(
        |dir| {
            let panels = dir.join(PANELS_DIR);
            std::fs::create_dir_all(&panels).map_err(settings_error)?;
            std::fs::write(panels.join(PANEL), PANEL_CODE).map_err(settings_error)
        },
        &[("window.scriptPanel", json!({"name": PANEL}))],
        OPEN_PANELS_FILE,
    )
}

pub fn settings() -> Result<Vec<u8>, Error> {
    with_settings(|_| Ok(()), &[("script.settings.save", json!({"section": SECTION, "key": "greeting", "value": "hello"}))], SCRIPT_SETTINGS_FILE)
}
