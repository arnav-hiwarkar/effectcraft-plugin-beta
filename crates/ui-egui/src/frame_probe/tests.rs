use serde_json::{Value, json};

use super::*;

fn s(k: u32) -> f64 {
    f64::from(k) / 1024.0
}

fn frame(p: &mut FrameProbe, start: f64, end: f64, playhead: Playhead) -> Option<FrameStages> {
    p.start(|| start);
    p.finish(start, || end, || playhead);
    p.take().and_then(|mut r| r.frames.pop())
}

fn paused(current: (u64, i64), shown: (u64, i64)) -> Playhead {
    Playhead { playing: false, current: Some(current), shown: Some(shown), clock: None }
}

fn on(clock: i64, shown: i64, step: i64, last: i64, generation: u64) -> Playhead {
    let clock = Clock { frame: clock, step, first: 0, last, generation };
    Playhead { playing: true, current: Some((7, shown)), shown: Some((7, shown)), clock: Some(clock) }
}

fn playing(clock: i64, shown: i64) -> Playhead {
    on(clock, shown, 1, 9, 1)
}

fn counts(heads: impl IntoIterator<Item = Playhead>) -> Vec<(u64, u64)> {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    heads.into_iter().enumerate().map(|(k, head)| frame(&mut p, k as f64, k as f64, head).map(|f| (f.played, f.late)).unwrap()).collect()
}

#[test]
fn the_disabled_probe_reads_no_clock_and_keeps_nothing_while_the_rate_follows_the_tick() {
    let reads = std::cell::Cell::new(0);
    let read = |t: f64| {
        reads.set(reads.get() + 1);
        t
    };
    let run = |p: &mut FrameProbe, frames: u32| {
        for k in 0..frames {
            let t = f64::from(k) / 64.0;
            p.start(|| read(t));
            p.input_queued(|| read(t));
            let since = p.stamp(|| read(t));
            p.add(Stage::Logic, since, || read(t));
            p.input_consumed();
            let clock = |t: f64| Clock { frame: read(t) as i64, step: 1, first: 0, last: 9, generation: 1 };
            p.finish(t, || read(t), || Playhead { playing: true, clock: Some(clock(t)), ..Playhead::default() });
        }
    };
    let mut p = FrameProbe::default();
    run(&mut p, 10_000);
    assert_eq!((reads.get(), p.take(), p.fps()), (0, None, Some(64.0)));
    assert_eq!(serde_json::to_value(&p).unwrap(), Value::Null);
    p.set_recording(true);
    run(&mut p, 10);
    assert_eq!((reads.get(), p.take().map(|r| r.frames.len())), (60, Some(10)));
}

#[test]
fn stages_do_not_overlap_and_the_ui_build_leaves_out_the_uploads_it_nested() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.start(|| s(1024));
    let logic = p.stamp(|| s(1026));
    p.add(Stage::Logic, logic, || s(1029));
    let build = p.stamp(|| s(1030));
    let upload = p.stamp(|| s(1031));
    p.add(Stage::Upload, upload, || s(1033));
    p.add(Stage::Build, build, || s(1040));
    p.finish(s(1024), || s(1044), Playhead::default);
    let f = p.take().unwrap().frames;
    let ms = |k: f64| k * 1000.0 / 1024.0;
    assert_eq!((f[0].t, f[0].input_ms, f[0].logic_ms, f[0].build_ms, f[0].upload_ms), (1000.0, ms(2.0), ms(3.0), ms(8.0), ms(2.0)));
}

#[test]
fn a_later_request_replaces_an_idle_input_stamp() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.input_queued(|| s(10));
    assert_eq!(frame(&mut p, s(20), s(30), Playhead::default()).unwrap().latency_ms, None);
    p.input_queued(|| s(40));
    p.start(|| s(41));
    p.input_consumed();
    p.finish(s(41), || s(45), Playhead::default);
    p.start(|| s(60));
    p.input_consumed();
    p.finish(s(60), || s(70), Playhead::default);
    let latencies: Vec<Option<f64>> = p.take().unwrap().frames.iter().map(|f| f.latency_ms).collect();
    assert_eq!(latencies, [Some(4.8828125), None]);
}

#[test]
fn input_not_yet_applied_has_no_latency() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.input_queued(|| s(10));
    assert_eq!(frame(&mut p, s(20), s(30), Playhead::default()).unwrap().latency_ms, None);
    p.start(|| s(40));
    p.input_consumed();
    p.finish(s(40), || s(50), Playhead::default);
    assert_eq!(p.take().unwrap().frames[0].latency_ms, Some(39.0625));
}

