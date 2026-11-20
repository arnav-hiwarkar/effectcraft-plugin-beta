use serde_json::{Value, json};

use super::*;

fn s(k: u32) -> f64 {
    f64::from(k) / 1024.0
}

fn frame(p: &mut FrameProbe, start: f64, end: f64, playhead: Playhead) -> Option<FrameStages> {
    p.start(start);
    p.finish(|| end, || playhead);
    p.take().and_then(|mut r| r.frames.pop())
}

fn paused(current: (u64, i64), shown: (u64, i64)) -> Playhead {
    Playhead { playing: false, current: Some(current), shown: Some(shown), sound: None }
}

fn playing(sound: i64, shown: i64) -> Playhead {
    Playhead { playing: true, current: Some((7, shown)), shown: Some((7, shown)), sound: Some((sound, 1)) }
}

#[test]
fn the_disabled_probe_keeps_nothing_and_reads_no_clock_but_still_shows_the_rate() {
    let reads = std::cell::Cell::new(0);
    let read = |t: f64| {
        reads.set(reads.get() + 1);
        t
    };
    let mut p = FrameProbe::default();
    for k in 0..10_000 {
        let t = f64::from(k) / 64.0;
        p.start(t);
        p.input_queued(|| read(t));
        let since = p.stamp(|| read(t));
        p.add(Stage::Logic, since, || read(t));
        p.input_consumed();
        p.finish(|| read(t), || Playhead { playing: true, sound: Some((read(t) as i64, 1)), ..Playhead::default() });
    }
    assert_eq!((reads.get(), p.take()), (0, None));
    assert_eq!(serde_json::to_value(&p).unwrap(), Value::Null);
    assert_eq!(p.fps(), Some(64.0));
}

#[test]
fn stages_do_not_overlap_and_the_ui_build_leaves_out_the_uploads_it_nested() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.start(s(1024));
    let logic = p.stamp(|| s(1026));
    p.add(Stage::Logic, logic, || s(1029));
    let build = p.stamp(|| s(1030));
    let upload = p.stamp(|| s(1031));
    p.add(Stage::Upload, upload, || s(1033));
    p.add(Stage::Build, build, || s(1040));
    p.finish(|| s(1044), Playhead::default);
    let f = p.take().unwrap().frames;
    let ms = |k: f64| k * 1000.0 / 1024.0;
    assert_eq!((f[0].t, f[0].input_ms, f[0].logic_ms, f[0].build_ms, f[0].upload_ms), (1000.0, ms(2.0), ms(3.0), ms(8.0), ms(2.0)));
}

#[test]
fn input_queued_after_a_withdrawal_is_timed_from_its_own_queueing() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.input_queued(|| s(10));
    p.input_withdrawn();
    assert_eq!(frame(&mut p, s(20), s(30), Playhead::default()).unwrap().latency_ms, None);
    p.input_queued(|| s(40));
    p.start(s(41));
    p.input_consumed();
    p.finish(|| s(45), Playhead::default);
    p.start(s(60));
    p.input_consumed();
    p.finish(|| s(70), Playhead::default);
    let latencies: Vec<Option<f64>> = p.take().unwrap().frames.iter().map(|f| f.latency_ms).collect();
    assert_eq!(latencies, [Some(4.8828125), None]);
}

#[test]
fn input_not_yet_applied_has_no_latency() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    p.input_queued(|| s(10));
    assert_eq!(frame(&mut p, s(20), s(30), Playhead::default()).unwrap().latency_ms, None);
    p.start(s(40));
    p.input_consumed();
    p.finish(|| s(50), Playhead::default);
    assert_eq!(p.take().unwrap().frames[0].latency_ms, Some(39.0625));
}

#[test]
fn frames_past_the_cap_count_as_dropped() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    for k in 0..RECORDED_FRAMES + 5 {
        p.start(k as f64);
        p.finish(|| k as f64, Playhead::default);
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
    p.start(s(10));
    p.start(s(20));
    p.finish(|| s(30), Playhead::default);
    p.finish(|| s(40), Playhead::default);
    assert_eq!(p.take().unwrap().frames.iter().map(|f| f.t).collect::<Vec<_>>(), [s(20) * 1000.0]);
}

#[test]
fn each_comp_frame_the_sound_passes_is_late_unless_the_viewer_showed_it_in_time() {
    let mut p = FrameProbe::default();
    p.set_recording(true);
    let sequence = [playing(0, 0), (Playhead { shown: Some((7, 0)), ..playing(1, 1) }), playing(1, 1), playing(3, 2), playing(4, 4), paused((7, 4), (7, 4))];
    let counts: Vec<(u64, u64)> =
        sequence.into_iter().enumerate().map(|(k, playhead)| frame(&mut p, k as f64, k as f64, playhead).map(|f| (f.played, f.late)).unwrap()).collect();
    assert_eq!(counts, [(0, 0), (1, 0), (0, 0), (2, 1), (1, 1), (0, 0)]);
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
