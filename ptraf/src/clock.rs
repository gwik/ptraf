use std::time::{Duration, Instant};

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Default)]
pub struct Timestamp(pub Duration);

impl Timestamp {
    pub fn saturating_elapsed_since(&self, other: &Timestamp) -> Duration {
        other.0.saturating_sub(self.0)
    }

    pub fn trunc(&self, window: Duration) -> Self {
        let nanos = self.0.as_nanos() as u64;
        let rem = nanos % (window.as_nanos() as u64);
        Self(Duration::from_nanos(nanos - rem))
    }
}

impl std::ops::Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl From<Duration> for Timestamp {
    fn from(val: Duration) -> Self {
        Self(val)
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Timestamp;

    #[test]
    fn timestamp_trunc() {
        let ts: Timestamp = Duration::from_secs(3010).into();
        let ts = ts.trunc(Duration::from_secs(1000));
        assert_eq!(ts, Duration::from_secs(3000).into());
    }
}
