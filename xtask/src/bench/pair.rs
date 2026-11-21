use std::num::NonZeroU32;
use std::path::Path;
use std::process::Command;

use craft_testkit::pair::frames::{self, Block, Identity, Opts, Output, Probe, Provenance, Run};
use craft_testkit::pair::{Count, Gate, Ident, Workload};
use serde_json::Value;

const BENCH: &str = "frame_probe_bench";
const PACKAGE: &str = "effectcraft-ui-egui";
const WORKLOADS: [&str; 2] = ["playback", "scrub"];
const STAGES: [&str; 4] = ["input", "logic", "ui_build", "upload"];
const MAX_LOAD_PER_CORE: f64 = 1.0;
const FRAMEWORK: &str = "craft-ui";

pub const HELP: &str = "usage: cargo xtask bench --pair [--workload playback|scrub]... [--units N] [--frames N] [--refresh-hz HZ]
                             [--night ID] [--runner-image ID] [--skip-build]

Runs the frame_probe_bench example in ABBA units of A and AA blocks (the same binary in both arms: the
A/A calibration CONF-calibrate), each block its own process, and writes one craft-pair/1 record per gate,
arm and block to target/bench/pair.records, the A/AA verdict per gate and each stage's p50/p95/p99 to
target/bench/pair-summary.md, and prints them. See docs/testing.md \"Paired frame runs\".

  --workload W      playback or scrub, repeatable (default: both)
  --units N         ABBA units (default 3)
  --frames N        frames a block, at least 2 (default 3600, a minute at 60 Hz)
  --refresh-hz HZ   the display grid the bench steps frames on (default 60)
  --night ID        the night records name (default the UTC date)
  --runner-image ID the runner image records name (default $ImageOS-$ImageVersion, else local)
  --skip-build      use the example already built in target/bench/release/examples";

fn gates(workload: &str) -> Vec<Gate> {
    let mut gates = vec![Gate::OverheadCpu, Gate::OverheadInterval, Gate::UiEqMissed, Gate::UiEqLongStalls];
    match workload {
        "scrub" => gates.extend([Gate::UiEqLatency, Gate::PlaybackSeek]),
        _ => gates.push(Gate::PlaybackLate),
    }
    gates
}

fn ident(text: &str) -> Result<Ident, String> {
    frames::ident(text).map_err(|e| e.to_string())
}

fn sample(frame: &Value) -> Result<frames::Frame<4>, String> {
    let number = |key: &str| frame.get(key).and_then(Value::as_f64).ok_or_else(|| format!("frame without a number `{key}`: {frame}"));
    let took = |key: &str| number(key).and_then(|ms| frames::millis(ms).ok_or_else(|| format!("frame with `{key}` {ms} ms, not a time: {frame}")));
    let optional = |key: &str| match frame.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => took(key).map(Some),
    };
    let count = |key: &str| frame.get(key).and_then(Value::as_u64).ok_or_else(|| format!("frame without a count `{key}`: {frame}"));
    Ok(frames::Frame {
        start: took("t")?,
        stages: [took("inputMs")?, took("logicMs")?, took("buildMs")?, took("uploadMs")?],
        latency: optional("latencyMs")?,
        playback: Count::new(count("late")?, count("played")?).ok_or_else(|| format!("more late than played frames: {frame}"))?,
        seek: optional("seekMs")?,
    })
}

pub fn probe(report: &Value, refresh_hz: NonZeroU32) -> Result<Probe<4>, String> {
    let frames = report["frames"].as_array().ok_or("the report has no frames")?.iter().map(sample).collect::<Result<Vec<_>, _>>()?;
    Ok(Probe::of(&frames, refresh_hz))
}

pub fn workload(name: &str, context: &Value, load: &str) -> Result<Workload, String> {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{BENCH}:{name}").as_bytes());
    let fixture: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let text = |key: &str| context[key].as_str().ok_or_else(|| format!("the report's context has no `{key}`"));
    Ok(Workload {
        fixture_hash: ident(&fixture)?,
        mode: ident("native")?,
        backend: ident(text("backend")?)?,
        load: ident(load)?,
        scale: ident(text("scale")?)?,
    })
}

