use effectcraft_engine::library::CATALOG_URL;
use effectcraft_legacy_corpus::{
    ABSENT_ID, BUNDLE_ID, BUNDLE_PANEL, CATALOG_FETCHED, DEFAULT_DIR_ID, FOLDER_ID, GAIN_COMP, GAIN_ID, LIBRARY_ID, LegacyPath, NEIGHBOUR_PROBE_COMP,
    NEIGHBOUR_PROBE_ID, NEIGHBOURS_COMP, NEIGHBOURS_ID, PANEL, PARAMS_PROBE_COMP, PARAMS_PROBE_ID, SCRIPT, SECTION, SUBMENU_SCRIPT, TAMPERED_ID,
};
use effectcraft_project::Project;
use serde_json::{Value, json};

use crate::{colour_stream, corpus, effect_ids, fresh_settings, loaded, open_project, settings, thousandths};

const SOLID: [[f32; 4]; 2] = [[0.8, 0.4, 0.2, 1.0]; 2];

fn preview(member: &str, file: &str) -> (Value, Vec<Vec<[f32; 4]>>) {
    effectcraft_plugin::preview(&corpus().bytes(member, file).unwrap(), 2, 1, &SOLID, &[0.0]).unwrap()
}

fn check(kind: LegacyPath) {
    match kind {
        LegacyPath::WasmAbi1 => {
            the_abi1_effect_loads_from_its_module_and_its_text();
            the_abi1_probe_receives_every_parameter_type();
        }
        LegacyPath::WasmAbi2 => {
            the_abi2_effect_gets_zeroed_neighbours_without_a_host();
            the_abi2_probe_without_a_host_sees_no_neighbours();
        }
        LegacyPath::ProjectPluginEffect => {
            the_project_renders_its_plugin_effects();
            the_probe_project_passes_saved_params_and_distinct_neighbours();
        }
        LegacyPath::JsxScript => installed_scripts_are_listed_and_run(),
        LegacyPath::ScriptUiPanels => remembered_scriptui_panels_reopen(),
        LegacyPath::ScriptSettings => script_settings_load_from_the_settings_folder(),
        LegacyPath::StartupScripts => startup_scripts_run(),
        LegacyPath::ShutdownScripts => shutdown_scripts_run_and_their_settings_are_kept(),
        LegacyPath::LibraryVerified => a_verified_library_install_loads_and_lists_from_the_cached_catalog(),
        LegacyPath::LibraryMismatch => a_mismatched_library_install_is_refused(),
        LegacyPath::PluginsDir => the_plugins_folder_loads_its_modules_and_reports_the_rest(),
        LegacyPath::PluginsDirDefault => the_default_plugins_folder_is_next_to_the_settings(),
        LegacyPath::PluginsInstall => a_bundle_installs_its_effect_and_panel(),
        LegacyPath::MissingPlugin => a_missing_effect_is_kept_and_renders_the_bare_solid(),
    }
}

#[test]
fn every_legacy_path_passes_its_parity_test() {
    let failed: Vec<&str> = LegacyPath::ALL.into_iter().filter(|kind| std::panic::catch_unwind(|| check(*kind)).is_err()).map(LegacyPath::name).collect();
    assert_eq!(failed, Vec::<&str>::new());
}

fn thousandth(v: f32) -> i32 {
    (v * 1000.0).round() as i32
}

fn the_abi1_probe_receives_every_parameter_type() {
    let pixels = [[0.8, 0.4, 0.2, 1.0]; 8];
    let (manifest, frames) = effectcraft_plugin::preview(&corpus().bytes("params-probe-effect", "params-probe.wasm").unwrap(), 4, 2, &pixels, &[0.5]).unwrap();
    assert_eq!(manifest["id"], json!(PARAMS_PROBE_ID));
    let stream: Vec<i32> = frames[0].iter().flat_map(|px| px[..3].iter().map(|v| thousandth(*v)).collect::<Vec<_>>()).take(15).collect();
    assert_eq!(stream, vec![10_000, 4000, 2000, 500, 1000, 250, 30_000, 1000, 1000, 1000, 1500, 100, 200, 300, 1000]);
}

fn the_abi2_probe_without_a_host_sees_no_neighbours() {
    let pixels = [[0.8, 0.4, 0.2, 1.0]; 8];
    let (manifest, frames) =
        effectcraft_plugin::preview(&corpus().bytes("neighbour-probe-effect", "neighbour-probe.wasm").unwrap(), 4, 2, &pixels, &[0.5]).unwrap();
    assert_eq!(manifest["id"], json!(NEIGHBOUR_PROBE_ID));
    let stream: Vec<i32> = frames[0].iter().flat_map(|px| px[..3].iter().map(|v| thousandth(*v)).collect::<Vec<_>>()).take(12).collect();
    assert_eq!(stream, vec![500, 0, 0, 0, 4000, 2000, 0, 800, 0, 0, 400, 0]);
}

