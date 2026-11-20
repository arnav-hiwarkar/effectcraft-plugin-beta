use craft_testkit::pair::Arm;
use serde_json::json;

use super::*;

fn hz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

fn identity() -> Identity {
    Identity {
        run: "r1".into(),
        app_pin: ident("f32ad3aa").unwrap(),
        fw_rev: ident("5a7a63a2").unwrap(),
        os: ident("macos-aarch64").unwrap(),
        runner_image: ident("macos15-20251110.1").unwrap(),
        night: ident("2026-11-21").unwrap(),
        refresh_hz: hz(60),
    }
}

fn frame(t: f64, logic: f64, latency: Value, played: u64, late: u64, seek: Value) -> Value {
    json!({"t": t, "inputMs": 0.5, "logicMs": logic, "buildMs": 4.0, "uploadMs": 0.5, "latencyMs": latency, "played": played, "late": late, "seekMs": seek})
}

fn report(workload: &str) -> Value {
    json!({
        "context": {"workload": workload, "backend": "cpu", "scale": "1920x1080@30fps-1920x1080", "refresh_hz": 60},
        "frames": [
            frame(0.0, 1.0, Value::Null, 0, 0, Value::Null),
            frame(16.667, 1.0, json!(12.0), 2, 1, json!(40.0)),
            frame(33.333, 1.0, Value::Null, 1, 0, Value::Null),
            frame(75.0, 33.0, Value::Null, 0, 0, Value::Null),
        ]
    })
}

fn lines(workload: &str) -> String {
    let report = report(workload);
    let probe = probe(&report, hz(60)).unwrap();
    let workload_id = super::workload(workload, &report["context"], "idle").unwrap();
    let block = Block { unit: 1, position: 2, arm: Arm::AA };
    pair::print_records(&frames::records(&identity(), workload, &workload_id, block, &probe, &gates(workload)).unwrap())
}

fn expected(workload: &str, fixture: &str, gates: &[(&str, &str)]) -> String {
    gates
        .iter()
        .map(|(gate, samples)| {
            format!(
                "craft-pair/1 test_id=CONF-calibrate run_id=r1.{workload}.1.2.{gate} app_pin=f32ad3aa fw_rev=5a7a63a2 os=macos-aarch64 \
                 runner_image=macos15-20251110.1 night=2026-11-21 fixture_hash={fixture} mode=native backend=cpu load=idle \
                 scale=1920x1080@30fps-1920x1080 block=1 arm=AA gate={gate} refresh_hz=60 samples={samples}\n"
            )
        })
        .collect()
}

#[test]
fn a_scrub_report_emits_exact_record_lines_with_input_and_seek_latency() {
    let frame_gates =
        [("OverheadCpu", "hist:860:3,1033:1"), ("OverheadInterval", "hist:960:2,1040:1"), ("UiEqMissed", "count:1/3"), ("UiEqLongStalls", "count:0/3")];
    let mut gates = frame_gates.to_vec();
    gates.extend([("UiEqLatency", "hist:924:1"), ("PlaybackSeek", "hist:1037:1")]);
    assert_eq!(lines("scrub"), expected("scrub", "956bb28b25351f1b", &gates));
}

#[test]
fn a_playback_report_counts_late_comp_frames_against_the_sound() {
    let gates = [
        ("OverheadCpu", "hist:860:3,1033:1"),
        ("OverheadInterval", "hist:960:2,1040:1"),
        ("UiEqMissed", "count:1/3"),
        ("UiEqLongStalls", "count:0/3"),
        ("PlaybackLate", "count:1/3"),
    ];
    assert_eq!(lines("playback"), expected("playback", "36ed0dd699594313", &gates));
}

#[test]
fn a_malformed_frame_is_an_error() {
    let late = json!({"frames": [frame(0.0, 1.0, Value::Null, 1, 2, Value::Null)]});
    let missing = json!({"frames": [{"t": 0.0}]});
    assert!(probe(&late, hz(60)).unwrap_err().starts_with("more late than played frames"));
    assert!(probe(&missing, hz(60)).unwrap_err().starts_with("frame without a number `inputMs`"));
    assert_eq!(probe(&json!({}), hz(60)).map(|p| p.intervals), Err("the report has no frames".into()));
}

#[test]
fn the_framework_rev_is_the_craft_ui_source_commit() {
    let lock = r#"
version = 4

[[package]]
name = "craft-ui-kit"
version = "0.1.0"
source = "git+https://github.com/storytold/craft-libs?rev=aaaa#aaaa"

[[package]]
name = "craft-ui"
version = "0.1.0"
source = "git+https://github.com/storytold/craft-libs?rev=5a7a63a2#5a7a63a2"
dependencies = [
 "egui",
]

[[package]]
name = "egui"
"#;
    assert_eq!(framework_rev(lock).as_deref(), Some("5a7a63a2"));
    assert_eq!(framework_rev("[[package]]\nname = \"craft-ui\"\nversion = \"0.1.0\"\n\n[[package]]\nsource = \"git+x#bbbb\"\n"), None);
    assert_eq!(framework_rev("version = 4\n"), None);
}

#[test]
fn the_load_label_follows_load_per_core() {
    let label = |one_minute: f64| load_label(contended(Some([one_minute, 0.0, 0.0]), Some(8)));
    assert_eq!((label(8.0), label(8.5), load_label(contended(None, Some(8)))), ("idle", "contended", "idle"));
}

#[test]
fn clearing_removes_stale_records_and_block_reports() {
    let dir = tempfile::tempdir().unwrap();
    let records = dir.path().join(RECORDS);
    let blocks = dir.path().join("pair");
    std::fs::write(&records, "craft-pair/1 stale\n").unwrap();
    std::fs::create_dir_all(&blocks).unwrap();
    std::fs::write(blocks.join("scrub-1-0.json"), "{}").unwrap();
    for stale in [&records, &blocks, &dir.path().join(SUMMARY)] {
        clear(stale).unwrap();
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn options() {
    let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    let o = parse_opts(&args(&["--pair"])).unwrap();
    assert_eq!((o.workloads, o.units, o.frames, o.refresh_hz.get(), o.skip_build), (vec!["playback".to_owned(), "scrub".to_owned()], 3, 3600, 60, false));
    let o = parse_opts(&args(&[
        "--pair",
        "--workload",
        "scrub",
        "--units",
        "5",
        "--frames",
        "600",
        "--refresh-hz",
        "120",
        "--night",
        "n1",
        "--runner-image",
        "img",
        "--skip-build",
    ]))
    .unwrap();
    assert_eq!((o.workloads, o.units, o.frames, o.refresh_hz.get()), (vec!["scrub".to_owned()], 5, 600, 120));
    assert_eq!((o.night.as_deref(), o.runner_image.as_deref(), o.skip_build), (Some("n1"), Some("img"), true));
    for bad in [&["--units", "0"][..], &["--frames", "x"], &["--frames", "1"], &["--refresh-hz", "0"], &["--workload", "drag"], &["--quick"]] {
        assert!(parse_opts(&args(bad)).is_err(), "{bad:?}");
    }
}
