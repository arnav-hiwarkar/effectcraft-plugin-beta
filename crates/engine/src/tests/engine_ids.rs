use craft_idmap::{Lock, Migration};
use effectcraft_project::{GroupKind, ItemId, LayerId};
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;
use crate::engine_ids::{EngineIds, LOCK, Site, migration_names, registered, simulate};

const OLD_EFFECT: &str = "ec.blur.gaussianBlur";
const EFFECT: &str = "ec.blur.gaussian";
const OLD_COMMAND: &str = "composition.new";
const COMMAND: &str = "comp.new";

fn name_from_title(mut params: Value) -> Result<Value, String> {
    if let Some(o) = params.as_object_mut()
        && !o.contains_key("name")
        && let Some(title) = o.remove("title")
    {
        o.insert("name".into(), title);
    }
    Ok(params)
}

const RENAMED: &[Migration<Value>] = &[Migration { name: "name-from-title", run: name_from_title }];

fn entry(id: &str) -> String {
    let lock = Lock::parse(LOCK).unwrap();
    let e = lock.entries().find(|e| e.id().as_str() == id).unwrap();
    format!("[[entry]]\nid = \"{id}\"\nclass = \"{}\"\nargs = \"{}\"\n", e.class(), e.args().unwrap())
}

fn renamed_lock() -> String {
    let (effect, command) = (entry(EFFECT), entry(COMMAND));
    let old_effect = effect.replacen(&format!("id = \"{EFFECT}\"\n"), &format!("id = \"{OLD_EFFECT}\"\ncurrent = \"{EFFECT}\"\n"), 1);
    let title = craft_idmap::ArgsHash::of_keys(["title", "width", "height"]);
    let old_command = format!(
        "{}migrations = [{{ from = \"{title}\", name = \"name-from-title\" }}]\n",
        command.replacen(&format!("id = \"{COMMAND}\"\n"), &format!("id = \"{OLD_COMMAND}\"\ncurrent = \"{COMMAND}\"\n"), 1)
    );
    LOCK.replace(&effect, &old_effect).replace(&command, &old_command).replacen("format = 1\n", "format = 1\ntombstones = [\"ec.blur.retired\"]\n", 1)
}

fn renamed<R>(f: impl FnOnce() -> R) -> R {
    simulate(EngineIds::new(&renamed_lock(), RENAMED).unwrap(), f)
}

fn blurred(effect: &str) -> (Session, ItemId, u64) {
    let mut s = Session::default();
    let cid =
        ItemId(s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 48, "frameRate": 30, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 24, "height": 24})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [a], "effect": EFFECT})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/blurriness", "value": 6})).unwrap();
    rename_effect(&mut s, cid, a, effect);
    (s, cid, a)
}

fn rename_effect(s: &mut Session, cid: ItemId, layer: u64, effect: &str) {
    let mut p = (*s.project).clone();
    let comp = p.comp_mut(cid).unwrap();
    let fx = comp.layer_mut(LayerId(layer)).unwrap().props.sub_mut("effects").unwrap();
    for n in &mut fx.children {
        if let effectcraft_project::Node::Group(g) = n {
            g.kind = GroupKind::Effect { effect: effect.into() };
        }
    }
    s.project = std::sync::Arc::new(p);
}

fn effect_ids(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            if o.get("kind") == Some(&json!("Effect"))
                && let Some(Value::String(id)) = o.get("effect")
            {
                out.push(id.clone());
            }
            o.values().for_each(|v| effect_ids(v, out));
        }
        Value::Array(a) => a.iter().for_each(|v| effect_ids(v, out)),
        _ => {}
    }
}

fn saved_effects(path: &str) -> Vec<String> {
    let mut out = vec![];
    effect_ids(&serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap(), &mut out);
    out
}

fn layer_effects(s: &Session, cid: ItemId, layer: u64) -> Vec<String> {
    let l = s.project.comp(cid).unwrap().layer(LayerId(layer)).unwrap();
    l.effects().unwrap().groups().filter_map(|g| if let GroupKind::Effect { effect } = &g.kind { Some(effect.clone()) } else { None }).collect()
}