fn the_probe_project_passes_saved_params_and_distinct_neighbours() {
    loaded();
    let s = open_project("probe-project");
    assert_eq!(effect_ids(&s.project), vec!["ec.generate.fill".to_owned(), NEIGHBOUR_PROBE_ID.to_owned(), PARAMS_PROBE_ID.to_owned()]);
    assert_eq!(
        colour_stream(&thousandths(&s, PARAMS_PROBE_COMP, 0.0), 15),
        vec![10_000, 4000, 4000, 0, 1000, 500, 45_000, 0, 2000, 3000, 1000, 400, 500, 600, 1000]
    );
    assert_eq!(colour_stream(&thousandths(&s, NEIGHBOUR_PROBE_COMP, 0.5), 12), vec![500, 400, 600, 3000, 4000, 4000, 400, 500, 600, 200, 250, 300]);
}

fn the_default_plugins_folder_is_next_to_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    corpus().copy_member("default-plugins-dir", dir.path()).unwrap();
    let mut s = effectcraft_engine::Session {
        config: Some(std::sync::Arc::new(effectcraft_engine::config::DirConfig::new(dir.path()))),
        plugin_loader: Some(effectcraft_plugin::loader),
        ..Default::default()
    };
    let folder = dir.path().join("Plug-ins");
    assert_eq!((s.plugins_dir(), s.library_dir()), (Some(folder.clone()), Some(folder.join("Library"))));
    let report = s.execute("effect.plugins.load", json!({"folder": folder.to_string_lossy()})).unwrap();
    assert_eq!((&report["loaded"][0]["id"], &report["errors"]), (&json!(DEFAULT_DIR_ID), &json!([])));
}

fn a_bundle_installs_its_effect_and_panel() {
    let source = tempfile::tempdir().unwrap();
    corpus().copy_member("install-bundle", source.path()).unwrap();
    let mut st = fresh_settings();
    let report = st.session.execute("effect.plugins.install", json!({"folder": source.path().join("Bundle").to_string_lossy()})).unwrap();
    assert_eq!((&report["installed"], &report["panels"], &report["errors"]), (&json!(["corpus-bundle.wasm"]), &json!([BUNDLE_PANEL]), &json!([])));
    assert_eq!(report["loaded"][0]["id"], json!(BUNDLE_ID));
    let corpus = corpus();
    assert_eq!(std::fs::read(st.path("Plug-ins/corpus-bundle.wasm")).unwrap(), corpus.bytes("install-bundle", "Bundle/corpus-bundle.wasm").unwrap());
    let panel = format!("Scripts/ScriptUI Panels/{BUNDLE_PANEL}");
    assert_eq!(std::fs::read(st.path(&panel)).unwrap(), corpus.bytes("install-bundle", &format!("Bundle/{BUNDLE_PANEL}")).unwrap());
    assert!(!st.path("Plug-ins/README.txt").exists());
}

fn setting(s: &mut effectcraft_engine::Session, key: &str) -> Value {
    s.execute("script.settings.get", json!({"section": SECTION, "key": key})).unwrap()
}

fn the_abi1_effect_loads_from_its_module_and_its_text() {
    for file in ["gain.wasm", "gain.wat"] {
        let (manifest, frames) = preview("gain-effect", file);
        assert_eq!(
            (&manifest["id"], &manifest["params"][0]["id"], &manifest["params"][0]["default"]),
            (&json!(GAIN_ID), &json!("gain"), &json!(1.0)),
            "{file}"
        );
        assert_eq!(frames, vec![SOLID.to_vec()], "{file}");
    }
}

fn the_abi2_effect_gets_zeroed_neighbours_without_a_host() {
    let (manifest, frames) = preview("neighbours-effect", "neighbours.wasm");
    assert_eq!(manifest["id"], json!(NEIGHBOURS_ID));
    assert_eq!(frames, vec![vec![[0.0, 0.0, 0.0, 1.0]; 2]]);
    loaded();
    assert!(effectcraft_effects::plugin::reads_other_times(NEIGHBOURS_ID));
    assert!(!effectcraft_effects::plugin::reads_other_times(GAIN_ID));
}

fn the_project_renders_its_plugin_effects() {
    loaded();
    let s = open_project("project");
    assert_eq!(effect_ids(&s.project), vec![GAIN_ID.to_owned(), NEIGHBOURS_ID.to_owned()]);
    assert_eq!(thousandths(&s, GAIN_COMP, 0.0), vec![[400, 200, 102, 1000]; 16]);
    for seconds in [0.0, 0.1] {
        assert_eq!(thousandths(&s, NEIGHBOURS_COMP, seconds), vec![[800, 749, 800, 1000]; 16], "{seconds} s");
    }
}

