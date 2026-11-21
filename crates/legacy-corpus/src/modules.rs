use crate::Error;

pub const GAIN_ID: &str = "org.example.corpus.gain";
pub const NEIGHBOURS_ID: &str = "org.example.corpus.neighbours";
pub const LIBRARY_ID: &str = "org.example.corpus.library";
pub const TAMPERED_ID: &str = "org.example.corpus.tampered";
pub const FOLDER_ID: &str = "org.example.corpus.folder";
pub const ABSENT_ID: &str = "org.example.corpus.absent";
pub const PARAMS_PROBE_ID: &str = "org.example.corpus.params-probe";
pub const NEIGHBOUR_PROBE_ID: &str = "org.example.corpus.neighbour-probe";
pub const BUNDLE_ID: &str = "org.example.corpus.bundle";
pub const DEFAULT_DIR_ID: &str = "org.example.corpus.default";

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

const PUT: &str = r#"(func $put (param $px i32) (param $w i32) (param $h i32) (param $k i32) (param $v f32)
    (if (i32.lt_u (local.get $k) (i32.mul (i32.mul (local.get $w) (local.get $h)) (i32.const 3)))
      (then (f32.store
        (i32.add (local.get $px)
          (i32.add (i32.mul (i32.div_u (local.get $k) (i32.const 3)) (i32.const 16)) (i32.mul (i32.rem_u (local.get $k) (i32.const 3)) (i32.const 4))))
        (local.get $v)))))"#;

const PARAMS_PROBE: &str = r#"(func (export "ec_render")
    (param $px i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $t f64) (param $scale f64)
    (result i32)
    (local $i i32)
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 0) (f32.convert_i32_s (local.get $np)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 1) (f32.convert_i32_s (local.get $w)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 2) (f32.convert_i32_s (local.get $h)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 3) (f32.demote_f64 (local.get $t)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 4) (f32.demote_f64 (local.get $scale)))
    (block $done
      (loop $each
        (br_if $done (i32.ge_s (local.get $i) (local.get $np)))
        (call $put (local.get $px) (local.get $w) (local.get $h) (i32.add (local.get $i) (i32.const 5))
          (f32.demote_f64 (f64.load (i32.add (local.get $params) (i32.mul (local.get $i) (i32.const 8))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $each)))
    (i32.const 0))"#;

const NEIGHBOUR_PROBE: &str = r#"(func (export "ec_render2")
    (param $px i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $t f64) (param $scale f64)
    (param $prev i32) (param $prev_t f64) (param $next i32) (param $next_t f64) (param $flags i32)
    (result i32)
    (local $prev_r f32) (local $cur_r f32) (local $next_r f32) (local $prev_g f32) (local $cur_g f32) (local $next_g f32)
    (local.set $prev_r (f32.load (local.get $prev)))
    (local.set $cur_r (f32.load (local.get $px)))
    (local.set $next_r (f32.load (local.get $next)))
    (local.set $prev_g (f32.load offset=4 (local.get $prev)))
    (local.set $cur_g (f32.load offset=4 (local.get $px)))
    (local.set $next_g (f32.load offset=4 (local.get $next)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 0) (f32.demote_f64 (local.get $t)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 1) (f32.demote_f64 (local.get $prev_t)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 2) (f32.demote_f64 (local.get $next_t)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 3) (f32.convert_i32_s (local.get $flags)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 4) (f32.convert_i32_s (local.get $w)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 5) (f32.convert_i32_s (local.get $h)))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 6) (local.get $prev_r))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 7) (local.get $cur_r))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 8) (local.get $next_r))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 9) (local.get $prev_g))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 10) (local.get $cur_g))
    (call $put (local.get $px) (local.get $w) (local.get $h) (i32.const 11) (local.get $next_g))
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

pub fn params_probe() -> Result<Module, Error> {
    module(
        1,
        &serde_json::json!({
            "api": 1,
            "id": PARAMS_PROBE_ID,
            "name": "Corpus Params Probe",
            "category": "Corpus Plug-ins",
            "version": "1.0.0",
            "author": "EffectCraft legacy corpus",
            "description": "Writes the flattened parameters, frame size, time and scale into the pixels.",
            "params": [
                {"id": "amount", "name": "Amount", "type": "slider", "default": 0.25, "min": 0.0, "max": 1.0},
                {"id": "turn", "name": "Turn", "type": "angle", "default": 30.0},
                {"id": "enabled", "name": "Enabled", "type": "checkbox", "default": true},
                {"id": "shape", "name": "Shape", "type": "popup", "options": ["Circle", "Square", "Star"], "default": 1},
                {"id": "centre", "name": "Centre", "type": "point", "default": [0.25, 0.75]},
                {"id": "tint", "name": "Tint", "type": "color", "default": [0.1, 0.2, 0.3, 1.0]},
            ],
        }),
        &format!("{PUT}\n  {PARAMS_PROBE}"),
    )
}

pub fn neighbour_probe() -> Result<Module, Error> {
    module(
        2,
        &serde_json::json!({
            "api": 1,
            "id": NEIGHBOUR_PROBE_ID,
            "name": "Corpus Neighbour Probe",
            "category": "Corpus Plug-ins",
            "version": "1.0.0",
            "author": "EffectCraft legacy corpus",
            "description": "Writes the frame times, flags and neighbour pixels into the pixels.",
            "params": [],
        }),
        &format!("{PUT}\n  {NEIGHBOUR_PROBE}"),
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
