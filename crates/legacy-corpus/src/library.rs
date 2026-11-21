use std::collections::BTreeMap;

use effectcraft_engine::library::{CATALOG_SCHEMA, CATALOG_URL, Catalog, Installed, InstalledPackage, Package, Release};
use serde::Serialize;

use crate::Error;
use crate::modules;

pub const CATALOG_FETCHED: &str = "2026-11-21T00:00:00+00:00";
pub const MISMATCHED_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const VERSION: &str = "1.0.0";

#[derive(Serialize)]
struct Cache<'a> {
    url: &'a str,
    fetched: &'a str,
    catalog: Catalog,
}

fn settings_error(e: impl std::fmt::Display) -> Error {
    Error::Settings(e.to_string())
}

pub fn layout(id: &str, mismatched: bool) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let module = modules::gain(id, &format!("Corpus Library {id}"))?.binary;
    let sha256 = craft_testkit::corpus::sha256_hex(&module);
    let file = format!("{id}/{VERSION}/{id}.wasm");
    let release = Release {
        version: VERSION.into(),
        api: 1,
        min_app: None,
        url: format!("https://github.com/example/corpus/releases/download/{VERSION}/{id}.wasm"),
        sha256: sha256.clone(),
        size: Some(module.len() as u64),
        effects: vec![id.to_owned()],
    };
    let package = Package {
        id: id.to_owned(),
        name: format!("Corpus Library {id}"),
        author: "EffectCraft legacy corpus".into(),
        description: "A Plug-in Library package for the legacy corpus.".into(),
        license: "MIT OR Apache-2.0".into(),
        repo: "https://github.com/example/corpus".into(),
        tags: vec!["corpus".into()],
        versions: vec![release],
        preview: None,
    };
    let listed = if mismatched { MISMATCHED_SHA256.to_owned() } else { sha256 };
    let installed = Installed {
        packages: BTreeMap::from([(
            id.to_owned(),
            InstalledPackage { version: VERSION.into(), file: file.clone(), sha256: listed, effects: vec![id.to_owned()] },
        )]),
    };
    let cache = Cache { url: CATALOG_URL, fetched: CATALOG_FETCHED, catalog: Catalog { schema: CATALOG_SCHEMA, packages: vec![package] } };
    Ok(vec![
        (format!("Plug-ins/Library/{file}"), module),
        ("Plug-ins/Library/installed.json".into(), serde_json::to_vec_pretty(&installed).map_err(settings_error)?),
        ("Plug-ins/Library/catalog.json".into(), serde_json::to_vec(&cache).map_err(settings_error)?),
    ])
}
