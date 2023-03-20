use std::{
    collections::HashMap,
    net::SocketAddr,
    ops::Range,
    time::{Duration, SystemTime},
};

use human_repr::HumanDuration;
use tui::{
    backend::Backend,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Cell, Row, Table, TableState},
    Frame,
};

use crate::{
    clock::{ClockNano, Timestamp},
    store::{Interest, Socket, Stat, Store, TimeSegment},
};

use super::{format::Formatter, Filter, UiContext};

#[derive(Debug)]
pub(crate) struct SocketTableConfig {
    filter: Filter,
    collection_window: Duration,
    rate_window: Duration,
}

impl Default for SocketTableConfig {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            collection_window: Duration::from_secs(300), // 5 min
            rate_window: Duration::from_secs(1),
        }
    }
}

impl SocketTableConfig {
    pub(crate) fn build(self) -> SocketTable {
        SocketTable::new(self)
    }

    pub(crate) fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    #[allow(unused)]
    pub(crate) fn rate_window(mut self, window: Duration) -> Self {
        self.rate_window = window;
        self
    }

    #[allow(unused)]
    pub(crate) fn collection_window(mut self, window: Duration) -> Self {
        self.collection_window = window;
        self
    }
}

#[derive(Debug)]
pub(crate) struct SocketTable {
    filter: Filter,
    dataset: Vec<Entry>,
    rate_collection_range: Option<Range<Timestamp>>,
    config: SocketTableConfig, // Remove this
}