#[test]
fn frames_past_the_cap_count_as_dropped() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    for k in 0..RECORDED_FRAMES + 5 {
        p.start(|| k as f64);
        p.finish(k as f64, || k as f64, Playhead::default);
    }
    assert_eq!(serde_json::to_value(&p).unwrap(), json!({"count": 131_072, "dropped": 5}));
    let recorded = p.take().unwrap();
    assert_eq!((recorded.frames.len(), recorded.frames.last().map(|f| f.t), recorded.dropped), (131_072, Some(131_071_000.0), 5));
    assert_eq!(serde_json::to_value(&p).unwrap(), json!({"count": 0, "dropped": 0}));
}

#[test]
fn a_frame_no_ui_pass_ended_is_not_recorded() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.start(|| s(10));
    p.start(|| s(20));
    p.finish(s(20), || s(30), Playhead::default);
    p.finish(s(40), || s(40), Playhead::default);
    assert_eq!(p.take().unwrap().frames.iter().map(|f| f.t).collect::<Vec<_>>(), [s(20) * 1000.0]);
}

#[test]
fn each_comp_frame_the_clock_passes_is_late_unless_the_viewer_showed_it_in_time() {
    let late_shown = Playhead { shown: Some((7, 0)), ..playing(1, 1) };
    let heads = [playing(0, 0), late_shown, playing(1, 1), playing(3, 2), playing(4, 4), paused((7, 4), (7, 4))];
    assert_eq!(counts(heads), [(0, 0), (1, 0), (0, 0), (2, 1), (1, 1), (0, 0)]);
}

#[test]
fn a_seek_or_restart_starts_a_new_generation_and_counts_nothing_for_the_jump() {
    assert_eq!(counts([on(0, 0, 1, 299, 1), on(100, 100, 1, 299, 2), on(101, 101, 1, 299, 2)]), [(0, 0), (0, 0), (1, 0)]);
}

#[test]
fn a_loop_wrap_counts_the_frames_to_the_end_and_from_the_start() {
    assert_eq!(counts([playing(8, 8), playing(1, 1)]), [(0, 0), (3, 2)]);
    assert_eq!(counts([on(18, 18, 2, 20, 1), on(2, 2, 2, 20, 1)]), [(0, 0), (3, 2)]);
}

#[test]
fn a_small_step_back_in_one_generation_counts_nothing_and_does_not_count_the_loop() {
    assert_eq!(counts([on(100, 100, 1, 299, 1), on(99, 99, 1, 299, 1), on(100, 100, 1, 299, 1), on(101, 101, 1, 299, 1)]), [(0, 0), (0, 0), (0, 0), (1, 0)]);
    assert_eq!(counts([on(297, 297, 1, 299, 1), on(2, 2, 1, 299, 1)]), [(0, 0), (5, 4)]);
}

