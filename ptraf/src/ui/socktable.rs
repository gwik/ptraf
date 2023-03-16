use std::{
    collections::HashMap,
    net::SocketAddr,
    ops::Range,
    time::{Duration, SystemTime},
};

use human_repr::HumanDuration;
use humansize::ToF64;
use tui::{
    backend::Backend,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table, TableState},
    Frame,
};

use crate::{
    clock::{ClockNano, Timestamp},
    store::{Interest, Socket, Stat, Store, TimeSegment},
};

#[derive(Debug)]
pub(crate) struct SocketTableConfig {
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
pub(crate) struct SocketTable {
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

pub(crate) fn socket_table_ui<B: Backend>(f: &mut Frame<B>, rect: Rect, sock_table: &SocketTable) {
    let now = SystemTime::now();

    let selected_style = Style::default().add_modifier(Modifier::REVERSED);
    let normal_style = Style::default().bg(Color::DarkGray);

    // FIXME(gwik): find a better place for this so that we don't need to lock/clone it.
    let mut table_state = sock_table.table_state().clone();
    let rate_duration = sock_table
        .rate_collection_range()
        .map(|range| range.start.saturating_elapsed_since(&range.end))
        .filter(|duration| !duration.is_zero());

    let header_cells = [
        "local".to_string(),
        "remote".to_string(),
        "type".to_string(),
        "last activity".to_string(),
        "pid".to_string(),
        "rx/s".to_string(),
        "tx/s".to_string(),
    ]
    .into_iter()
    .map(|h| Cell::from(h).style(Style::default().fg(Color::Yellow)));
    let header = Row::new(header_cells).style(normal_style).height(1);

    let formatter = Formatter::default();

    let rows = sock_table.dataset().iter().map(|datapoint| {
        let last_activity = now
            .duration_since(datapoint.last_activity)
            .unwrap_or_default();

        let cells = [
            Cell::from(datapoint.socket.local.to_string()),
            Cell::from(datapoint.socket.remote.to_string()),
            Cell::from(datapoint.socket.sock_type.to_string()),
            Cell::from(last_activity.human_duration().to_string()),
            Cell::from(datapoint.pid.to_string()),
            Cell::from(formatter.format_rate(rate_duration, datapoint.rate_stat.rx)),
            Cell::from(formatter.format_rate(rate_duration, datapoint.rate_stat.tx)),
        ];
        Row::new(cells) // style
    });
    let t = Table::new(rows)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Table"))
        .highlight_style(selected_style)
        .highlight_symbol("> ")
        .widths(&[
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Min(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
        ]);
    f.render_stateful_widget(t, rect, &mut table_state);
}

struct Formatter(humansize::FormatSizeOptions);

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
                let rate = ((val * 8).to_f64() / rate_duration.as_secs_f64())
                    .round()
                    .clamp(f64::MIN_POSITIVE, f64::MAX) as u64;
                humansize::format_size(rate, self.0)
            })
            .unwrap_or_default()
    }
}