impl SocketTable {
    pub fn new(config: SocketTableConfig) -> Self {
        Self {
            filter: config.filter,
            dataset: Vec::default(),
            rate_collection_range: None,
            config,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.dataset.len()
    }

    #[allow(unused)]
    pub fn config(&self) -> &SocketTableConfig {
        &self.config
    }

    pub fn dataset(&self) -> &[Entry] {
        &self.dataset
    }

    pub fn rate_collection_range(&self) -> Option<&Range<Timestamp>> {
        self.rate_collection_range.as_ref()
    }

    pub fn collect(&mut self, ts: Timestamp, clock: &ClockNano, store: &Store) {
        let window = store.window();

        let ts = ts.trunc(window);

        let rate_until: Timestamp =
            ts.0.saturating_sub(self.config.rate_window)
                .max(window)
                .into();
        let rate_until = rate_until.trunc(window);

        let mut collector = SocketTableCollector::new(self.filter, rate_until);

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
pub(crate) struct Entry {
    pub socket: Socket,
    pub stat: Stat,
    pub last_activity: SystemTime,
    pub rate_stat: Stat,
    pub pid: u32,
}

#[derive(Debug)]
struct SocketTableCollector {
    filter: Filter,
    socket_cache: HashMap<SocketAddr, Entry, fxhash::FxBuildHasher>,
    oldest_segment_ts: Option<Timestamp>,
    oldest_rate_segment_ts: Option<Timestamp>,
    rate_until: Timestamp,
}

impl SocketTableCollector {
    fn new(filter: Filter, rate_until: Timestamp) -> Self {
        Self {
            filter,
            socket_cache: HashMap::default(),
            oldest_segment_ts: None,
            oldest_rate_segment_ts: None,
            rate_until,
        }
    }

    fn into_dataset(self, _ts: Timestamp) -> Vec<Entry> {
        self.socket_cache.into_values().collect()
    }

    fn collect(&mut self, time_segment: &TimeSegment, clock: &ClockNano) {
        self.oldest_segment_ts.replace(time_segment.ts);
        let is_rate_eligible = self.rate_until <= time_segment.ts;
        if is_rate_eligible {
            self.oldest_rate_segment_ts.replace(time_segment.ts);
        }

        let interest = self.filter.interest();

        time_segment.segment.for_each_socket(|socket| {
            if !socket.match_interest(interest) {
                return;
            }

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
                .or_insert_with(|| Entry {
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

#[derive(Debug)]
pub(super) struct SocketTableView {
    socket_table: SocketTable,
    table_state: TableState,
}

impl Default for SocketTableView {
    fn default() -> Self {
        let socket_table = SocketTableConfig::default().build();
        Self {
            socket_table,
            table_state: TableState::default(),
        }
    }
}

impl SocketTableView {
    pub(super) fn new(socket_table: SocketTable) -> Self {
        Self {
            socket_table,
            table_state: TableState::default(),
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.socket_table.len()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn down(&mut self) {
        let selected = if self.is_empty() {
            None
        } else {
            self.table_state
                .selected()
                .map_or(0, |selected| {
                    selected.saturating_add(1).min(self.len().saturating_sub(1))
                })
                .into()
        };
        self.table_state.select(selected);
    }

    pub(super) fn up(&mut self) {
        let selected = if self.is_empty() || self.table_state.selected().is_none() {
            None
        } else {
            self.table_state
                .selected()
                .unwrap()
                .saturating_sub(1)
                .min(self.len().saturating_sub(1))
                .into()
        };
        self.table_state.select(selected);
    }

    pub(super) fn selected_pid(&self) -> Option<u32> {
        self.table_state.selected().and_then(|selected| {
            self.socket_table
                .dataset()
                .get(selected)
                .map(|item| item.pid)
        })
    }

    pub(super) fn selected(&self) -> Option<&Entry> {
        self.table_state
            .selected()
            .and_then(|selected| self.socket_table.dataset().get(selected))
    }

    pub(super) fn render<B: Backend>(
        &mut self,
        frame: &mut Frame<B>,
        rect: Rect,
        ctx: &UiContext<'_>,
    ) {
        if !ctx.paused {
            self.socket_table.collect(ctx.ts, ctx.clock, ctx.store);
        }

        let now = SystemTime::now();

        let selected_style = Style::default().add_modifier(Modifier::REVERSED);
        let normal_style = Style::default().bg(Color::DarkGray);

        let rate_duration = self
            .socket_table
            .rate_collection_range()
            .map(|range| range.start.saturating_elapsed_since(&range.end))
            .filter(|duration| !duration.is_zero());

        let header_cells = [
            "local".to_string(),
            "remote".to_string(),
            "type".to_string(),
            "last activity".to_string(),
            "pid".to_string(),
            "process".to_string(),
            "rx/s".to_string(),
            "tx/s".to_string(),
        ]
        .into_iter()
        .map(|h| Cell::from(h).style(Style::default().fg(Color::Yellow)));
        let header = Row::new(header_cells).style(normal_style).height(1);

        let formatter = Formatter::default();

        let rows = self.socket_table.dataset().iter().map(|datapoint| {
            let last_activity = now
                .duration_since(datapoint.last_activity)
                .unwrap_or_default();

            let cells = [
                Cell::from(datapoint.socket.local.to_string()),
                Cell::from(datapoint.socket.remote.to_string()),
                Cell::from(datapoint.socket.sock_type.to_string()),
                Cell::from(last_activity.human_duration().to_string()),
                Cell::from(datapoint.pid.to_string()),
                Cell::from(pid_name(datapoint.pid)),
                Cell::from(formatter.format_rate(rate_duration, datapoint.rate_stat.rx)),
                Cell::from(formatter.format_rate(rate_duration, datapoint.rate_stat.tx)),
            ];
            Row::new(cells) // style
        });

        let t = Table::new(rows)
            .header(header)
            .highlight_style(selected_style)
            .highlight_symbol("> ")
            .widths(&[
                Constraint::Percentage(22),
                Constraint::Percentage(22),
                Constraint::Min(10),
                Constraint::Percentage(10),
                Constraint::Percentage(5),
                Constraint::Percentage(11),
                Constraint::Percentage(10),
                Constraint::Percentage(10),
            ]);
        frame.render_stateful_widget(t, rect, &mut self.table_state);
    }
}

fn pid_name(pid: u32) -> String {
    procfs::process::Process::new(pid as i32)
        .ok()
        .and_then(|proc| proc.exe().ok())
        .as_ref()
        .and_then(|exe| exe.iter().last())
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_default()
}
