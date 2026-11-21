mod coverage;
mod parity;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use craft_testkit::corpus::Committed;
use effectcraft_engine::Session;
use effectcraft_engine::config::DirConfig;
use effectcraft_legacy_corpus::{GAIN_ID, NEIGHBOURS_ID};
use effectcraft_project::{GroupKind, ItemId, Node, Project};

pub fn corpus() -> Committed {
    craft_testkit::corpus::open(&effectcraft_legacy_corpus::root()).unwrap()
}

pub fn loaded() {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        let corpus = corpus();
        effectcraft_plugin::load_wasm(&corpus.bytes("gain-effect", "gain.wasm").unwrap(), "gain.wasm").unwrap();
        effectcraft_plugin::load_wasm(&corpus.bytes("neighbours-effect", "neighbours.wasm").unwrap(), "neighbours.wasm").unwrap();
    });
    assert!(effectcraft_effects::find(GAIN_ID).is_some() && effectcraft_effects::find(NEIGHBOURS_ID).is_some());
}

pub struct Settings {
    pub session: Session,
    pub dir: tempfile::TempDir,
}

impl Settings {
    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }
}

pub fn settings(member: &str) -> Settings {
    let dir = tempfile::tempdir().unwrap();
    copy_member(member, dir.path());
    let mut session = Session {
        config: Some(Arc::new(DirConfig::new(dir.path()))),
        plugin_loader: Some(effectcraft_plugin::loader),
        plugin_previewer: Some(effectcraft_plugin::preview),
        plugins_dir: Some(dir.path().join("Plug-ins")),
        ..Default::default()
    };
    effectcraft_script::install(&mut session);
    session.load_settings();
    Settings { session, dir }
}

pub fn copy_member(member: &str, to: &Path) {
    let corpus = corpus();
    let entry = corpus.manifest().entry(member).unwrap().clone();
    for file in &entry.files {
        let target = to.join(&file.path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, corpus.bytes(member, &file.path).unwrap()).unwrap();
    }
}

pub fn open_project(member: &str) -> Session {
    let mut s = Session::default();
    let path = corpus().path(member, "project.ecproj").unwrap();
    s.execute("file.open", serde_json::json!({"path": path.to_string_lossy()})).unwrap();
    s
}

pub fn comp(s: &Session, name: &str) -> ItemId {
    s.project.comps().map(|(id, _)| *id).find(|id| s.project.item(*id).is_some_and(|item| item.name == name)).unwrap()
}

pub fn thousandths(s: &Session, comp_name: &str, seconds: f64) -> Vec<[i32; 4]> {
    let image = s.render(comp(s, comp_name), effectcraft_time::Tick::from_seconds_f64(seconds), effectcraft_render::RenderOpts::default());
    image.data.iter().map(|px| px.map(|v| (v * 1000.0).round() as i32)).collect()
}

pub fn effect_ids(project: &Project) -> Vec<String> {
    let mut ids = Vec::new();
    for (_, comp) in project.comps() {
        for layer in comp.layers.iter() {
            for node in layer.effects().map(|fx| fx.children.as_slice()).unwrap_or_default() {
                if let Node::Group(group) = node
                    && let GroupKind::Effect { effect } = &group.kind
                {
                    ids.push(effect.clone());
                }
            }
        }
    }
    ids.sort();
    ids
}
