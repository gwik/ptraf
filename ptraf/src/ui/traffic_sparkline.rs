use std::{collections::VecDeque, time::Duration};

use ptraf_common::Channel;
use tui::{
    backend::Backend,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Sparkline},
    Frame,
};

use crate::{
    clock::{ClockNano, Timestamp},
    store::Store,
};

#[derive(Debug)]
struct DataPoint {
    ts: Timestamp,
    rx: u64,
    tx: u64,
}

#[derive(Debug, Default)]
pub(crate) struct TrafficSparkline {
    dataset: VecDeque<DataPoint>,
}

impl TrafficSparkline {
    // TODO(gwik): config
    const TOTAL_DURATION: Duration = Duration::from_secs(60);

    pub(crate) fn collect(&mut self, ts: Timestamp, _clock: &ClockNano, store: &Store) {
        // drop outdated datapoints.
        while let Some(front) = self.dataset.front() {
            if ts.saturating_elapsed_since(&front.ts) > Self::TOTAL_DURATION {
                self.dataset.pop_front();
            } else {
                break;
            }
        }

        let last_ts = self.dataset.back().map(|d| d.ts);

        store
            .segments_view()
            .iter()
            // TODO(gwik): add a more efficient method to segments view iterator
            .skip_while(|time_segment| time_segment.ts < last_ts.unwrap_or_default())
            .for_each(|time_segment| {
                if last_ts == Some(time_segment.ts) {
                    let datapoint = self.dataset.back_mut().unwrap();
                    datapoint.rx = time_segment.segment.total(Channel::Rx.into());
                    datapoint.tx = time_segment.segment.total(Channel::Tx.into());
                } else {
                    self.dataset.push_back(DataPoint {
                        ts: time_segment.ts,
                        rx: time_segment.segment.total(Channel::Rx.into()),
                        tx: time_segment.segment.total(Channel::Tx.into()),
                    });
                }
            });
    }
}

pub(crate) fn traffic_sparkline_ui<B: Backend>(
    f: &mut Frame<B>,
    rect: Rect,
    traffic: &TrafficSparkline,
) {
    let skip = traffic.dataset.len().saturating_sub(rect.width as usize);
    let data = traffic
        .dataset
        .iter()
        .skip(skip)
        .map(|datapoint| datapoint.rx + datapoint.tx)
        // so sad...
        .collect::<Vec<_>>();

    let sparkline = Sparkline::default()
        .data(&data[..data.len().saturating_sub(1)])
        .style(Style::default().fg(Color::Yellow));

    f.render_widget(sparkline, rect);
}
