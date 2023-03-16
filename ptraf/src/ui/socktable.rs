use std::{
    collections::HashMap,
    net::SocketAddr,
    ops::Range,
    time::{Duration, SystemTime},
};

use tui::widgets::TableState;

use crate::{
    clock::{ClockNano, Timestamp},
    store::{Interest, Socket, Stat, Store, TimeSegment},
};

#[derive(Debug)]
pub struct SocketTableConfig {
    collection_window: Duration,
    rate_window: Duration,
}

impl Default for SocketTableConfig {
    fn default() -> Self {
        Self {
            collection_window: Duration::from_secs(300), // 5 min
            rate_window: Duration::from_secs(1),
        }
    }
}

impl SocketTableConfig {
    pub fn build(self) -> SocketTable {
        SocketTable::new(self)
    }

    pub fn rate_window(&self) -> Duration {
        self.rate_window
    }

    pub fn backlog(&self) -> Duration {
        self.collection_window
    }
}

#[derive(Debug)]
pub struct SocketTable {
    table_state: TableState,
    dataset: Vec<DataPoint>,
    rate_collection_range: Option<Range<Timestamp>>,
    config: SocketTableConfig,
}

impl SocketTable {
    pub fn new(config: SocketTableConfig) -> Self {
        Self {
            table_state: TableState::default(),
            dataset: Vec::default(),
            rate_collection_range: None,
            config,
        }
    }

    pub fn config(&self) -> &SocketTableConfig {
        &self.config
    }

    pub fn dataset(&self) -> &[DataPoint] {
        &self.dataset
    }

    pub fn table_state(&self) -> &TableState {
        &self.table_state
    }

    pub fn rate_collection_range(&self) -> Option<&Range<Timestamp>> {
        self.rate_collection_range.as_ref()
    }

    pub fn down(&mut self) {
        let selected = if self.dataset.is_empty() {
            None
        } else {
            self.table_state
                .selected()
                .map_or(0, |selected| {
                    selected
                        .saturating_add(1)
                        .min(self.dataset.len().saturating_sub(1))
                })
                .into()
        };
        self.table_state.select(selected);
    }

    pub fn up(&mut self) {
        let selected = if self.dataset.is_empty() || self.table_state.selected().is_none() {
            None
        } else {
            self.table_state
                .selected()
                .unwrap()
                .saturating_sub(1)
                .min(self.dataset.len().saturating_sub(1))
                .into()
        };
        self.table_state.select(selected);
    }

    pub fn collect(&mut self, ts: Timestamp, clock: &ClockNano, store: &Store) {
        let window = store.window();

        let ts = ts.trunc(window);

        let rate_until: Timestamp =
            ts.0.saturating_sub(self.config.rate_window)
                .max(window)
                .into();
        let rate_until = rate_until.trunc(window);

        let mut collector = SocketTableCollector::new(rate_until);

        store
            .segments_view()
            .iter()
            .rev()
            .take_while(|time_segment| {
                time_segment.ts.saturating_elapsed_since(&ts) <= self.config.collection_window
            })
            .for_each(|time_segment| collector.collect(time_segment, clock));

        self.rate_collection_range
            .replace(collector.oldest_rate_segment_ts.unwrap_or(ts)..ts);
        self.dataset = collector.into_dataset(ts);
    }
}

#[derive(Debug, Clone)]
pub struct DataPoint {
    pub socket: Socket,
    pub stat: Stat,
    pub last_activity: SystemTime,
    pub rate_stat: Stat,
    pub pid: u32,
}

#[derive(Debug)]
struct SocketTableCollector {
    socket_cache: HashMap<SocketAddr, DataPoint, fxhash::FxBuildHasher>,
    oldest_segment_ts: Option<Timestamp>,
    oldest_rate_segment_ts: Option<Timestamp>,
    rate_until: Timestamp,
}

impl SocketTableCollector {
    fn new(rate_until: Timestamp) -> Self {
        Self {
            socket_cache: HashMap::default(),
            oldest_segment_ts: None,
            oldest_rate_segment_ts: None,
            rate_until,
        }
    }

    fn into_dataset(self, _ts: Timestamp) -> Vec<DataPoint> {
        self.socket_cache.into_values().collect()
    }

    fn collect(&mut self, time_segment: &TimeSegment, clock: &ClockNano) {
        self.oldest_segment_ts.replace(time_segment.ts);
        let is_rate_eligible = self.rate_until <= time_segment.ts;
        if is_rate_eligible {
            self.oldest_rate_segment_ts.replace(time_segment.ts);
        }

        time_segment.segment.for_each_socket(|socket| {
            let stat = time_segment
                .segment
                .stat_by_interest(&Interest::LocalSocket(socket.local))
                .unwrap_or_default();

            self.socket_cache
                .entry(socket.local)
                .and_modify(|datapoint| {
                    datapoint.stat.merge(&stat);
                    if is_rate_eligible {
                        datapoint.rate_stat.merge(&stat);
                    }
                })
                .or_insert_with(|| DataPoint {
                    socket: *socket,
                    stat,
                    last_activity: clock.wall_time(time_segment.ts),
                    rate_stat: if is_rate_eligible {
                        stat
                    } else {
                        Stat::default()
                    },
                    pid: socket.pid,
                });
        });
    }
}
