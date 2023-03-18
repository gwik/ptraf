use std::time::Duration;

pub(crate) struct Formatter(humansize::FormatSizeOptions);

impl Default for Formatter {
    fn default() -> Self {
        Self(
            humansize::FormatSizeOptions::from(humansize::BINARY)
                .base_unit(humansize::BaseUnit::Byte)
                .space_after_value(true),
        )
    }
}

impl Formatter {
    pub fn format_rate(&self, rate_duration: Option<Duration>, val: u64) -> String {
        rate_duration
            .map(|rate_duration| {
                let rate = (val as f64 / rate_duration.as_secs_f64()).round().max(0.0) as u64;
                humansize::format_size(rate, self.0)
            })
            .unwrap_or_default()
    }
}
