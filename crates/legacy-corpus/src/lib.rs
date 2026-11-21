#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod library;
mod modules;
mod projects;
mod scripts;

use std::fmt;
use std::path::{Path, PathBuf};

use craft_testkit::corpus::Corpus;

pub use library::{CATALOG_FETCHED, MISMATCHED_SHA256};
pub use modules::{ABSENT_ID, BUNDLE_ID, DEFAULT_DIR_ID, FOLDER_ID, GAIN_ID, LIBRARY_ID, NEIGHBOUR_PROBE_ID, NEIGHBOURS_ID, PARAMS_PROBE_ID, TAMPERED_ID};
pub use projects::{GAIN_COMP, NEIGHBOUR_PROBE_COMP, NEIGHBOURS_COMP, PARAMS_PROBE_COMP, SAVED_BY};
pub use scripts::{BUNDLE_PANEL, PANEL, SCRIPT, SECTION, SUBMENU_SCRIPT};

pub const APP: &str = "effectcraft";

craft_testkit::legacy_paths! {
    pub enum LegacyPath {
        WasmAbi1 => "wasm-abi1",
        WasmAbi2 => "wasm-abi2",
        ProjectPluginEffect => "project-plugin-effect",
        JsxScript => "jsx-script",
        ScriptUiPanels => "scriptui-panels",
        ScriptSettings => "script-settings",
        StartupScripts => "startup-scripts",
        ShutdownScripts => "shutdown-scripts",
        LibraryVerified => "library-verified",
        LibraryMismatch => "library-mismatch",
        PluginsDir => "plugins-dir",
        PluginsDirDefault => "plugins-dir-default",
        PluginsInstall => "effect-plugins-install",
        MissingPlugin => "plugin-missing",
    }
}

