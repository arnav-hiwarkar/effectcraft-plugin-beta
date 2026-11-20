use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use effectcraft_ui_egui::ControlRequest;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::audio::{AudioDevice, AudioFeed};
use effectcraft_ui_egui::bench::Driver;
use effectcraft_ui_egui::frame_probe::RECORDED_FRAMES;
use serde_json::{Value, json};

const WINDOW: [f32; 2] = [1920.0, 1080.0];
const COMP: [u32; 2] = [1920, 1080];
const COMP_FPS: u32 = 30;
const COMP_SECONDS: u32 = 10;
const SAMPLE_RATE: u32 = 48_000;
const WARMUP_FRAMES: u32 = 120;

#[derive(Clone, Copy)]
enum Workload {
    Playback,
    Scrub,
}

impl Workload {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "playback" => Ok(Workload::Playback),
            "scrub" => Ok(Workload::Scrub),
            other => Err(format!("unknown workload `{other}`: playback or scrub")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Workload::Playback => "playback",
            Workload::Scrub => "scrub",
        }
    }
}

struct Args {
    workload: Workload,
    frames: u32,
    refresh_hz: u32,
    json: Option<PathBuf>,
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let number = |name: &str, default: u32| match value(args, name) {
        None => Ok(default),
        Some(v) => v.parse::<u32>().ok().filter(|n| *n > 0).ok_or(format!("{name} needs a whole number above 0")),
    };
    let frames = number("--frames", 3600)?;
    if frames < 2 || usize::try_from(frames).unwrap_or(usize::MAX) > RECORDED_FRAMES {
        return Err(format!("--frames is 2 to {RECORDED_FRAMES}: an interval needs two frames, and the frame probe keeps {RECORDED_FRAMES}"));
    }
    Ok(Args {
        workload: Workload::parse(value(args, "--workload").unwrap_or("playback"))?,
        frames,
        refresh_hz: number("--refresh-hz", 60)?,
        json: value(args, "--json").map(PathBuf::from),
    })
}

