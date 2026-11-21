use effectcraft_engine::Session;
use effectcraft_project::{GroupKind, Node, Project, SavedView};
use serde_json::{Value, json};

use crate::Error;
use crate::modules::{ABSENT_ID, GAIN_ID, NEIGHBOUR_PROBE_ID, NEIGHBOURS_ID, PARAMS_PROBE_ID};

pub const SAVED_BY: &str = "0.24.1";
pub const GAIN_COMP: &str = "Gain";
pub const NEIGHBOURS_COMP: &str = "Neighbours";
pub const PARAMS_PROBE_COMP: &str = "Params Probe";
pub const NEIGHBOUR_PROBE_COMP: &str = "Neighbour Probe";

fn project_error(e: impl std::fmt::Display) -> Error {
    Error::Project(e.to_string())
}

fn register(id: &str, binary: &[u8]) -> Result<(), Error> {
    if effectcraft_effects::find(id).is_some() {
        return Ok(());
    }
    effectcraft_plugin::load_wasm(binary, id).map(|_| ()).map_err(Error::Module)
}

fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, Error> {
    s.execute(id, params).map_err(|e| Error::Project(format!("{id}: {e}")))
}

fn comp_with(s: &mut Session, name: &str, effect: &str) -> Result<u64, Error> {
    run(s, "comp.new", json!({"name": name, "width": 4, "height": 2, "frameRate": 10, "duration": 1}))?;
    let layer = run(s, "layer.newSolid", json!({"name": format!("{name} Solid"), "color": "#cc6633"}))?["layer"]
        .as_u64()
        .ok_or_else(|| Error::Project("layer.newSolid returned no layer".into()))?;
    run(s, "effect.apply", json!({"effect": effect, "layers": [layer]}))?;
    Ok(layer)
}

fn file_text(project: &Project) -> Result<Vec<u8>, Error> {
    craft_ecproj_format::File::new(SAVED_BY, &SavedView::default(), project).to_json().map(String::into_bytes).map_err(project_error)
}

fn rename_effect(project: &mut Project, from: &str, to: &str) {
    let ids: Vec<_> = project.comps().map(|(id, _)| *id).collect();
    for id in ids {
        let Some(comp) = project.comp_mut(id) else { continue };
        for layer in comp.layers.iter_mut() {
            let Some(effects) = layer.props.sub_mut("effects") else { continue };
            for node in &mut effects.children {
                if let Node::Group(group) = node
                    && matches!(&group.kind, GroupKind::Effect { effect } if effect == from)
                {
                    group.kind = GroupKind::Effect { effect: to.to_owned() };
                    group.match_id = to.to_owned();
                }
            }
        }
    }
}

pub fn projects(gain: &[u8], neighbours: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
    register(GAIN_ID, gain)?;
    register(NEIGHBOURS_ID, neighbours)?;
    let mut s = Session::default();
    let layer = comp_with(&mut s, GAIN_COMP, GAIN_ID)?;
    run(&mut s, "prop.set", json!({"layer": layer, "path": "effects/#1/gain", "value": 0.5}))?;
    let mut missing = Project::clone(&s.project);
    comp_with(&mut s, NEIGHBOURS_COMP, NEIGHBOURS_ID)?;
    rename_effect(&mut missing, GAIN_ID, ABSENT_ID);
    Ok((file_text(&s.project)?, file_text(&missing)?))
}

pub fn probe_project(params_probe: &[u8], neighbour_probe: &[u8]) -> Result<Vec<u8>, Error> {
    register(PARAMS_PROBE_ID, params_probe)?;
    register(NEIGHBOUR_PROBE_ID, neighbour_probe)?;
    let mut s = Session::default();
    run(&mut s, "file.projectSettings", json!({"bitDepth": 32}))?;
    let layer = comp_with(&mut s, PARAMS_PROBE_COMP, PARAMS_PROBE_ID)?;
    for (param, value) in [
        ("amount", json!(0.5)),
        ("turn", json!(45)),
        ("enabled", json!(false)),
        ("shape", json!(2)),
        ("centre", json!([3, 1])),
        ("tint", json!([0.4, 0.5, 0.6, 1.0])),
    ] {
        run(&mut s, "prop.set", json!({"layer": layer, "path": format!("effects/#1/{param}"), "value": value}))?;
    }
    let layer = comp_with(&mut s, NEIGHBOUR_PROBE_COMP, "ec.generate.fill")?;
    for (time, value) in [(0.0, json!([0.0, 0.0, 0.0, 1.0])), (1.0, json!([1.0, 0.5, 0.0, 1.0]))] {
        run(&mut s, "prop.addKey", json!({"layer": layer, "path": "effects/#1/color", "time": time, "value": value}))?;
    }
    run(&mut s, "effect.apply", json!({"effect": NEIGHBOUR_PROBE_ID, "layers": [layer]}))?;
    file_text(&s.project)
}
