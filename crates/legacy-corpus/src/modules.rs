use crate::Error;

pub const GAIN_ID: &str = "org.example.corpus.gain";
pub const NEIGHBOURS_ID: &str = "org.example.corpus.neighbours";
pub const LIBRARY_ID: &str = "org.example.corpus.library";
pub const TAMPERED_ID: &str = "org.example.corpus.tampered";
pub const FOLDER_ID: &str = "org.example.corpus.folder";
pub const ABSENT_ID: &str = "org.example.corpus.absent";

pub struct Module {
    pub text: String,
    pub binary: Vec<u8>,
}

const ALLOC: &str = r#"(func (export "ec_alloc") (param $n i32) (result i32)
    (local $need i32)
    (local.set $need
      (i32.sub (i32.shr_u (i32.add (local.get $n) (i32.const 131071)) (i32.const 16)) (memory.size)))
    (if (i32.gt_s (local.get $need) (i32.const 0))
      (then (drop (memory.grow (local.get $need)))))
    (i32.const 65536))"#;

const GAIN: &str = r#"(func (export "ec_render")
    (param $px i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $t f64) (param $scale f64)
    (result i32)
    (local $i i32) (local $end i32) (local $gain f32)
    (local.set $gain (f32.demote_f64 (f64.load (local.get $params))))
    (local.set $end (i32.add (local.get $px) (i32.mul (i32.mul (local.get $w) (local.get $h)) (i32.const 16))))
    (local.set $i (local.get $px))
    (block $done
      (loop $each
        (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
        (f32.store (local.get $i) (f32.mul (f32.load (local.get $i)) (local.get $gain)))
        (f32.store offset=4 (local.get $i) (f32.mul (f32.load offset=4 (local.get $i)) (local.get $gain)))
        (f32.store offset=8 (local.get $i) (f32.mul (f32.load offset=8 (local.get $i)) (local.get $gain)))
        (local.set $i (i32.add (local.get $i) (i32.const 16)))
        (br $each)))
    (i32.const 0))"#;

const NEIGHBOURS: &str = r#"(func (export "ec_render2")
    (param $px i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $t f64) (param $scale f64)
    (param $prev i32) (param $prev_t f64) (param $next i32) (param $next_t f64) (param $flags i32)
    (result i32)
    (local $at i32) (local $end i32)
    (local.set $end (i32.mul (i32.mul (local.get $w) (local.get $h)) (i32.const 16)))
    (block $done
      (loop $each
        (br_if $done (i32.ge_u (local.get $at) (local.get $end)))
        (f32.store (i32.add (local.get $px) (local.get $at)) (f32.load (i32.add (local.get $prev) (local.get $at))))
        (f32.store offset=4 (i32.add (local.get $px) (local.get $at)) (f32.div (f32.convert_i32_s (local.get $flags)) (f32.const 4)))
        (f32.store offset=8 (i32.add (local.get $px) (local.get $at)) (f32.load (i32.add (local.get $next) (local.get $at))))
        (f32.store offset=12 (i32.add (local.get $px) (local.get $at)) (f32.const 1))
        (local.set $at (i32.add (local.get $at) (i32.const 16)))
        (br $each)))
    (i32.const 0))"#;

fn module(abi: u32, manifest: &serde_json::Value, render: &str) -> Result<Module, Error> {
    let json = serde_json::to_string(manifest).map_err(|e| Error::Module(e.to_string()))?;
    let text = format!(
        r#"(module
  (memory (export "memory") 2)
  (data (i32.const 16) "{data}")
  (func (export "ec_api_version") (result i32)
    (i32.const {abi}))
  (func (export "ec_manifest_ptr") (result i32)
    (i32.const 16))
  (func (export "ec_manifest_len") (result i32)
    (i32.const {len}))
  {ALLOC}
  {render})
"#,
        data = json.replace('\\', "\\\\").replace('"', "\\\""),
        len = json.len(),
    );
    let binary = wat::parse_str(&text).map_err(|e| Error::Module(e.to_string()))?;
    Ok(Module { text, binary })
}

pub fn gain(id: &str, name: &str) -> Result<Module, Error> {
    module(
        1,
        &serde_json::json!({
            "api": 1,
            "id": id,
            "name": name,
            "category": "Corpus Plug-ins",
            "version": "1.0.0",
            "author": "EffectCraft legacy corpus",
            "description": "Multiplies colour by Gain.",
            "params": [{"id": "gain", "name": "Gain", "type": "slider", "default": 1.0, "min": 0.0, "max": 4.0}],
        }),
        GAIN,
    )
}

pub fn neighbours() -> Result<Module, Error> {
    module(
        2,
        &serde_json::json!({
            "api": 1,
            "id": NEIGHBOURS_ID,
            "name": "Corpus Neighbours",
            "category": "Corpus Plug-ins",
            "version": "1.0.0",
            "author": "EffectCraft legacy corpus",
            "description": "Shows the previous and next frames.",
            "params": [],
        }),
        NEIGHBOURS,
    )
}
