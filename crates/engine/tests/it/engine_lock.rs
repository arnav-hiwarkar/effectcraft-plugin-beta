use craft_idmap::Request;
use effectcraft_engine::engine_ids::{LOCK, migration_names, registered};

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugin-contract/engine.lock");

#[test]
fn engine_lock_lists_every_shipped_id() {
    let args = std::env::var("CRAFT_ENGINE_LOCK_ARGS").unwrap_or_default();
    let request = Request::from_args(&args.lines().collect::<Vec<_>>()).unwrap_or_else(|e| panic!("cargo xtask engine-lock: {e}"));
    match request.run(LOCK, &registered().unwrap(), &migration_names()) {
        Ok(Some(text)) => std::fs::write(PATH, text).unwrap(),
        Ok(None) => {}
        Err(problems) => panic!("plugin-contract/engine.lock does not match the registry ({} findings):\n{}", problems.len(), problems.join("\n")),
    }
}
