use std::time::{Duration, Instant, SystemTime};

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

#[derive(Debug, Clone)]
pub struct ClockNano {
    start: Instant,
    wall_time: SystemTime,
}

impl ClockNano {
    /// Returns the wall time of the timestamp `ts` generated
    /// from this clock.
    pub fn wall_time(&self, ts: Timestamp) -> SystemTime {
        self.wall_time.checked_add(ts.0).unwrap_or(self.wall_time)
    }
}

impl Default for ClockNano {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            wall_time: SystemTime::now(),
        }
    }
}

impl ClockNano {
    pub fn now(&self) -> Timestamp {
        Timestamp(self.start.elapsed())
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
