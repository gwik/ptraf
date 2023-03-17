use std::ops::Deref;
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
use tui::style::Style;
use tui::widgets::Paragraph;
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    Frame, Terminal,
};

use crate::clock::{ClockNano, Timestamp};
use crate::store::{Interest, Store};

use self::socktable::{SocketTableConfig, SocketTableView};
use self::traffic_sparkline::TrafficSparkline;

mod socktable;
mod traffic_sparkline;

pub struct App {
    clock: ClockNano,
    store: Store,
    traffic: RwLock<traffic_sparkline::TrafficSparkline>,
}

impl App {
    pub fn new(clock: ClockNano, store: Store) -> Self {
        let traffic = traffic_sparkline::TrafficSparkline::default();
        let traffic = RwLock::new(traffic);

        Self {
            traffic,
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

    pub(crate) fn traffic(&self) -> impl Deref<Target = traffic_sparkline::TrafficSparkline> + '_ {
        self.traffic.read().unwrap()
    }

    async fn collect(&self, rate: Duration) -> Result<(), anyhow::Error> {
        loop {
            tokio::time::sleep(rate).await;

            let ts = self.store.oldest_timestamp(self.clock.now());
            {
                let traffic = &mut self.traffic.write().unwrap();
                traffic.collect(ts, &self.clock, &self.store);
            }
        }
    }
}

#[derive(Debug, Clone)]
enum UiEvent {
    Quit,
    Change,
    Back,
    SelectProcess(u32),
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

    let mut last_update = Instant::now();
    let mut ui = Ui::default();

    let mut events = event::EventStream::new();

    loop {
        let app = Arc::clone(&app);
        let collect_handle = &mut collect_handle;

        if ui.needs_display() || last_update.elapsed() > tick_rate {
            terminal.draw(|f| ui.render(f, &app))?;
            last_update = Instant::now();
        }

        let timeout = tokio::time::sleep(tick_rate.saturating_sub(last_update.elapsed()));

        tokio::select! {
            event = events.next() => {
                // FIXME(gwik): exit on error ?
                if let Some(Ok(event)) = event {
                    if matches!(ui.handle_event(&event), Some(UiEvent::Quit)) {
                            return Ok(());
                    };
                }
            }
            res = collect_handle => {
                return res.context("collect task exited").and_then(|task_result| task_result);
            }
            _ = timeout => {}
        };
    }
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

#[derive(Debug, Default)]
struct TrafficSparklineView {}

impl TrafficSparklineView {
    fn render<B: Backend>(&self, frame: &mut Frame<B>, rect: Rect, traffic: &TrafficSparkline) {
        // TODO(gwik): move rendering here
        traffic_sparkline::traffic_sparkline_ui(frame, rect, traffic);
    }
}

#[derive(Debug, Default)]
struct FooterBar {}

trait FrameRenderer {
    fn render<B: Backend>(&self, frame: &mut Frame<B>);
}

impl FooterBar {
    fn render<B: Backend>(&self, frame: &mut Frame<B>, rect: Rect, paused: bool) {
        let paragraph = if paused {
            let style = Style::default().bg(tui::style::Color::Red);
            Paragraph::new(" PAUSED (press SpaceBar to run)").style(style)
        } else {
            let style = Style::default().bg(tui::style::Color::DarkGray);
            Paragraph::new(" RUNNING (press SpaceBar to pause)").style(style)
        };

        frame.render_widget(paragraph, rect);
    }
}

struct UiContext<'a> {
    ts: Timestamp,
    store: &'a Store,
    clock: &'a ClockNano,
    traffic: &'a TrafficSparkline,
    paused: bool,
}

trait View<B: Backend> {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        let _ = event;
        None
    }

