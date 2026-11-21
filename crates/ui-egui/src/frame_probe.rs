use std::collections::VecDeque;

pub const RECORDED_FRAMES: usize = 1 << 17;
const RATE_WINDOW_S: f64 = 1.0;
const RATE_STARTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Logic,
    Build,
    Upload,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameStages {
    pub t: f64,
    pub input_ms: f64,
    pub logic_ms: f64,
    pub build_ms: f64,
    pub upload_ms: f64,
    pub latency_ms: Option<f64>,
    pub played: u64,
    pub late: u64,
    pub seek_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Playhead {
    pub playing: bool,
    pub current: Option<(u64, i64)>,
    pub shown: Option<(u64, i64)>,
    pub clock: Option<Clock>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    pub frame: i64,
    pub step: i64,
    pub first: i64,
    pub last: i64,
    pub generation: u64,
}

impl Clock {
    fn passed(&self, from: i64) -> Option<u64> {
        let step = self.step.max(1);
        let between = |a: i64, b: i64| u64::try_from((b - a) / step).unwrap_or(0);
        if self.frame >= from {
            return Some(between(from, self.frame));
        }
        let wrap = between(from, self.last) + 1 + between(self.first, self.frame);
        (wrap < between(self.frame, from)).then_some(wrap)
    }
}

#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct Recorded {
    pub frames: Vec<FrameStages>,
    pub dropped: u64,
}

#[derive(Debug)]
struct Open {
    t: f64,
    first: Option<f64>,
    logic: f64,
    draw: f64,
    upload: f64,
    consumed: bool,
}

#[derive(Clone, Copy, Debug)]
struct Due {
    frame: i64,
    generation: u64,
    shown: bool,
}

#[derive(Debug, Default)]
struct Recording {
    frames: Vec<FrameStages>,
    dropped: u64,
    injected_at: Option<f64>,
    due: Option<Due>,
    current: Option<(u64, i64)>,
    seek: Option<(f64, (u64, i64))>,
}

impl Recording {
    fn playback(&mut self, p: &Playhead) -> (u64, u64) {
        let Some((comp, clock)) = p.current.map(|(comp, _)| comp).zip(p.clock).filter(|_| p.playing) else {
            self.due = None;
            return (0, 0);
        };
        let on_time = p.shown == Some((comp, clock.frame));
        let now = Due { frame: clock.frame, generation: clock.generation, shown: on_time };
        let Some(due) = self.due.filter(|due| due.generation == clock.generation) else {
            self.due = Some(now);
            return (0, 0);
        };
        if due.frame == clock.frame {
            self.due = Some(Due { shown: due.shown || on_time, ..now });
            return (0, 0);
        }
        let Some(passed) = clock.passed(due.frame) else { return (0, 0) };
        self.due = Some(now);
        (passed, u64::from(!due.shown) + passed.saturating_sub(1))
    }

    fn seek(&mut self, p: &Playhead, start: f64, end: f64) -> Option<f64> {
        let moved = self.current.zip(p.current).filter(|(was, now)| was.0 == now.0 && was.1 != now.1).map(|(_, now)| now);
        let comp_changed = self.current.zip(p.current).is_some_and(|(was, now)| was.0 != now.0);
        self.current = p.current;
        if p.playing || comp_changed {
            self.seek = None;
        } else if let Some(target) = moved {
            self.seek = Some((start, target));
        }
        let (since, target) = self.seek?;
        (p.shown == Some(target)).then(|| {
            self.seek = None;
            (end - since).max(0.0) * 1000.0
        })
    }
}

#[derive(Debug, Default)]
pub struct FrameProbe {
    starts: VecDeque<f64>,
    open: Option<Open>,
    recording: Option<Recording>,
}

impl FrameProbe {
    pub fn set_recording(&mut self, on: bool) {
        self.recording = on.then(|| self.recording.take().unwrap_or_default());
    }

    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn seeking(&self) -> bool {
        self.recording.as_ref().is_some_and(|r| r.seek.is_some())
    }

    pub fn take(&mut self) -> Option<Recorded> {
        self.recording.as_mut().map(|r| Recorded { frames: std::mem::take(&mut r.frames), dropped: std::mem::take(&mut r.dropped) })
    }

    pub fn fps(&self) -> Option<f64> {
        let (first, last) = (self.starts.front()?, self.starts.back()?);
        (last > first).then(|| (self.starts.len() - 1) as f64 / (last - first))
    }

    pub fn start(&mut self, now: impl FnOnce() -> f64) {
        self.open = self.recording.is_some().then(|| Open { t: now(), first: None, logic: 0.0, draw: 0.0, upload: 0.0, consumed: false });
    }

    pub fn stamp(&self, now: impl FnOnce() -> f64) -> Option<f64> {
        (self.recording.is_some() && self.open.is_some()).then(now)
    }

    pub fn add(&mut self, stage: Stage, since: Option<f64>, now: impl FnOnce() -> f64) {
        let (Some(since), Some(open)) = (since, &mut self.open) else { return };
        let took = (now() - since).max(0.0);
        open.first = Some(open.first.map_or(since, |first| first.min(since)));
        match stage {
            Stage::Logic => open.logic += took,
            Stage::Build => open.draw += took,
            Stage::Upload => open.upload += took,
        }
    }

    pub fn input_queued(&mut self, now: impl FnOnce() -> f64) {
        if let Some(r) = &mut self.recording {
            r.injected_at = Some(now());
        }
    }

    pub fn input_consumed(&mut self) {
        if let Some(open) = &mut self.open {
            open.consumed = true;
        }
    }

    pub fn finish(&mut self, tick: f64, now: impl FnOnce() -> f64, playhead: impl FnOnce() -> Playhead) {
        while self.starts.front().is_some_and(|t| tick - t > RATE_WINDOW_S) || self.starts.len() >= RATE_STARTS {
            self.starts.pop_front();
        }
        self.starts.push_back(tick);
        let (Some(open), Some(r)) = (self.open.take(), &mut self.recording) else { return };
        let end = now();
        let p = playhead();
        let ms = |s: f64| s * 1000.0;
        let latency_ms = r.injected_at.filter(|_| open.consumed).map(|at| ms((end - at).max(0.0)));
        if latency_ms.is_some() {
            r.injected_at = None;
        }
        let (played, late) = r.playback(&p);
        let seek_ms = r.seek(&p, open.t, end);
        let frame = FrameStages {
            t: ms(open.t),
            input_ms: ms((open.first.unwrap_or(end) - open.t).max(0.0)),
            logic_ms: ms(open.logic),
            build_ms: ms((open.draw - open.upload).max(0.0)),
            upload_ms: ms(open.upload),
            latency_ms,
            played,
            late,
            seek_ms,
        };
        if r.frames.len() < RECORDED_FRAMES {
            r.frames.push(frame);
        } else {
            r.dropped = r.dropped.saturating_add(1);
        }
    }
}

impl serde::Serialize for FrameProbe {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let Some(r) = &self.recording else { return ser.serialize_none() };
        let mut out = ser.serialize_struct("FrameProbe", 2)?;
        out.serialize_field("count", &r.frames.len())?;
        out.serialize_field("dropped", &r.dropped)?;
        out.end()
    }
}

#[cfg(test)]
mod tests;
