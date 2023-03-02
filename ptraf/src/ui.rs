pub mod socktable;

use std::ops::{Deref, DerefMut};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};
use std::{io, sync::Arc};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::stream::StreamExt;
use tui::widgets::{Cell, Table};
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Row},
    Frame, Terminal,
};

use crate::clock::ClockNano;
use crate::store::Store;

pub struct App {
    clock: ClockNano,
    store: Store,
    sock_table: RwLock<socktable::SocketTable>,
}

impl App {
    pub fn new(clock: ClockNano, store: Store) -> Self {
        // TODO(gwik): config
        let sock_table = socktable::SocketTableConfig::default().build();
        let sock_table = RwLock::new(sock_table);
        Self {
            sock_table,
            store,
            clock,
        }
    }

    pub fn clock(&self) -> &ClockNano {
        &self.clock
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn sock_table(&self) -> impl Deref<Target = socktable::SocketTable> + '_ {
        self.sock_table.read().unwrap()
    }

    pub(crate) fn sock_table_mut(&self) -> impl DerefMut<Target = socktable::SocketTable> + '_ {
        self.sock_table.write().unwrap()
    }

    async fn collect(&self, rate: Duration) -> Result<(), anyhow::Error> {
        loop {
            tokio::time::sleep(rate).await;

            let ts = self.clock.now();

            let sock_table = &mut self.sock_table.write().unwrap();
            sock_table.collect(ts, &self.clock, &self.store);
        }
    }

    /*
        async fn run_store(&self) {
            let freq = Duration::from_millis(250); // TODO(gwik): config
            let mut now = self.clock.now();

            loop {
                tokio::time::sleep(freq).await;
                let view = self.store.segments_view();

                if view.is_empty() {
                    continue;
                }

                let cur = self.clock.now();
                let (min_ts, val, packets) = view
                    .iter()
                    .rev()
                    .take_while(|segment| {
                        segment.ts.saturating_elapsed_since(&now) < freq + Duration::from_millis(250)
                    })
                    .fold((cur, 0u64, 0u64), |(_, val, packets), segment| {
                        (
                            segment.ts,
                            val + segment.segment.total(None),
                            packets + segment.segment.total_packet_count(),
                        )
                    });

                let elapsed = min_ts.saturating_elapsed_since(&cur);

                let rate = if elapsed.is_zero() {
                    0u64
                } else {
                    (val as f64 / elapsed.as_secs_f64()) as u64
                };

                let packets = if elapsed.is_zero() {
                    0u64
                } else {
                    (packets as f64 / elapsed.as_secs_f64()) as u64
                };

                debug!(
                    "segments={} rate={}/s packets={}/s elapsed={:?}",
                    view.len(),
                    humansize::format_size(rate, humansize::DECIMAL),
                    packets,
                    elapsed,
                );

                now = cur;
            }
        }
    */
}

#[derive(Debug, Clone)]
enum Action {
    Quit,
    Change,
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: Arc<App>,
    tick_rate: Duration,
) -> Result<(), anyhow::Error> {
    // FIXME(use)
    let _collect_handle = {
        let app = Arc::clone(&app);
        tokio::spawn(async move { app.collect(tick_rate).await })
    };

    let (sigtx, mut sigrx) = tokio::sync::mpsc::channel(100);

    {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut events = event::EventStream::new();
            while let Some(event) = events.next().await {
                // FIXME(gwik): handle error;
                if let Event::Key(key) = event? {
                    match key.code {
                        KeyCode::Char('q') => {
                            sigtx.send(Action::Quit).await?;
                            return Ok(());
                        }
                        KeyCode::Up => {
                            app.sock_table_mut().up();
                            sigtx.send(Action::Change).await?;
                        }
                        KeyCode::Down => {
                            app.sock_table_mut().down();
                            sigtx.send(Action::Change).await?;
                        }
                        _ => {}
                    }
                }
            }

            Ok::<_, anyhow::Error>(())
        });
    }

    let mut last_update = Instant::now();
    let mut needs_display = true;

    loop {
        let app = Arc::clone(&app);

        if needs_display || last_update.elapsed() > tick_rate {
            terminal.draw(|f| table_ui(f, app))?;
            last_update = Instant::now();
            needs_display = false;
        }

        let rx_fut = sigrx.recv();
        let timeout = tokio::time::sleep(tick_rate.saturating_sub(last_update.elapsed()));

        tokio::select! {
            res = rx_fut => {
                match res {
                    Some(Action::Quit) => break,
                    Some(Action::Change) => { needs_display = true },
                    _ => {},
                }
            }
            _ = timeout => {}
        };
    }

    // collect_handle.await??;
    Ok(())
}

pub async fn run_ui(app: Arc<App>, tick_rate: Duration) -> Result<(), anyhow::Error> {
    // setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    terminal.clear()?;

    let res = run_app(&mut terminal, app, tick_rate).await;

    // restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen,)?;
    terminal.show_cursor()?;

    res
}

fn table_ui<B: Backend>(f: &mut Frame<B>, app: Arc<App>) {
    let rects = Layout::default()
        .constraints([Constraint::Percentage(100)].as_ref())
        .split(f.size());

    let now = SystemTime::now();

    let selected_style = Style::default().add_modifier(Modifier::REVERSED);
    let normal_style = Style::default().bg(Color::DarkGray);

    let sock_table = app.sock_table();
    // FIXME(gwik): find a better place for this so that we don't need to lock/clone it.
    let mut table_state = sock_table.table_state().clone();
    let rate_duration = sock_table
        .rate_collection_range()
        .map(|range| range.start.saturating_elapsed_since(&range.end))
        .filter(|duration| !duration.is_zero());

    let header_cells = [
        "local".to_string(),
        "remote".to_string(),
        "last activity".to_string(),
        "pid".to_string(),
        format!("rx/s [{:?}]", rate_duration.unwrap_or_default()),
        format!("tx/s [{:?}]", rate_duration.unwrap_or_default()),
    ]
    .into_iter()
    .map(|h| Cell::from(h).style(Style::default().fg(Color::Yellow)));
    let header = Row::new(header_cells).style(normal_style).height(1);

    let formatter = Formatter::default();

    let rows = sock_table.dataset().iter().map(|datapoint| {
        let cells = [
            Cell::from(datapoint.socket.local.to_string()),
            Cell::from(datapoint.socket.remote.to_string()),
            Cell::from(format!(
                "{:?}",
                now.duration_since(datapoint.last_activity)
                    .ok()
                    .unwrap_or_default()
            )),
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
        .highlight_symbol(">> ")
        .widths(&[
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
        ]);
    f.render_stateful_widget(t, rects[0], &mut table_state);
}

struct Formatter(humansize::FormatSizeOptions);

impl Default for Formatter {
    fn default() -> Self {
        Self(
            humansize::FormatSizeOptions::from(humansize::DECIMAL)
                .base_unit(humansize::BaseUnit::Bit),
        )
    }
}

impl Formatter {
    pub fn format_rate(&self, rate_duration: Option<Duration>, val: u64) -> String {
        rate_duration
            .map(|rate_duration| {
                let rate = val * 8
                    / (rate_duration.as_secs() * 1000 + rate_duration.subsec_millis() as u64)
                    * 1000;
                humansize::format_size(rate as u64, self.0)
            })
            .unwrap_or_default()
    }
}