    fn render(&mut self, f: &mut Frame<B>, ctx: &UiContext<'_>);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    NoFilter,
    Process(u32),
}

impl Filter {
    fn interest(self) -> Option<Interest> {
        match self {
            Self::Process(pid) => Interest::Pid(pid).into(),
            Self::NoFilter => None,
        }
    }
}

struct Ui<B> {
    paused: bool,
    dirty: bool,
    filter: Filter,
    view: Box<dyn View<B> + Send>,
}

impl<B: Backend> Ui<B> {
    fn render(&mut self, frame: &mut Frame<B>, app: &App) {
        self.dirty = false;

        let ctx = UiContext {
            ts: app.clock().now(),
            clock: app.clock(),
            store: &app.store,
            traffic: &app.traffic(),
            paused: self.paused,
        };

        self.view.render(frame, &ctx);
    }
}

impl<B: Backend> Default for Ui<B> {
    fn default() -> Self {
        Self {
            paused: false,
            dirty: true,
            filter: Filter::default(),
            view: Box::new(MainView::default()),
        }
    }
}

impl<B: Backend> Ui<B> {
    #[inline]
    fn set_dirty(&mut self) {
        self.dirty = true;
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    fn needs_display(&self) -> bool {
        self.dirty
    }

    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        if let Some(ui_event) = self.view.handle_event(event) {
            match ui_event {
                UiEvent::SelectProcess(pid) => {
                    self.update_filter(Filter::Process(pid));
                }
                UiEvent::Back => {
                    self.update_filter(Filter::NoFilter);
                }
                _ => {}
            }

            self.dirty = true;
            return ui_event.into();
        }

        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Char('q') => {
                    return UiEvent::Quit.into();
                }
                KeyCode::Char(' ') => {
                    self.set_dirty();
                    self.toggle_pause();
                    return UiEvent::Change.into();
                }
                _ => {}
            }
        }

        None
    }

    fn update_filter(&mut self, filter: Filter) -> bool {
        if self.filter == filter {
            false
        } else {
            self.filter = filter;
            self.update_view();
            true
        }
    }

    fn set_process(&mut self, process: Process) {
        self.view = Box::new(ProcessView::new(process))
    }

    fn reset(&mut self) {
        self.view = Box::new(MainView::default())
    }

    fn update_view(&mut self) {
        match self.filter {
            Filter::NoFilter => self.reset(),
            Filter::Process(pid) => self.set_process(Process {
                pid,
                name: "".to_string(),
            }),
        }
    }
}

#[derive(Debug, Default)]
struct MainView {
    traffic_sparkline_view: TrafficSparklineView,
    sock_table_view: SocketTableView,
    footer_bar: FooterBar,
}

impl<B: Backend> View<B> for MainView {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.sock_table_view.up();
                    return UiEvent::Change.into();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.sock_table_view.down();
                    return UiEvent::Change.into();
                }
                KeyCode::Char('p') => {
                    return self
                        .sock_table_view
                        .selected_pid()
                        .map(UiEvent::SelectProcess)
                }
                _ => {}
            }
        }

        None
    }

    fn render(&mut self, frame: &mut Frame<B>, ctx: &UiContext<'_>) {
        let rects = Layout::default()
            .constraints(
                [
                    Constraint::Percentage(12),
                    Constraint::Percentage(87),
                    Constraint::Min(1),
                ]
                .as_ref(),
            )
            .split(frame.size());

        self.traffic_sparkline_view
            .render(frame, rects[0], ctx.traffic);

        self.sock_table_view.render(frame, rects[1], ctx);

        self.footer_bar.render(frame, rects[2], ctx.paused);
    }
}

#[derive(Debug, Clone)]
struct Process {
    pid: u32,
    name: String,
}

struct ProcessView {
    process: Process,

    traffic_sparkline_view: TrafficSparklineView,
    sock_table_view: SocketTableView,
    footer_bar: FooterBar,
}

impl ProcessView {
    fn new(process: Process) -> Self {
        let pid = process.pid;
        // TODO(gwik): config
        let socket_table = SocketTableConfig::default()
            .filter(Filter::Process(pid))
            .build();

        Self {
            process,
            traffic_sparkline_view: TrafficSparklineView::default(),
            sock_table_view: SocketTableView::new(socket_table),
            footer_bar: FooterBar::default(),
        }
    }
}

impl<B: Backend> View<B> for ProcessView {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Char('q') | KeyCode::Backspace => {
                    return UiEvent::Back.into();
                }

                KeyCode::Up | KeyCode::Char('k') => {
                    self.sock_table_view.up();
                    return UiEvent::Change.into();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.sock_table_view.down();
                    return UiEvent::Change.into();
                }
                _ => {}
            }
        }

        None
    }

    fn render(&mut self, frame: &mut Frame<B>, ctx: &UiContext<'_>) {
        let rects = Layout::default()
            .constraints(
                [
                    Constraint::Percentage(12),
                    Constraint::Percentage(87),
                    Constraint::Min(1),
                ]
                .as_ref(),
            )
            .split(frame.size());

        self.traffic_sparkline_view
            .render(frame, rects[0], ctx.traffic);

        self.sock_table_view.render(frame, rects[1], ctx);

        self.footer_bar.render(frame, rects[2], ctx.paused);
    }
}
