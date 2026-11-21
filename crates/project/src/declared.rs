#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Declared {
    Audio,
    Filter(Visual),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visual {
    pub routes: Routes,
    pub time: TimeReach,
    pub inputs: EffectInputs,
    pub cover: Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Routes {
    Cpu,
    CpuAndGpu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeReach {
    Frame,
    Clock,
    OtherFrames,
    ClockAndOtherFrames,
    OtherTimesUnkeyed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectInputs {
    Layer,
    CompScene,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    Layer,
    OwnMargins,
}

pub type FindDeclared = fn(&str) -> Option<Declared>;

impl Visual {
    pub const CPU: Visual = Visual { routes: Routes::Cpu, time: TimeReach::Frame, inputs: EffectInputs::Layer, cover: Cover::Layer };
    pub const GPU: Visual = Visual { routes: Routes::CpuAndGpu, ..Visual::CPU };

    pub const fn time(self, time: TimeReach) -> Visual {
        Visual { time, ..self }
    }

    pub const fn inputs(self, inputs: EffectInputs) -> Visual {
        Visual { inputs, ..self }
    }

    pub const fn cover(self, cover: Cover) -> Visual {
        Visual { cover, ..self }
    }
}

impl Declared {
    pub fn visual(self) -> Option<Visual> {
        match self {
            Declared::Audio => None,
            Declared::Filter(v) => Some(v),
        }
    }

    pub fn runs_on_gpu(self) -> bool {
        self.visual().is_some_and(|v| v.routes == Routes::CpuAndGpu)
    }

    pub fn time(self) -> TimeReach {
        self.visual().map_or(TimeReach::Frame, |v| v.time)
    }
}

impl TimeReach {
    pub fn keys_on_time(self) -> bool {
        matches!(self, TimeReach::Clock | TimeReach::ClockAndOtherFrames | TimeReach::OtherTimesUnkeyed)
    }

    pub fn reaches_other_frames(self) -> bool {
        matches!(self, TimeReach::OtherFrames | TimeReach::ClockAndOtherFrames | TimeReach::OtherTimesUnkeyed)
    }
}

pub(crate) fn is_audio(find: FindDeclared, effect: &str) -> bool {
    find(effect) == Some(Declared::Audio)
}

pub(crate) fn reads_other_frames(find: FindDeclared, effect: &str) -> bool {
    find(effect).is_none_or(|d| d.time().reaches_other_frames())
}
