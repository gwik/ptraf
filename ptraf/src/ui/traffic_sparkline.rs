use std::{collections::VecDeque, time::Duration};

use tui::{
    backend::Backend,
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Sparkline},
    Frame,
};

use crate::{
    clock::Timestamp,
    store::{Store, TimeSegment},
};

use super::{format::Formatter, Filter, UiContext};

#[derive(Debug, Default, Clone, Copy)]
struct DataPoint {
    ts: Timestamp,
    rx: u64,
    tx: u64,
}

#[derive(Debug, Default)]
pub(crate) struct TrafficSparkline {
    filter: Filter,
    dataset: VecDeque<DataPoint>,
    reverse_buffer: Vec<DataPoint>,
}

impl TrafficSparkline {
    pub(crate) fn with_filter(filter: Filter) -> Self {
        Self {
            filter,
            ..Self::default()
        }
    }

    pub(crate) fn collect(&mut self, store: &Store) {
        // the starting point for data already collected.
        let start = self.dataset.back().map(|dp| dp.ts).unwrap_or_default();

        let view = store.segments_view();

        // Discard the outdated datapoints.
        if let Some(TimeSegment { ts: oldest, .. }) = view.oldest() {
            while let Some(DataPoint { ts: front_ts, .. }) = self.dataset.front() {
                if front_ts < oldest {
                    self.dataset.pop_front();
                } else {
                    break;
                }
            }
        }

        let interest = self.filter.interest();

        // Insert new datapoints from newest to oldest in the reverse buffer.
        view.iter()
            .rev()
            .take_while(|time_segment| time_segment.ts > start)
            .for_each(|time_segment| {
                let mut datapoint = DataPoint {
                    ts: time_segment.ts,
                    rx: 0,
                    tx: 0,
                };

                let stat = time_segment.segment.stat_by_interest(&interest);
                datapoint.rx = stat.as_ref().map(|stat| stat.rx).unwrap_or_default();
                datapoint.tx = stat.as_ref().map(|stat| stat.tx).unwrap_or_default();

                self.reverse_buffer.push(datapoint);
            });

        // Drain the reverse buffer and insert in the dataset in chronological order.
        self.reverse_buffer
            .drain(..)
            .skip(1) // skip the newest since the segment is incomplete
            .rev()
            .for_each(|datapoint| self.dataset.push_back(datapoint));
    }
}

#[derive(Debug, Default)]
pub(super) struct TrafficSparklineView {
    traffic: TrafficSparkline,
    output_buffer: Vec<f64>,
    input_buffer_ts: Vec<f64>,
    input_buffer_val: Vec<f64>,
}

impl TrafficSparklineView {
    pub(super) fn with_filter(filter: Filter) -> Self {
        let traffic = TrafficSparkline::with_filter(filter);
        Self {
            traffic,
            ..Self::default()
        }
    }

    pub(super) fn render<B: Backend>(
        &mut self,
        frame: &mut Frame<B>,
        rect: Rect,
        ctx: &UiContext<'_>,
    ) {
        self.traffic.collect(ctx.store);

        let output_interval = Duration::from_secs_f64(
            ctx.store.window().as_secs_f64() * ctx.store.max_capacity() as f64 / rect.width as f64,
        );

        let mut max = 0.0f64;

        let data: Vec<u64> = {
            let window = ctx.store.window().as_secs_f64();

            self.input_buffer_ts.clear();
            self.input_buffer_val.clear();

            // Builds two separate vector of timestamp ans values
            // coordinates for the interpolate function.
            for datapoint in self.traffic.dataset.iter() {
                let val = (datapoint.rx + datapoint.tx) as f64 / window;

                self.input_buffer_ts.push(datapoint.ts.0.as_secs_f64());
                self.input_buffer_val.push(val);
            }

            // Clear and allocate the interpolation output buffer.
            self.output_buffer.clear();
            self.output_buffer.resize(rect.width as usize + 1, 0.0);

            // Sizes the output buffer relatevily to the input size so they reprensent
            // the same duration.
            let buf_len = ((ctx.store.window().as_secs_f64() / output_interval.as_secs_f64()
                * self.traffic.dataset.len() as f64)
                .round() as usize)
                .min(self.output_buffer.len().saturating_sub(1));

            interpolate(
                &self.input_buffer_ts,
                &self.input_buffer_val,
                &mut self.output_buffer[..buf_len],
                output_interval.as_secs_f64(),
            );

            self.output_buffer
                .drain(..)
                .inspect(|&v| max = max.max(v))
                .map(|v| v as u64)
                .collect()
        };

        let formatter = Formatter::default();
        let sparkline = Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .title(format!(
                        " max: {}/s",
                        formatter.format_rate(Duration::from_secs(1).into(), max as u64),
                    ))
                    .title_alignment(Alignment::Right),
            )
            .max((max + max * 0.1) as u64)
            .data(&data[..data.len().saturating_sub(1)])
            .style(Style::default().fg(Color::Yellow));

        frame.render_widget(sparkline, rect);
    }
}

fn interpolate(input_ts: &[f64], input_val: &[f64], output_buf: &mut [f64], output_interval: f64) {
    if output_buf.is_empty() || input_ts.is_empty() || input_val.is_empty() {
        return;
    }

    let start = input_ts[0];

    for (idx, out_sample) in output_buf.iter_mut().enumerate() {
        let position = output_interval * idx as f64 + start;
        *out_sample = interp::interp(input_ts, input_val, position);
    }
}
