//! Operations plugins call, run on the thread that owns the session, inside the event's undo group.
//! Every document change goes through `Session::execute`, so undo, the journal and the command
//! guards stay the engine's. Ops act only on the composition and selection captured when the event
//! began (B3 captured scope).

use craft_ext_core::ui::tree::Node;
use craft_ext_host::api::{Code, OpError};
use effectcraft_engine::Session;
use effectcraft_project::{ItemId, LayerId};
use serde_json::{Value, json};

/// What an event captured at its start.
pub struct Scope {
    pub comp: Option<ItemId>,
    pub selection: Vec<LayerId>,
    /// The plugin's display name (notifications are prefixed with it).
    pub plugin_name: String,
}

/// UI changes an op asks for; the runtime applies them to its surfaces.
pub enum UiChange {
    Overlay(Value),
    Panel { panel: String, tree: Value },
    Window { id: String, title: String, tree: Value },
}

pub enum Outcome {
    Value(Value),
    Ui(UiChange),
}

const MAX_NAME: usize = 256;
const MAX_MESSAGE: usize = 300;
const MAX_EFFECT_PARAMS: usize = 64;

fn invalid(m: impl Into<String>) -> OpError {
    OpError::invalid(m)
}

fn engine(e: effectcraft_engine::EngineError) -> OpError {
    OpError::new(Code::Invalid, e.to_string())
}

fn comp(scope: &Scope) -> Result<ItemId, OpError> {
    scope.comp.ok_or_else(|| OpError::new(Code::NotFound, "no composition was active when this event started"))
}

fn text<'a>(args: &'a Value, key: &str, max: usize) -> Result<&'a str, OpError> {
    let s = args.get(key).and_then(Value::as_str).ok_or_else(|| invalid(format!("missing `{key}`")))?;
    if s.chars().count() > max || s.chars().any(|c| c.is_control() && c != '\n') {
        return Err(invalid(format!("`{key}` is too long or contains control characters")));
    }
    Ok(s)
}

fn layers_of(s: &Session, cid: ItemId, only: Option<&[LayerId]>) -> Result<Value, OpError> {
    let c = s.project.comp(cid).ok_or_else(|| OpError::new(Code::Stale, "the composition is gone"))?;
    let layers: Vec<Value> = c
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| only.is_none_or(|sel| sel.contains(&l.id)))
        .map(|(i, l)| json!({"id": l.id.0, "name": l.name, "index": i + 1}))
        .collect();
    Ok(json!({"layers": layers}))
}

/// Validates a UI tree (size, depth, labels) and returns it as JSON for the frontend.
pub fn checked_tree(tree: &Value) -> Result<Value, OpError> {
    let node = Node::parse(tree).map_err(|e| invalid(e.0))?;
    node.validate().map_err(|e| invalid(e.0))?;
    serde_json::to_value(&node).map_err(|e| OpError::new(Code::Internal, e.to_string()))
}

// Strings and objects could name an unpicked file (e.g. a LUT); document grants never grant filesystem access.
fn numeric_value(v: &Value) -> bool {
    v.is_boolean()
        || v.as_f64().is_some_and(f64::is_finite)
        || v.as_array().is_some_and(|a| a.len() <= 4 && a.iter().all(|v| v.as_f64().is_some_and(f64::is_finite)))
}