fn a_missing_effect_is_kept_and_renders_the_bare_solid() {
    let mut s = open_project("missing-effect-project");
    assert!(effectcraft_effects::find(ABSENT_ID).is_none());
    assert_eq!(effect_ids(&s.project), vec![ABSENT_ID.to_owned()]);
    assert_eq!(thousandths(&s, GAIN_COMP, 0.0), vec![[800, 400, 200, 1000]; 16]);
    assert_eq!(s.execute("library.missing", json!({})).unwrap(), json!({"effects": [{"effect": ABSENT_ID, "packages": []}]}));
    let resaved = Project::from_json(&s.project.to_file_json().unwrap()).unwrap();
    assert_eq!(effect_ids(&resaved), vec![ABSENT_ID.to_owned()]);
}

fn installed_scripts_are_listed_and_run() {
    let mut st = settings("scripts");
    let list = st.session.execute("file.scripts.list", json!({})).unwrap();
    let installed: Vec<&str> = list.as_array().unwrap().iter().filter(|e| e["source"] == "installed").map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(installed, vec![SCRIPT, SUBMENU_SCRIPT]);
    st.session.execute("file.runScript", json!({"name": SCRIPT})).unwrap();
    assert!(st.session.project.items.values().any(|item| item.name == "Corpus Script Comp"));
}

fn remembered_scriptui_panels_reopen() {
    let mut st = settings("scriptui-panels");
    assert_eq!(st.session.execute("window.restoreScriptPanels", json!({})).unwrap(), json!({"opened": [PANEL], "failed": []}));
}

fn script_settings_load_from_the_settings_folder() {
    let mut st = settings("script-settings");
    assert_eq!(setting(&mut st.session, "greeting"), json!({"have": true, "value": "hello"}));
}

fn startup_scripts_run() {
    let mut st = settings("startup-scripts");
    let ran = st.session.execute("file.runStartupScripts", json!({})).unwrap();
    assert_eq!((&ran["ran"], &ran["failed"]), (&json!(["Corpus Startup.jsx"]), &json!([])));
    assert_eq!(setting(&mut st.session, "startup"), json!({"have": true, "value": "ran"}));
}

fn shutdown_scripts_run_and_their_settings_are_kept() {
    let mut st = settings("shutdown-scripts");
    let ran = st.session.execute("file.runShutdownScripts", json!({})).unwrap();
    assert_eq!((&ran["ran"], &ran["failed"]), (&json!(["Corpus Shutdown.jsx"]), &json!([])));
    let kept: Value = serde_json::from_slice(&std::fs::read(st.path("script_settings.json")).unwrap()).unwrap();
    assert_eq!(kept, json!({"Corpus": {"shutdown": "ran"}}));
}

fn a_verified_library_install_loads_and_lists_from_the_cached_catalog() {
    let mut st = settings("library-verified");
    let catalog = std::fs::read(st.path("Plug-ins/Library/catalog.json")).unwrap();
    let loaded = st.session.execute("library.loadInstalled", json!({})).unwrap();
    assert_eq!(loaded["errors"], json!([]));
    assert_eq!(
        (&loaded["loaded"][0]["package"], &loaded["loaded"][0]["packageVersion"], &loaded["loaded"][0]["id"]),
        (&json!(LIBRARY_ID), &json!("1.0.0"), &json!(LIBRARY_ID))
    );
    let list = st.session.execute("library.list", json!({})).unwrap();
    assert_eq!((&list["catalog"], &list["fetched"], &list["busy"]), (&json!(CATALOG_URL), &json!(CATALOG_FETCHED), &Value::Null));
    let package = &list["packages"][0];
    assert_eq!((&package["id"], &package["installed"], &package["loaded"]), (&json!(LIBRARY_ID), &json!("1.0.0"), &json!("1.0.0")));
    assert_eq!(std::fs::read(st.path("Plug-ins/Library/catalog.json")).unwrap(), catalog);
}

fn a_mismatched_library_install_is_refused() {
    let mut st = settings("library-mismatch");
    assert_eq!(
        st.session.execute("library.loadInstalled", json!({})).unwrap(),
        json!({"loaded": [], "errors": [{"package": TAMPERED_ID, "error": "the installed file changed since it was installed (SHA-256 differs): install it again"}]})
    );
    assert!(effectcraft_effects::find(TAMPERED_ID).is_none());
}

fn the_plugins_folder_loads_its_modules_and_reports_the_rest() {
    let mut st = settings("plugins-dir");
    let folder = st.session.plugins_dir.clone().unwrap();
    let report = st.session.execute("effect.plugins.load", json!({"folder": folder.to_string_lossy()})).unwrap();
    assert_eq!(report["loaded"][0]["id"], json!(FOLDER_ID));
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["path"], json!(folder.join("not-a-module.wasm").to_string_lossy()));
    assert!(effectcraft_effects::find(FOLDER_ID).is_some());
}
