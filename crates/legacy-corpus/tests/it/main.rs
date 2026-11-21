mod coverage;
mod parity;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use craft_testkit::corpus::Committed;
use effectcraft_engine::Session;
use effectcraft_engine::config::DirConfig;
use effectcraft_legacy_corpus::{GAIN_ID, NEIGHBOUR_PROBE_ID, NEIGHBOURS_ID, PARAMS_PROBE_ID};
use effectcraft_project::{GroupKind, ItemId, Node, Project};

pub fn corpus() -> &'static Committed {
    static CORPUS: std::sync::OnceLock<Committed> = std::sync::OnceLock::new();
    CORPUS.get_or_init(|| craft_testkit::corpus::open(&effectcraft_legacy_corpus::root()).unwrap())
}

pub fn loaded() {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        let corpus = corpus();
        effectcraft_plugin::load_wasm(&corpus.bytes("gain-effect", "gain.wasm").unwrap(), "gain.wasm").unwrap();
        effectcraft_plugin::load_wasm(&corpus.bytes("neighbours-effect", "neighbours.wasm").unwrap(), "neighbours.wasm").unwrap();
        effectcraft_plugin::load_wasm(&corpus.bytes("params-probe-effect", "params-probe.wasm").unwrap(), "params-probe.wasm").unwrap();
        effectcraft_plugin::load_wasm(&corpus.bytes("neighbour-probe-effect", "neighbour-probe.wasm").unwrap(), "neighbour-probe.wasm").unwrap();
    });
    for id in [GAIN_ID, NEIGHBOURS_ID, PARAMS_PROBE_ID, NEIGHBOUR_PROBE_ID] {
        assert!(effectcraft_effects::find(id).is_some(), "{id}");
    }
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
    let mut st = fresh_settings();
    corpus().copy_member(member, st.dir.path()).unwrap();
    st.session.load_settings();
    st
}

pub fn fresh_settings() -> Settings {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session {
        config: Some(Arc::new(DirConfig::new(dir.path()))),
        plugin_loader: Some(effectcraft_plugin::loader),
        plugin_previewer: Some(effectcraft_plugin::preview),
        plugins_dir: Some(dir.path().join("Plug-ins")),
        ..Default::default()
    };
    effectcraft_script::install(&mut session);
    Settings { session, dir }
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

pub fn colour_stream(pixels: &[[i32; 4]], n: usize) -> Vec<i32> {
    pixels.iter().flat_map(|px| px[..3].to_vec()).take(n).collect()
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