#[test]
fn frames_counted_before_a_step_back_are_not_counted_again_after_it() {
    let stalled = (1..=5).map(|k| Playhead { shown: Some((7, 0)), ..playing(k, k) });
    let resumed = (1..=7).map(|k| playing(k, k));
    let counted = counts(std::iter::once(playing(0, 0)).chain(stalled).chain(resumed));
    assert_eq!(counted[6..], [(0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (1, 0), (1, 0)]);
    assert_eq!(counted.iter().fold((0, 0), |(p, l), (fp, fl)| (p + fp, l + fl)), (7, 4));
}

#[test]
fn stalled_frames_the_clock_passes_are_late() {
    let stalled = (1..=5).map(|k| Playhead { shown: Some((7, 0)), ..playing(k, k) });
    assert_eq!(counts(std::iter::once(playing(0, 0)).chain(stalled)), [(0, 0), (1, 0), (1, 1), (1, 1), (1, 1), (1, 1)]);
}

#[test]
fn a_frame_step_counts_only_the_frames_the_preview_plays() {
    assert_eq!(counts([on(2, 2, 2, 10, 1), on(6, 6, 2, 10, 1), on(8, 6, 2, 10, 1), on(10, 10, 2, 10, 1)]), [(0, 0), (2, 1), (1, 0), (1, 1)]);
}

#[test]
fn a_seek_runs_from_its_frame_start_until_the_viewer_shows_the_new_frame() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    let seeks: Vec<Option<f64>> = [
        (0, 1, paused((7, 10), (7, 10))),
        (100, 101, paused((7, 11), (7, 10))),
        (200, 210, paused((7, 11), (7, 11))),
        (300, 301, paused((7, 12), (7, 11))),
        (400, 401, paused((7, 13), (7, 12))),
        (440, 450, paused((7, 13), (7, 13))),
        (500, 501, paused((8, 0), (8, 0))),
        (600, 601, Playhead { playing: true, ..paused((8, 5), (8, 4)) }),
        (700, 701, paused((8, 5), (8, 5))),
    ]
    .into_iter()
    .map(|(start, end, playhead)| frame(&mut p, s(start), s(end), playhead).unwrap().seek_ms)
    .collect();
    assert_eq!(seeks, [None, None, Some(107.421875), None, None, Some(48.828125), None, None, None]);
    frame(&mut p, s(800), s(801), paused((8, 6), (8, 5)));
    assert!(p.seeking());
    frame(&mut p, s(900), s(901), paused((8, 6), (8, 6)));
    assert!(!p.seeking());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_next_frame_key_through_the_control_channel_records_one_input_latency_and_one_seek() {
    let (control, requests) = std::sync::mpsc::channel();
    let mut session = effectcraft_engine::Session::default();
    session.execute("comp.new", json!({"name": "C", "width": 64, "height": 36, "frameRate": 30, "duration": 2})).unwrap();
    session.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let app = crate::EffectcraftApp::new(session).with_control(requests);
    let mut driver = crate::bench::Driver::new(app, egui::vec2(1280.0, 800.0));
    for _ in 0..30 {
        driver.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    driver.app.frame_probe.set_recording(true);
    let (reply, answer) = std::sync::mpsc::channel();
    control.send(crate::ControlRequest { method: "ui.key".into(), params: json!({"key": "PageDown"}), reply }).unwrap();
    let (mut frames, mut answered) = (Vec::new(), false);
    for _ in 0..400 {
        driver.step();
        frames.extend(driver.app.frame_probe.take().unwrap().frames);
        answered |= answer.try_recv().is_ok();
        if answered && !driver.app.frame_probe.seeking() && frames.iter().any(|f| f.seek_ms.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let latencies: Vec<f64> = frames.iter().filter_map(|f| f.latency_ms).collect();
    let seeks: Vec<f64> = frames.iter().filter_map(|f| f.seek_ms).collect();
    assert_eq!((answered, latencies.len(), seeks.len()), (true, 1, 1), "{frames:?}");
    assert!(frames.windows(2).all(|w| w[1].t > w[0].t), "{frames:?}");
    assert!(frames.iter().all(|f| f.build_ms > 0.0 && f.played == 0), "{frames:?}");
}

#[cfg(not(target_arch = "wasm32"))]
fn driver() -> (crate::bench::Driver, std::sync::mpsc::Sender<crate::ControlRequest>) {
    let (control, requests) = std::sync::mpsc::channel();
    let mut session = effectcraft_engine::Session::default();
    session.execute("comp.new", json!({"name": "C", "width": 64, "height": 36, "frameRate": 30, "duration": 2})).unwrap();
    session.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let mut driver = crate::bench::Driver::new(crate::EffectcraftApp::new(session).with_control(requests), egui::vec2(1280.0, 800.0));
    for _ in 0..30 {
        driver.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    (driver, control)
}

#[cfg(not(target_arch = "wasm32"))]
fn call(driver: &mut crate::bench::Driver, control: &std::sync::mpsc::Sender<crate::ControlRequest>, method: &str, params: Value) -> Value {
    let (reply, answer) = std::sync::mpsc::channel();
    control.send(crate::ControlRequest { method: method.into(), params, reply }).unwrap();
    driver.step();
    answer.try_recv().unwrap()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_control_channel_turns_the_probe_on_reports_its_count_and_drains_it() {
    let (mut d, control) = driver();
    let frames = |d: &mut crate::bench::Driver| call(d, &control, "ui.inspect", json!({}))["result"]["perf"]["frames"].clone();
    assert_eq!(frames(&mut d), Value::Null);
    assert_eq!(call(&mut d, &control, "ui.perf.takeFrames", json!({}))["error"], "the frame probe is off: turn it on with ui.set {\"frameProbe\": true}");
    assert_eq!(call(&mut d, &control, "ui.set", json!({"frameProbe": "on"}))["error"], "frameProbe must be a boolean");
    assert_eq!(call(&mut d, &control, "ui.set", json!({"frameProbe": true}))["ok"], true);
    d.step();
    assert_eq!(frames(&mut d), json!({"count": 1, "dropped": 0}));
    let taken = call(&mut d, &control, "ui.perf.takeFrames", json!({}));
    assert_eq!((taken["result"]["frames"].as_array().map(Vec::len), taken["result"]["dropped"].clone()), (Some(2), json!(0)));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_silent_preview_records_its_comp_frames_against_its_own_pacing_clock() {
    let (mut d, control) = driver();
    assert_eq!(call(&mut d, &control, "ui.set", json!({"frameProbe": true}))["ok"], true);
    call(&mut d, &control, "ui.playback", json!({"action": "play"}));
    let mut frames = Vec::new();
    for _ in 0..400 {
        d.step();
        frames.extend(d.app.frame_probe.take().unwrap().frames);
        if frames.iter().map(|f| f.played).sum::<u64>() >= 10 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (played, late) = frames.iter().fold((0, 0), |(p, l), f| (p + f.played, l + f.late));
    assert!(d.app.audio.is_none() && played >= 10 && late <= played, "{played} played, {late} late: {frames:?}");
}