#[derive(Default)]
struct Clocked {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioDevice for Clocked {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn start(&mut self, feed: Arc<AudioFeed>) -> Result<(), String> {
        let stop = self.stop.clone();
        self.thread = Some(std::thread::spawn(move || {
            let (start, mut pulled, mut buffer) = (Instant::now(), 0u64, Vec::new());
            while !stop.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * f64::from(SAMPLE_RATE)) as u64;
                buffer.resize(2 * (due - pulled) as usize, 0.0);
                feed.pull(&mut buffer);
                pulled = due;
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn tone(path: &Path) -> Result<(), String> {
    let frames = SAMPLE_RATE * COMP_SECONDS;
    let data = frames * 4;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for k in 0..frames {
        let sample = ((f64::from(k) * 440.0 / f64::from(SAMPLE_RATE) * std::f64::consts::TAU).sin() * 8000.0) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, wav).map_err(|e| format!("{}: {e}", path.display()))
}

fn execute(app: &mut EffectcraftApp, method: &str, params: Value) -> Result<Value, String> {
    app.session.execute(method, params).map_err(|e| format!("{method}: {e}"))
}

fn project(app: &mut EffectcraftApp, media: &Path) -> Result<(), String> {
    let wav = media.join("tone.wav");
    tone(&wav)?;
    let comp = execute(app, "comp.new", json!({"name": "Bench", "width": COMP[0], "height": COMP[1], "frameRate": COMP_FPS, "duration": COMP_SECONDS}))?;
    let layer = execute(app, "layer.newSolid", json!({"color": "#3366cc", "width": 480, "height": 270}))?["layer"].clone();
    for (time, x) in [(0.0, 240.0), (f64::from(COMP_SECONDS), 1680.0)] {
        execute(app, "prop.addKey", json!({"layer": layer, "path": "transform/position", "time": time, "value": [x, 540.0]}))?;
    }
    execute(app, "file.import", json!({"paths": [wav.to_string_lossy()], "addToComp": true}))?;
    execute(app, "comp.open", json!({"comp": comp["comp"]}))?;
    Ok(())
}

struct Bench {
    driver: Driver,
    control: Sender<ControlRequest>,
    waiting: Option<Receiver<Value>>,
    period: Duration,
    next: Instant,
}

impl Bench {
    fn present(&mut self) {
        let wait = self.next.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        self.driver.step();
        let done = Instant::now();
        while self.next <= done {
            self.next += self.period;
        }
    }

    fn answered(&mut self) -> Result<bool, String> {
        let Some(reply) = &self.waiting else { return Ok(true) };
        match reply.try_recv() {
            Ok(v) if v["ok"] == json!(true) => {
                self.waiting = None;
                Ok(true)
            }
            Ok(v) => Err(format!("control request failed: {v}")),
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err("the control reply was dropped".into()),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<(), String> {
        let (reply, answer) = channel();
        self.control.send(ControlRequest { method: method.into(), params, reply }).map_err(|e| format!("{method}: {e}"))?;
        self.waiting = Some(answer);
        Ok(())
    }

    fn drive(&mut self, workload: Workload) -> Result<(), String> {
        if !self.answered()? {
            return Ok(());
        }
        let app = &self.driver.app;
        match workload {
            Workload::Playback if !app.playback.playing => self.request("ui.playback", json!({"action": "play"})),
            Workload::Playback => Ok(()),
            Workload::Scrub if app.frame_probe.seeking() || app.playback.playing => Ok(()),
            Workload::Scrub => {
                let last = i64::from(COMP_FPS * COMP_SECONDS) - 1;
                let at_end = app.session.active_comp().is_some_and(|c| c.frame_rate.frame_at(app.session.time()) >= last);
                self.request("ui.key", json!({"key": if at_end { "Home" } else { "PageDown" }}))
            }
        }
    }
}

fn run(args: &Args) -> Result<Value, String> {
    let media = tempfile::tempdir().map_err(|e| format!("media folder: {e}"))?;
    let (control, requests) = channel();
    let mut app = EffectcraftApp::new(effectcraft_host::session()).with_control(requests);
    let started = Arc::new(Mutex::new(0u32));
    let opened = started.clone();
    app.hooks.audio_device = Some(Box::new(move |_| {
        *opened.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        Some(Box::new(Clocked::default()) as Box<dyn AudioDevice>)
    }));
    app.ui.cache_when_idle = false;
    project(&mut app, media.path())?;
    let driver = Driver::new(app, egui::vec2(WINDOW[0], WINDOW[1]));
    let period = Duration::from_secs(1) / args.refresh_hz;
    let mut bench = Bench { driver, control, waiting: None, period, next: Instant::now() };
    for _ in 0..WARMUP_FRAMES {
        bench.drive(args.workload)?;
        bench.present();
    }
    bench.driver.app.frame_probe.set_recording(true);
    for _ in 0..args.frames {
        bench.drive(args.workload)?;
        bench.present();
    }
    let app = &mut bench.driver.app;
    let recorded = app.frame_probe.take().ok_or("the frame probe stopped recording")?;
    if recorded.dropped > 0 {
        return Err(format!("the frame probe dropped {} frames past its cap", recorded.dropped));
    }
    let frames = recorded.frames;
    let latencies = frames.iter().filter(|f| f.latency_ms.is_some()).count();
    let seeks = frames.iter().filter(|f| f.seek_ms.is_some()).count();
    let (played, late) = frames.iter().fold((0, 0), |(p, l), f| (p + f.played, l + f.late));
    let outputs = *started.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    eprintln!(
        "{}: {} frames, {latencies} with input latency, {seeks} seeks, {late} of {played} comp frames late, {outputs} audio outputs opened",
        args.workload.name(),
        frames.len()
    );
    if matches!(args.workload, Workload::Playback) && (played == 0 || outputs == 0) {
        return Err("playback never ran against the audio clock".into());
    }
    if matches!(args.workload, Workload::Scrub) && (latencies == 0 || seeks == 0) {
        return Err("the scrub measured no input latency or seek".into());
    }
    Ok(json!({
        "context": {
            "workload": args.workload.name(),
            "backend": "cpu",
            "scale": format!("{}x{}@{}fps-{}x{}", COMP[0], COMP[1], COMP_FPS, WINDOW[0], WINDOW[1]),
            "refresh_hz": args.refresh_hz,
        },
        "frames": frames,
    }))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let args = parse_args(&args)?;
    let report = run(&args)?;
    if let Some(path) = &args.json {
        let text = serde_json::to_string(&report).map_err(|e| format!("encode: {e}"))?;
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}
