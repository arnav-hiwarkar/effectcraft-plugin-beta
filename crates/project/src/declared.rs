#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declared {
    pub role: EffectRole,
    pub routes: Routes,
    pub time: TimeReach,
    pub inputs: EffectInputs,
    pub cover: Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectRole {
    Filter,
    Audio,
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
    Neighbours,
    History,
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

impl Declared {
    pub const CPU: Declared =
        Declared { role: EffectRole::Filter, routes: Routes::Cpu, time: TimeReach::Frame, inputs: EffectInputs::Layer, cover: Cover::Layer };
    pub const GPU: Declared = Declared { routes: Routes::CpuAndGpu, ..Declared::CPU };
    pub const AUDIO: Declared = Declared { role: EffectRole::Audio, ..Declared::CPU };

    pub const fn time(self, time: TimeReach) -> Declared {
        Declared { time, ..self }
    }

    pub const fn inputs(self, inputs: EffectInputs) -> Declared {
        Declared { inputs, ..self }
    }

    pub const fn cover(self, cover: Cover) -> Declared {
        Declared { cover, ..self }
    }

    pub fn runs_on_gpu(&self) -> bool {
        self.routes == Routes::CpuAndGpu
    }
}

impl TimeReach {
    pub fn reads_clock(self) -> bool {
        matches!(self, TimeReach::Clock | TimeReach::Neighbours | TimeReach::History)
    }

    pub fn reads_other_frames(self) -> bool {
        matches!(self, TimeReach::OtherFrames | TimeReach::Neighbours | TimeReach::History)
    }
}

pub(crate) fn is_audio(find: FindDeclared, effect: &str) -> bool {
    find(effect).is_some_and(|d| d.role == EffectRole::Audio)
}

pub(crate) fn reads_other_frames(find: FindDeclared, effect: &str) -> bool {
    find(effect).is_none_or(|d| d.time.reads_other_frames())
}