#[derive(Debug)]
pub enum Error {
    Module(String),
    Project(String),
    Settings(String),
    Corpus(craft_testkit::corpus::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Module(e) => write!(f, "module: {e}"),
            Error::Project(e) => write!(f, "project: {e}"),
            Error::Settings(e) => write!(f, "settings: {e}"),
            Error::Corpus(e) => write!(f, "corpus: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<craft_testkit::corpus::Error> for Error {
    fn from(e: craft_testkit::corpus::Error) -> Self {
        Error::Corpus(e)
    }
}

pub fn root() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir.ancestors().nth(2).unwrap_or(crate_dir).join("tests/legacy-corpus")
}

fn file(path: &str, bytes: impl Into<Vec<u8>>) -> (String, Vec<u8>) {
    (path.to_owned(), bytes.into())
}

pub fn generate() -> Result<Corpus, Error> {
    let gain = modules::gain(GAIN_ID, "Corpus Gain")?;
    let neighbours = modules::neighbours()?;
    let folder = modules::gain(FOLDER_ID, "Corpus Folder Gain")?;
    let params_probe = modules::params_probe()?;
    let neighbour_probe = modules::neighbour_probe()?;
    let mut corpus = Corpus::new(APP)?;
    corpus.add(
        "gain-effect",
        LegacyPath::WasmAbi1.name(),
        "ABI 1 org.example.corpus.gain loads from .wasm and from .wat text and multiplies colour by gain, 1 by default",
        vec![file("gain.wat", gain.text.clone()), file("gain.wasm", gain.binary.clone())],
    )?;
    corpus.add(
        "neighbours-effect",
        LegacyPath::WasmAbi2.name(),
        "ABI 2 org.example.corpus.neighbours writes the previous frame's red, the neighbour flags over 4 and the next frame's red",
        vec![file("neighbours.wat", neighbours.text.clone()), file("neighbours.wasm", neighbours.binary.clone())],
    )?;
    let (project, missing) = projects::projects(&gain.binary, &neighbours.binary)?;
    corpus.add(
        "project",
        LegacyPath::ProjectPluginEffect.name(),
        "comp Gain renders the 0.8, 0.4, 0.2 solid at gain 0.5 in 8 bpc; comp Neighbours gets both neighbour frames on frames 0 and 1",
        vec![file("project.ecproj", project)],
    )?;
    corpus.add(
        "missing-effect-project",
        LegacyPath::MissingPlugin.name(),
        "the effect org.example.corpus.absent is kept through save, renders as the bare solid, and library.missing lists it",
        vec![file("project.ecproj", missing)],
    )?;
    corpus.add(
        "scripts",
        LegacyPath::JsxScript.name(),
        "File > Scripts lists Corpus Script.jsx and Corpus Tools/Corpus Hello.jsx; running Corpus Script.jsx adds the comp Corpus Script Comp",
        vec![file(&format!("Scripts/{SCRIPT}"), scripts::SCRIPT_CODE), file(&format!("Scripts/{SUBMENU_SCRIPT}"), scripts::SUBMENU_CODE)],
    )?;
    corpus.add(
        "scriptui-panels",
        LegacyPath::ScriptUiPanels.name(),
        "window.restoreScriptPanels reopens Corpus Panel.jsx from scriptui_panels.json",
        vec![file(&format!("Scripts/ScriptUI Panels/{PANEL}"), scripts::PANEL_CODE), file("scriptui_panels.json", scripts::open_panels()?)],
    )?;
    corpus.add(
        "script-settings",
        LegacyPath::ScriptSettings.name(),
        "app.settings reads greeting = hello in section Corpus from script_settings.json",
        vec![file("script_settings.json", scripts::settings()?)],
    )?;
    corpus.add(
        "startup-scripts",
        LegacyPath::StartupScripts.name(),
        "file.runStartupScripts runs Scripts/Startup/Corpus Startup.jsx, which saves startup = ran",
        vec![file("Scripts/Startup/Corpus Startup.jsx", scripts::STARTUP_CODE)],
    )?;
    corpus.add(
        "shutdown-scripts",
        LegacyPath::ShutdownScripts.name(),
        "file.runShutdownScripts runs Scripts/Shutdown/Corpus Shutdown.jsx, which saves shutdown = ran",
        vec![file("Scripts/Shutdown/Corpus Shutdown.jsx", scripts::SHUTDOWN_CODE)],
    )?;
    corpus.add(
        "library-verified",
        LegacyPath::LibraryVerified.name(),
        "library.loadInstalled loads org.example.corpus.library 1.0.0 after its sha256 matches, and library.list reads catalog.json without fetching",
        library::layout(LIBRARY_ID, false)?,
    )?;
    corpus.add(
        "library-mismatch",
        LegacyPath::LibraryMismatch.name(),
        "library.loadInstalled refuses org.example.corpus.tampered because installed.json lists another sha256",
        library::layout(TAMPERED_ID, true)?,
    )?;
    corpus.add(
        "plugins-dir",
        LegacyPath::PluginsDir.name(),
        "effect.plugins.load on the Plug-ins folder loads org.example.corpus.folder and reports not-a-module.wasm",
        vec![file("Plug-ins/corpus-folder.wasm", folder.binary), file("Plug-ins/not-a-module.wasm", b"not a module\n".to_vec())],
    )?;
    corpus.add(
        "params-probe-effect",
        LegacyPath::WasmAbi1.name(),
        "the ABI 1 probe writes the parameter count, frame size, time, scale and the ten flattened slider, angle, checkbox, popup, point and colour values",
        vec![file("params-probe.wat", params_probe.text.clone()), file("params-probe.wasm", params_probe.binary.clone())],
    )?;
    corpus.add(
        "neighbour-probe-effect",
        LegacyPath::WasmAbi2.name(),
        "the ABI 2 probe writes the current, previous and next layer times, the flags, the frame size and the red and green of each frame",
        vec![file("neighbour-probe.wat", neighbour_probe.text.clone()), file("neighbour-probe.wasm", neighbour_probe.binary.clone())],
    )?;
    corpus.add(
        "probe-project",
        LegacyPath::ProjectPluginEffect.name(),
        "a 32 bpc project: Params Probe reads the saved parameter values, Neighbour Probe reads an animated Fill one frame either side",
        vec![file("project.ecproj", projects::probe_project(&params_probe.binary, &neighbour_probe.binary)?)],
    )?;
    corpus.add(
        "install-bundle",
        LegacyPath::PluginsInstall.name(),
        "effect.plugins.install of the Bundle folder copies corpus-bundle.wasm into Plug-ins, the panel into ScriptUI Panels, and skips README.txt",
        vec![
            file("Bundle/corpus-bundle.wasm", modules::gain(BUNDLE_ID, "Corpus Bundle Gain")?.binary),
            file(&format!("Bundle/{BUNDLE_PANEL}"), scripts::PANEL_CODE),
            file("Bundle/README.txt", b"Corpus bundle: one effect and one panel.\n".to_vec()),
        ],
    )?;
    corpus.add(
        "default-plugins-dir",
        LegacyPath::PluginsDirDefault.name(),
        "with no host folder, plugins_dir is Plug-ins next to the settings and loading it registers org.example.corpus.default",
        vec![file("Plug-ins/corpus-default.wasm", modules::gain(DEFAULT_DIR_ID, "Corpus Default Gain")?.binary)],
    )?;
    Ok(corpus)
}
