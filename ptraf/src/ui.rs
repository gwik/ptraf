pub mod socktable;

use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use std::{io, sync::Arc};

use anyhow::Context;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::stream::StreamExt;
use tui::layout::Rect;
use tui::style::{Modifier, Style};
use tui::widgets::canvas::Label;
use tui::widgets::{Block, Paragraph};
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    Frame, Terminal,
};

use crate::clock::ClockNano;
use crate::store::Store;

pub struct App {
    clock: ClockNano,
    store: Store,
    sock_table: RwLock<socktable::SocketTable>,
    paused: AtomicBool,
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
            paused: false.into(),
        }
    }

    pub fn clock(&self) -> &ClockNano {
        &self.clock
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn toggle_pause(&self) {
        // incorrect usage of atomics but good enough for this purpose, and cheap.
        let val = self.is_paused();
        self.paused.store(!val, Relaxed);
    }

    pub(crate) fn sock_table(&self) -> impl Deref<Target = socktable::SocketTable> + '_ {
        self.sock_table.read().unwrap()
    }

    pub(crate) fn sock_table_mut(&self) -> impl DerefMut<Target = socktable::SocketTable> + '_ {
        self.sock_table.write().unwrap()
    }

    #[inline]
    fn is_paused(&self) -> bool {
        self.paused.load(Relaxed)
    }

    async fn collect(&self, rate: Duration) -> Result<(), anyhow::Error> {
        loop {
            tokio::time::sleep(rate).await;

            if !self.is_paused() {
                let ts = self.clock.now();

                let sock_table = &mut self.sock_table.write().unwrap();
                sock_table.collect(ts, &self.clock, &self.store);
            }
        }
    }
}

#[derive(Debug, Clone)]
enum Action {
    Quit,
    Pause,
    Change,
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: Arc<App>,
    tick_rate: Duration,
) -> Result<(), anyhow::Error> {
    let mut collect_handle = {
        let app = Arc::clone(&app);
        tokio::spawn(async move { app.collect(tick_rate).await })
    };

    let (sigtx, mut sigrx) = tokio::sync::mpsc::channel(32);

    let mut events_handle = {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut events = event::EventStream::new();
            while let Some(event) = events.next().await {
                if let Event::Key(key) = event? {
                    match key.code {
                        KeyCode::Char('q') => {
                            sigtx.send(Action::Quit).await?;
                            return Ok(());
                        }
                        KeyCode::Char(' ') => {
                            app.toggle_pause();
                            sigtx.send(Action::Pause).await?;
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
        })
    };

    let mut last_update = Instant::now();
    let mut needs_display = true;

    loop {
        let app = Arc::clone(&app);
        let collect_handle = &mut collect_handle;
        let events_handle = &mut events_handle;

        if needs_display || last_update.elapsed() > tick_rate {
            terminal.draw(|f| table_ui(f, &app))?;
            last_update = Instant::now();
            needs_display = false;
        }

        let rx_fut = sigrx.recv();
        let timeout = tokio::time::sleep(tick_rate.saturating_sub(last_update.elapsed()));

        tokio::select! {
            res = events_handle => {
                return res.context("event loop exited").and_then(|task_result| task_result);
            }
            res = collect_handle => {
                return res.context("collect task exited").and_then(|task_result| task_result);
            }
            action = rx_fut => {
                match action {
                    Some(Action::Quit) => break,
                    Some(Action::Change |  Action::Pause) => { needs_display = true },
                    _ => {}
                }
            }
            _ = timeout => {}
        };
    }

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

fn table_ui<B: Backend>(f: &mut Frame<B>, app: &App) {
    let rects = Layout::default()
        .constraints([Constraint::Percentage(99), Constraint::Min(1)].as_ref())
        .split(f.size());

    let socket_table = app.sock_table();
    socktable::socket_table_ui(f, rects[0], &socket_table);

    footer_bar_ui(f, rects[1], app);
}

fn footer_bar_ui<B: Backend>(f: &mut Frame<B>, rect: Rect, app: &App) {
    let paragraph = if app.is_paused() {
        let style = Style::default().bg(tui::style::Color::Red);
        Paragraph::new(" PAUSED (press SpaceBar to run)").style(style)
    } else {
        let style = Style::default().bg(tui::style::Color::DarkGray);
        Paragraph::new(" RUNNING (press SpaceBar to pause)").style(style)
    };

    f.render_widget(paragraph, rect);
}
