use std::time::{Duration, Instant};

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Default)]
pub struct Timestamp(pub Duration);

impl Timestamp {
    pub fn saturating_elapsed_since(&self, other: &Timestamp) -> Duration {
        other.0.saturating_sub(self.0)
    }
}

pub struct ClockNano(Instant);

impl Default for ClockNano {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl ClockNano {
    pub fn now(&self) -> Timestamp {
        Timestamp(self.0.elapsed())
    }
}
