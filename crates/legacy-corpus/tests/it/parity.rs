use effectcraft_engine::library::CATALOG_URL;
use effectcraft_legacy_corpus::{
    ABSENT_ID, CATALOG_FETCHED, FOLDER_ID, GAIN_COMP, GAIN_ID, LIBRARY_ID, NEIGHBOURS_COMP, NEIGHBOURS_ID, PANEL, SCRIPT, SECTION, SUBMENU_SCRIPT, TAMPERED_ID,
};
use effectcraft_project::Project;
use serde_json::{Value, json};

use crate::{corpus, effect_ids, loaded, open_project, settings, thousandths};

const SOLID: [[f32; 4]; 2] = [[0.8, 0.4, 0.2, 1.0]; 2];

fn preview(member: &str, file: &str) -> (Value, Vec<Vec<[f32; 4]>>) {
    effectcraft_plugin::preview(&corpus().bytes(member, file).unwrap(), 2, 1, &SOLID, &[0.0]).unwrap()
}

fn setting(s: &mut effectcraft_engine::Session, key: &str) -> Value {
    s.execute("script.settings.get", json!({"section": SECTION, "key": key})).unwrap()
}

#[test]
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

#[test]
fn the_abi2_effect_gets_zeroed_neighbours_without_a_host() {
    let (manifest, frames) = preview("neighbours-effect", "neighbours.wasm");
    assert_eq!(manifest["id"], json!(NEIGHBOURS_ID));
    assert_eq!(frames, vec![vec![[0.0, 0.0, 0.0, 1.0]; 2]]);
    loaded();
    assert!(effectcraft_effects::plugin::reads_other_times(NEIGHBOURS_ID));
    assert!(!effectcraft_effects::plugin::reads_other_times(GAIN_ID));
}

#[test]
fn the_project_renders_its_plugin_effects() {
    loaded();
    let s = open_project("project");
    assert_eq!(effect_ids(&s.project), vec![GAIN_ID.to_owned(), NEIGHBOURS_ID.to_owned()]);
    assert_eq!(thousandths(&s, GAIN_COMP, 0.0), vec![[400, 200, 102, 1000]; 16]);
    for seconds in [0.0, 0.1] {
        assert_eq!(thousandths(&s, NEIGHBOURS_COMP, seconds), vec![[800, 749, 800, 1000]; 16], "{seconds} s");
    }
}

#[test]
fn a_missing_effect_is_kept_and_renders_the_bare_solid() {
    let mut s = open_project("missing-effect-project");
    assert!(effectcraft_effects::find(ABSENT_ID).is_none());
    assert_eq!(effect_ids(&s.project), vec![ABSENT_ID.to_owned()]);
    assert_eq!(thousandths(&s, GAIN_COMP, 0.0), vec![[800, 400, 200, 1000]; 16]);
    assert_eq!(s.execute("library.missing", json!({})).unwrap(), json!({"effects": [{"effect": ABSENT_ID, "packages": []}]}));
    let resaved = Project::from_json(&s.project.to_file_json().unwrap()).unwrap();
    assert_eq!(effect_ids(&resaved), vec![ABSENT_ID.to_owned()]);
}

#[test]
fn installed_scripts_are_listed_and_run() {
    let mut st = settings("scripts");
    let list = st.session.execute("file.scripts.list", json!({})).unwrap();
    let installed: Vec<&str> = list.as_array().unwrap().iter().filter(|e| e["source"] == "installed").map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(installed, vec![SCRIPT, SUBMENU_SCRIPT]);
    st.session.execute("file.runScript", json!({"name": SCRIPT})).unwrap();
    assert!(st.session.project.items.values().any(|item| item.name == "Corpus Script Comp"));
}

#[test]
fn remembered_scriptui_panels_reopen() {
    let mut st = settings("scriptui-panels");
    assert_eq!(st.session.execute("window.restoreScriptPanels", json!({})).unwrap(), json!({"opened": [PANEL], "failed": []}));
}

#[test]
fn script_settings_load_from_the_settings_folder() {
    let mut st = settings("script-settings");
    assert_eq!(setting(&mut st.session, "greeting"), json!({"have": true, "value": "hello"}));
}

#[test]
fn startup_scripts_run() {
    let mut st = settings("startup-scripts");
    let ran = st.session.execute("file.runStartupScripts", json!({})).unwrap();
    assert_eq!((&ran["ran"], &ran["failed"]), (&json!(["Corpus Startup.jsx"]), &json!([])));
    assert_eq!(setting(&mut st.session, "startup"), json!({"have": true, "value": "ran"}));
}

#[test]
fn shutdown_scripts_run_and_their_settings_are_kept() {
    let mut st = settings("shutdown-scripts");
    let ran = st.session.execute("file.runShutdownScripts", json!({})).unwrap();
    assert_eq!((&ran["ran"], &ran["failed"]), (&json!(["Corpus Shutdown.jsx"]), &json!([])));
    let kept: Value = serde_json::from_slice(&std::fs::read(st.path("script_settings.json")).unwrap()).unwrap();
    assert_eq!(kept, json!({"Corpus": {"shutdown": "ran"}}));
}

#[test]
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

#[test]
fn a_mismatched_library_install_is_refused() {
    let mut st = settings("library-mismatch");
    assert_eq!(
        st.session.execute("library.loadInstalled", json!({})).unwrap(),
        json!({"loaded": [], "errors": [{"package": TAMPERED_ID, "error": "the installed file changed since it was installed (SHA-256 differs): install it again"}]})
    );
    assert!(effectcraft_effects::find(TAMPERED_ID).is_none());
}

#[test]
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