pub fn framework_rev(lock: &str) -> Option<String> {
    let mut lines = lock.lines().map(str::trim);
    lines.find(|line| *line == format!("name = \"{FRAMEWORK}\""))?;
    lines
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.strip_prefix("source = \""))
        .and_then(|source| source.trim_end_matches('"').rsplit_once('#'))
        .map(|(_, rev)| rev.to_owned())
}

pub fn contended(load: Option<[f64; 3]>, cores: Option<usize>) -> bool {
    load.zip(cores).is_some_and(|([one, _, _], cores)| one > cores as f64 * MAX_LOAD_PER_CORE)
}

fn hosted_image() -> Option<String> {
    Some(format!("{}-{}", std::env::var("ImageOS").ok()?, std::env::var("ImageVersion").ok()?))
}

fn identity(root: &Path, o: &Opts) -> Result<Identity, String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let now = super::unix_now();
    let commit = git(&["rev-parse", "HEAD"]);
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("read Cargo.lock: {e}"))?;
    let framework = framework_rev(&lock);
    let date = super::utc(now);
    let provenance =
        Provenance { run: format!("{now}-{}", std::process::id()), commit: commit.as_deref(), dirty, framework: framework.as_deref(), date: &date };
    Identity::of(o, provenance).map_err(|e| e.to_string())
}

fn measure(exe: &Path, dir: &Path, root: &Path, o: &Opts, name: &str, block: Block) -> Result<(Value, Probe<4>), String> {
    let json = dir.join(format!("{name}-{}-{}.json", block.unit, block.position));
    let args = ["--workload", name, "--frames", &o.frames.to_string(), "--refresh-hz", &o.refresh_hz.to_string()];
    eprintln!("bench --pair: {BENCH} {} ({:?}, unit {})", args.join(" "), block.arm, block.unit);
    let status = Command::new(exe).current_dir(root).args(args).arg("--json").arg(&json).status().map_err(|e| format!("{}: {e}", exe.display()))?;
    if !status.success() {
        return Err(format!("{BENCH} {name} failed ({status})"));
    }
    let text = std::fs::read_to_string(&json).map_err(|e| format!("{}: {e}", json.display()))?;
    let report: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", json.display()))?;
    if report["context"]["workload"].as_str() != Some(name) {
        return Err(format!("{}: ran workload {}, not `{name}`", json.display(), report["context"]["workload"]));
    }
    let probe = probe(&report, o.refresh_hz)?;
    Ok((report["context"].clone(), probe))
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let args: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--pair").collect();
    let o = Opts::parse(&args, &WORKLOADS, hosted_image().as_deref()).map_err(|e| format!("bench --pair: {e}\n\n{HELP}"))?;
    let cores = std::thread::available_parallelism().map(|n| n.get()).ok();
    let load = frames::load_label(contended(super::loadavg(), cores));
    let target = root.join("target").join("bench");
    let out = Output::new(target.clone());
    out.prepare()?;
    if !o.skip_build {
        eprintln!("bench --pair: building {BENCH} (--release) into {}", target.display());
        let status = Command::new(env!("CARGO"))
            .current_dir(root)
            .env("CARGO_TARGET_DIR", &target)
            .args(["build", "--release", "-p", PACKAGE, "--example", BENCH])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("building {BENCH} failed"));
        }
    }
    let exe = target.join("release").join("examples").join(format!("{BENCH}{}", std::env::consts::EXE_SUFFIX));
    let id = identity(root, &o)?;
    let run = Run::drive(&o, &id, gates, |name, block| {
        let (context, probe) = measure(&exe, &out.blocks(), root, &o, name, block)?;
        Ok((workload(name, &context, load)?, probe))
    })?;
    run.write(&out, &id, load, &o, STAGES).map(drop)
}

#[cfg(test)]
mod tests;