fn local_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Runs one op. `own_panels` are the plugin's declared panel ids.
pub fn run(s: &mut Session, scope: &Scope, op: &str, args: Value, own_panels: &[String]) -> Result<Outcome, OpError> {
    let v = match op {
        "canvas.overlay.set" => {
            let tree = checked_tree(
                &json!({"type":"canvas","id":"overlay","label":"Guides","width":1,"height":1,"ops":args.get("ops").cloned().unwrap_or(json!([]))}),
            )?;
            return Ok(Outcome::Ui(UiChange::Overlay(tree["ops"].clone())));
        }
        "marker.list" => s.execute("markers.list", json!({"comp":comp(scope)?.0})).map_err(engine)?,
        "prop.get" | "prop.set" | "prop.addKey" | "prop.reset" | "effect.remove" | "layer.newSolid" | "layer.newNull" | "layer.select" | "markers.set"
        | "markers.delete" | "effect.reorder" | "comp.settings" | "layer.tree" | "time.set" | "layer.addItem" | "layer.newText" | "layer.newShape"
        | "layer.setSwitch" | "comp.new" => {
            let mut args = args.as_object().filter(|a| a.len() <= 32).cloned().ok_or_else(|| invalid("arguments must be an object with at most 32 fields"))?;
            if serde_json::to_vec(&args).map_err(|e| invalid(e.to_string()))?.len() > 65536 {
                return Err(invalid("arguments exceed 64 KiB"));
            }
            args.remove("merge");
            if op == "comp.new" {
                args.remove("comp");
            } else {
                args.insert("comp".into(), json!(comp(scope)?.0));
            }
            if op.starts_with("prop.") {
                text(&Value::Object(args.clone()), "path", 512)?;
                if matches!(op, "prop.set" | "prop.addKey") && args.get("value").is_some_and(|v| !numeric_value(v)) {
                    return Err(invalid("property edits accept numeric, boolean or numeric vector values; file and text properties are not exposed"));
                }
                let layer = args.get("layer").and_then(Value::as_u64).ok_or_else(|| invalid("layer id required"))?;
                if !s.project.comp(comp(scope)?).is_some_and(|c| c.layer(LayerId(layer)).is_some()) {
                    return Err(invalid("layer not in captured composition"));
                }
            }
            s.execute(op, Value::Object(args)).map_err(engine)?
        }

        "project.summary" => s.execute("project.summary", json!({})).map_err(engine)?,
        "selection.get" => layers_of(s, comp(scope)?, Some(&scope.selection))?,
        "layer.list" => layers_of(s, comp(scope)?, None)?,
        "comp.info" => s.execute("comp.info", json!({"comp": comp(scope)?.0})).map_err(engine)?,
        "time.get" => json!({"time": s.time().seconds(), "comp": scope.comp.map(|c| c.0)}),
        "layer.rename" => {
            let cid = comp(scope)?;
            let layer = args.get("layer").and_then(Value::as_u64).ok_or_else(|| invalid("missing `layer` (a layer id)"))?;
            let name = text(&args, "name", MAX_NAME)?;
            if name.trim().is_empty() {
                return Err(invalid("the name is empty"));
            }
            if !s.project.comp(cid).is_some_and(|c| c.layer(LayerId(layer)).is_some()) {
                return Err(OpError::new(Code::NotFound, format!("no layer {layer} in the composition")));
            }
            s.execute("layer.rename", json!({"comp": cid.0, "layer": layer, "name": name})).map_err(engine)?;
            Value::Null
        }
        "marker.add" => {
            let cid = comp(scope)?;
            let label = text(&args, "label", MAX_NAME)?;
            let mut p = json!({"comp": cid.0, "comment": label});
            if let Some(t) = args.get("time") {
                let t = t.as_f64().filter(|t| t.is_finite() && *t >= 0.0).ok_or_else(|| invalid("`time` must be seconds ≥ 0"))?;
                p["time"] = json!(t);
            }
            s.execute("comp.addMarker", p).map_err(engine)?
        }
        "effect.apply" => {
            let cid = comp(scope)?;
            let effect = text(&args, "effect", 256)?;
            if effectcraft_engine::effects::find(effect).is_none() {
                return Err(OpError::new(Code::NotFound, format!("no effect `{effect}` (use an effect id)")));
            }
            let layers: Vec<u64> = scope.selection.iter().filter(|l| s.project.comp(cid).is_some_and(|c| c.layer(**l).is_some())).map(|l| l.0).collect();
            if layers.is_empty() {
                return Err(OpError::new(Code::NotFound, "no layer was selected when this event started"));
            }
            let params = match args.get("params") {
                None | Some(Value::Null) => serde_json::Map::new(),
                Some(Value::Object(m)) if m.len() <= MAX_EFFECT_PARAMS => m.clone(),
                Some(_) => return Err(invalid("`params` must be an object of at most 64 values")),
            };
            if params.values().any(|v| !numeric_value(v)) {
                return Err(invalid("effect parameters must be numeric, boolean or numeric vectors; file paths require host picking"));
            }
            let r = s.execute("effect.apply", json!({"comp": cid.0, "layers": layers, "effect": effect})).map_err(engine)?;
            let paths: Vec<Option<String>> = r["paths"].as_array().map(|a| a.iter().map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
            for (layer, path) in layers.iter().zip(&paths) {
                let Some(path) = path else { continue };
                for (k, value) in &params {
                    if !local_id(k) {
                        return Err(invalid(format!("bad parameter id `{k}`")));
                    }
                    s.execute("prop.set", json!({"comp": cid.0, "layer": layer, "path": format!("{path}/{k}"), "value": value})).map_err(engine)?;
                }
            }
            json!({"effects": r["effects"], "paths": r["paths"]})
        }
        "ui.notify" => {
            let message = text(&args, "message", MAX_MESSAGE)?;
            let error = args.get("error").and_then(Value::as_bool).unwrap_or(false);
            effectcraft_engine::ext::notify(s, format!("{}: {message}", scope.plugin_name), error);
            Value::Null
        }
        "ui.panel.set" => {
            let panel = text(&args, "panel", 48)?.to_string();
            if !own_panels.contains(&panel) {
                return Err(OpError::new(Code::NotFound, format!("this plugin declares no enabled panel `{panel}`")));
            }
            let tree = checked_tree(args.get("tree").unwrap_or(&Value::Null))?;
            return Ok(Outcome::Ui(UiChange::Panel { panel, tree }));
        }
        "ui.window.open" => {
            let id = text(&args, "id", 48)?.to_string();
            if !local_id(&id) {
                return Err(invalid("`id` must be letters, digits, - or _"));
            }
            let title = text(&args, "title", 64)?.to_string();
            let tree = checked_tree(args.get("tree").unwrap_or(&Value::Null))?;
            return Ok(Outcome::Ui(UiChange::Window { id, title, tree }));
        }
        _ => return Err(OpError::new(Code::NotFound, format!("EffectCraft has no operation {op}"))),
    };
    Ok(Outcome::Value(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v2_trees_and_invalid_trees() {
        assert!(checked_tree(&json!({"type":"tabs","id":"tabs","tabs":[{"id":"one","label":"One","children":[{"type":"table","id":"rows","label":"Rows","columns":[{"label":"Name"}],"rows":[]}]}]})).is_ok());
        assert!(checked_tree(&json!({"type":"table","id":"rows","columns":[],"rows":[]})).is_err());
    }
    #[test]
    fn file_values_are_not_document_edits() {
        assert!(!numeric_value(&json!("/etc/passwd")));
        assert!(!numeric_value(&json!({"path":"/etc/passwd"})));
        assert!(numeric_value(&json!([1, 2, 3, 4])));
        assert!(!numeric_value(&json!([1, 2, 3, 4, 5])));
    }
    #[test]
    fn property_keyframes_markers_and_group_undo() {
        let mut s = Session::default();
        let empty = Scope { comp: None, selection: vec![], plugin_name: "Test".into() };
        assert!(run(&mut s, &empty, "comp.new", json!({"width":32,"height":32}), &[]).is_ok());
        let id = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
        let scope = Scope { comp: s.active_comp_id(), selection: vec![LayerId(id)], plugin_name: "Test".into() };
        let mark = s.begin_group();
        run(&mut s, &scope, "prop.addKey", json!({"layer":id,"path":"transform/position","value":[10,20],"time":0}), &[]).unwrap_or_else(|e| panic!("{e}"));
        run(&mut s, &scope, "marker.add", json!({"label":"Cue","time":0}), &[]).unwrap_or_else(|e| panic!("{e}"));
        s.finish_group(mark, "Gesture", true);
        assert_eq!(s.history.undo.last().unwrap().0, "Gesture");
        assert_eq!(s.project.comp(scope.comp.unwrap()).unwrap().markers.len(), 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.project.comp(scope.comp.unwrap()).unwrap().markers.is_empty());
        assert!(run(&mut s, &scope, "prop.set", json!({"layer":9999,"path":"transform/position","value":[1,2]}), &[]).is_err());
        assert!(run(&mut s, &scope, "prop.addKey", json!({"layer":id,"path":"missing","value":0}), &[]).is_err());
    }
}