fn frame(s: &mut Session, cid: ItemId) -> Vec<[f32; 4]> {
    s.render(cid, Tick::ZERO, RenderOpts::default()).data.clone()
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ec-engine-ids-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn project_effect_resolves() {
    let d = dir("project");
    let (old, cid, a) = blurred(OLD_EFFECT);
    let path = d.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.project.to_json()).unwrap();
    let mut s = Session::default();
    s.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(layer_effects(&s, cid, a), vec![EFFECT], "the open resolves the saved id");
    let (mut current, _, _) = blurred(EFFECT);
    let (mut plain, _, _) = blurred(EFFECT);
    plain.execute("prop.set", json!({"layer": a, "path": "effects/#1/blurriness", "value": 0})).unwrap();
    assert_eq!(frame(&mut s, cid), frame(&mut current, cid), "it renders as the current effect");
    assert_ne!(frame(&mut s, cid), frame(&mut plain, cid), "the blur ran");
    let resaved = d.join("resaved.ecproj").to_string_lossy().to_string();
    s.execute("file.save", json!({"path": resaved})).unwrap();
    assert_eq!(saved_effects(&resaved), vec![OLD_EFFECT], "saves keep the first-shipped id");
    assert_eq!(layer_effects(&s, cid, a), vec![EFFECT], "the open project keeps the current id");
    let _ = std::fs::remove_dir_all(d);
}

fn preset_effect_resolves() {
    let (source, cid, a) = blurred(EFFECT);
    let group = source.project.comp(cid).unwrap().layer(LayerId(a)).unwrap().effects().unwrap().groups().next().unwrap().clone();
    let mut fx = serde_json::to_value(&group).unwrap();
    fx["kind"]["effect"] = json!(OLD_EFFECT);
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 48, "frameRate": 30, "duration": 1})).unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 24, "height": 24})).unwrap()["layer"].as_u64().unwrap();
    let preset = json!({"effectcraftAnimationPreset": 4, "name": "Blur", "properties": [], "effects": [fx], "textAnimators": []});
    let r = s.execute("anim.applyPreset", json!({"preset": preset, "layers": [b]})).unwrap();
    assert_eq!((r["applied"].clone(), r["unresolved"].clone()), (json!(1), json!([])));
    let cid = s.state.active_comp.unwrap();
    assert_eq!(layer_effects(&s, cid, b), vec![EFFECT]);
}

fn comp_names(s: &Session) -> Vec<String> {
    s.project.comps().filter_map(|(id, _)| s.project.item(*id).map(|i| i.name.clone())).collect()
}

fn journal_resolves() {
    let mut s = Session::default();
    let script = format!("{{\"command\": \"{OLD_COMMAND}\", \"params\": {{\"title\": \"Replayed\", \"width\": 32, \"height\": 32}}}}\n");
    let report = crate::replay::run(&mut s, &script, &crate::replay::Options::default());
    assert!(report.ok, "{:?}", report.first_failure);
    assert_eq!(comp_names(&s), vec!["Replayed"]);
}

fn script_resolves() {
    let mut s = Session::default();
    s.execute_for_script(OLD_COMMAND, json!({"title": "Scripted", "width": 32, "height": 32})).unwrap();
    assert_eq!(comp_names(&s), vec!["Scripted"]);
}

#[test]
fn every_load_path_resolves_a_renamed_id_and_argument() {
    renamed(|| {
        for site in Site::ALL {
            match site {
                Site::ProjectEffect => project_effect_resolves(),
                Site::PresetEffect => preset_effect_resolves(),
                Site::Journal => journal_resolves(),
                Site::Script => script_resolves(),
            }
        }
    });
}

#[test]
fn the_simulated_rename_is_a_lock_the_gate_accepts() {
    let registry: Vec<_> = registered().unwrap().into_iter().filter(|r| r.id.as_str() != "test.panic").collect();
    let lock = Lock::parse(&renamed_lock()).unwrap();
    let mut names = migration_names();
    names.push("name-from-title");
    assert_eq!(craft_idmap::check(&lock, &registry, &names), vec![]);
}

#[test]
fn unknown_and_retired_ids_stay_inert_with_a_diagnostic() {
    renamed(|| {
        let d = dir("inert");
        let (old, cid, a) = blurred("ec.blur.retired");
        let path = d.join("retired.ecproj").to_string_lossy().to_string();
        std::fs::write(&path, old.project.to_json()).unwrap();
        let mut s = Session::default();
        s.execute("file.open", json!({"path": path})).unwrap();
        assert_eq!(layer_effects(&s, cid, a), vec!["ec.blur.retired"], "the saved id is kept");
        let resaved = d.join("resaved.ecproj").to_string_lossy().to_string();
        s.execute("file.save", json!({"path": resaved})).unwrap();
        assert_eq!(saved_effects(&resaved), vec!["ec.blur.retired"]);
        let mut p = (*old.project).clone();
        let upgraded = crate::upgrade_effects(&mut p);
        assert_eq!(upgraded.unresolved_text().as_deref(), Some("kept as saved, without rendering: project effect: effect `ec.blur.retired` was retired"));
        let _ = std::fs::remove_dir_all(d);

        let mut s = Session::default();
        let e = s.execute_for_script("comp.never", json!({})).unwrap_err().to_string();
        assert_eq!(e, "script command: unknown command `comp.never`");
        assert!(s.project.comps().next().is_none());
    });
}
