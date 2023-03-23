use std::net::IpAddr;
use std::time::{Duration, Instant};
use std::{io, sync::Arc};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::stream::StreamExt;
use ptraf_filter::Interpretor;
use tui::layout::Rect;
use tui::style::Style;
use tui::text::{Span, Spans};
use tui::widgets::Paragraph;
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    Frame, Terminal,
};

use crate::clock::{ClockNano, Timestamp};
use crate::store::{Interest, Store};

use self::process_details::ProcessDetailsView;
use self::remote_ip_details::RemoteIpDetailsView;
use self::socktable::{SocketTableConfig, SocketTableView};
use self::traffic_sparkline::TrafficSparklineView;

mod filter_editor;
mod format;
mod process_details;
mod remote_ip_details;
mod socktable;
mod styles;
mod traffic_sparkline;

pub struct App {
    clock: ClockNano,
    store: Store,
}

impl App {
    pub fn new(clock: ClockNano, store: Store) -> Self {
        Self { store, clock }
    }

    pub fn clock(&self) -> &ClockNano {
        &self.clock
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
}

#[derive(Debug, Clone)]
enum UiEvent {
    Quit,
    Change,
    Back,
    SelectProcess(u32),
    SelectRemoteIp(IpAddr),
    SetCustomFilter(Option<CustomFilter>),
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: Arc<App>,
    tick_rate: Duration,
) -> Result<(), anyhow::Error> {
    let mut last_update = Instant::now();
    let mut ui = Ui::default();

    let mut events = event::EventStream::new();

    loop {
        let app = Arc::clone(&app);

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
struct FooterBar {}

trait FrameRenderer {
    fn render<B: Backend>(&self, frame: &mut Frame<B>);
}

impl FooterBar {
    fn render<B: Backend>(&self, frame: &mut Frame<B>, rect: Rect, paused: bool) {
        let paragraph = if paused {
            let style = Style::default().bg(tui::style::Color::Red);
            Paragraph::new(Spans::from(vec![Span::from(
                " PAUSED (press SpaceBar to run) -- UP/DOWN: k/j, - FILTERS: p (process), r (remote IP) - QUIT/BACK: q",
            )]))
            .style(style)
        } else {
            let style = Style::default().bg(tui::style::Color::DarkGray);
            Paragraph::new(
                " RUNNING (press SpaceBar to pause) -- UP/DOWN: k/j, - FILTERS: p (process), r (remote IP) - QUIT/BACK: q",
            )
            .style(style)
        };

        frame.render_widget(paragraph, rect);
    }
}

struct UiContext<'a> {
    ts: Timestamp,
    store: &'a Store,
    clock: &'a ClockNano,
    paused: bool,
}

trait View {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        let _ = event;
        None
    }

    fn render<B: Backend>(&mut self, f: &mut Frame<B>, rect: Rect, ctx: &UiContext<'_>);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    None,
    Process(u32),
    RemoteIp(IpAddr),
}

impl Filter {
    pub(crate) fn interest(self) -> Interest {
        match self {
            Self::None => Interest::All,
            Self::Process(pid) => Interest::Pid(pid),
            Self::RemoteIp(ip) => Interest::RemoteIp(ip),
        }
    }
}

#[derive(Debug)]
enum RootView {
    Main(MainView),
    Process(ProcessView),
    RemoteIp(RemoteIpView),
}

impl Default for RootView {
    fn default() -> Self {
        Self::Main(MainView::default())
    }
}

impl View for RootView {
    #[inline]
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        match self {
            RootView::Main(inner) => inner.handle_event(event),
            RootView::Process(inner) => inner.handle_event(event),
            RootView::RemoteIp(inner) => inner.handle_event(event),
        }
    }

    #[inline]
    fn render<B: Backend>(&mut self, f: &mut Frame<B>, rect: Rect, ctx: &UiContext<'_>) {
        match self {
            RootView::Main(inner) => inner.render(f, rect, ctx),
            RootView::Process(inner) => inner.render(f, rect, ctx),
            RootView::RemoteIp(inner) => inner.render(f, rect, ctx),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CustomFilter {
    content: String,
    interpretor: Interpretor,
}

struct Ui {
    paused: bool,
    dirty: bool,
    filter: Filter,
    custom_filter: Option<CustomFilter>,
    view: RootView,
    footer: FooterBar,
}

impl Ui {
    fn render<B: Backend>(&mut self, frame: &mut Frame<B>, app: &App) {
        self.dirty = false;

        // FIXME(gwik): ugly hack to force segments creation when no traffic.
        let ts = app.clock().now();
        let ts = app.store.oldest_timestamp(ts);

        let ctx = UiContext {
            ts,
            clock: app.clock(),
            store: &app.store,
            paused: self.paused,
        };

        let rects = Layout::default()
            .constraints(vec![Constraint::Ratio(9999, 10000), Constraint::Length(1)])
            .split(frame.size());

        self.view.render(frame, rects[0], &ctx);
        self.footer.render(frame, rects[1], ctx.paused);
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            paused: false,
            dirty: true,
            filter: Filter::default(),
            custom_filter: None,
            #[allow(clippy::box_default)]
            view: RootView::Main(MainView::default()),
            footer: FooterBar::default(),
        }
    }
}

impl Ui {
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
            self.dirty = true;

            match ui_event {
                UiEvent::SelectRemoteIp(ip) => {
                    self.update_filter(Filter::RemoteIp(ip));
                }
                UiEvent::SelectProcess(pid) => {
                    self.update_filter(Filter::Process(pid));
                }
                UiEvent::Back => {
                    self.update_filter(Filter::None);
                }
                UiEvent::SetCustomFilter(filter) => {
                    self.custom_filter = filter
                }
                _ => return ui_event.into(),
            }
            return None;
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

    fn update_view(&mut self) {
        self.view = match self.filter {
            #[allow(clippy::box_default)]
            Filter::None => RootView::Main(MainView::new(self.custom_filter.as_ref())),
            Filter::Process(pid) => {
                RootView::Process(ProcessView::new(pid, self.custom_filter.as_ref()))
            }
            Filter::RemoteIp(ipaddr) => {
                RootView::RemoteIp(RemoteIpView::new(ipaddr, self.custom_filter.as_ref()))
            }
        }
    }
}

#[derive(Debug, Default)]
struct MainView {
    traffic_sparkline_view: TrafficSparklineView,
    sock_table_view: SocketTableView,
}

impl MainView {
    fn new(custom_filter: Option<&CustomFilter>) -> Self {
        Self {
            traffic_sparkline_view: TrafficSparklineView::default(),
            sock_table_view: SocketTableView::new(
                SocketTableConfig::default().build(),
                custom_filter,
            ),
        }
    }
}

impl View for MainView {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        if let Some(ui_event) = self.sock_table_view.handle_event(event) {
            return Some(ui_event);
        }

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
                KeyCode::Char('p') | KeyCode::Enter => {
                    return self
                        .sock_table_view
                        .selected_pid()
                        .map(UiEvent::SelectProcess)
                }
                KeyCode::Char('r') => {
                    return self
                        .sock_table_view
                        .selected()
                        .map(|entry| UiEvent::SelectRemoteIp(entry.socket.remote.ip()))
                }
                _ => {}
            }
        }

        None
    }

    fn render<B: Backend>(&mut self, frame: &mut Frame<B>, rect: Rect, ctx: &UiContext<'_>) {
        let rects = Layout::default()
            .constraints([Constraint::Percentage(13), Constraint::Percentage(87)].as_ref())
            .split(rect);

        self.traffic_sparkline_view.render(frame, rects[0], ctx);

        self.sock_table_view.render(frame, rects[1], ctx);
    }
}

#[derive(Debug)]
struct RemoteIpView {
    remote_ip_details_view: RemoteIpDetailsView,
    traffic_sparkline_view: TrafficSparklineView,
    sock_table_view: SocketTableView,
}

impl RemoteIpView {
    fn new(ipaddr: IpAddr, custom_filter: Option<&CustomFilter>) -> Self {
        let socket_table = SocketTableConfig::default()
            .filter(Filter::RemoteIp(ipaddr))
            .build();

        Self {
            remote_ip_details_view: RemoteIpDetailsView::new(ipaddr),
            traffic_sparkline_view: TrafficSparklineView::with_filter(Filter::RemoteIp(ipaddr)),
            sock_table_view: SocketTableView::new(socket_table, custom_filter),
        }
    }
}

impl View for RemoteIpView {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        if let Some(ui_event) = self.sock_table_view.handle_event(event) {
            return Some(ui_event);
        }

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
                KeyCode::Char('p') | KeyCode::Enter => {
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

    fn render<B: Backend>(&mut self, frame: &mut Frame<B>, rect: Rect, ctx: &UiContext<'_>) {
        let rects = Layout::default()
            .constraints(
                [
                    Constraint::Percentage(15),
                    Constraint::Percentage(15),
                    Constraint::Percentage(70),
                ]
                .as_ref(),
            )
            .split(rect);

        self.remote_ip_details_view.render(frame, rects[0], ctx);

        self.traffic_sparkline_view.render(frame, rects[1], ctx);

        self.sock_table_view.render(frame, rects[2], ctx);
    }
}

#[derive(Debug)]
struct ProcessView {
    process_details_view: ProcessDetailsView,
    traffic_sparkline_view: TrafficSparklineView,
    sock_table_view: SocketTableView,
}

impl ProcessView {
    fn new(pid: u32, custom_filter: Option<&CustomFilter>) -> Self {
        let socket_table = SocketTableConfig::default()
            .filter(Filter::Process(pid))
            .build();

        Self {
            process_details_view: ProcessDetailsView::new(pid),
            traffic_sparkline_view: TrafficSparklineView::with_filter(Filter::Process(pid)),
            sock_table_view: SocketTableView::new(socket_table, custom_filter),
        }
    }
}

impl View for ProcessView {
    fn handle_event(&mut self, event: &Event) -> Option<UiEvent> {
        if let Some(ui_event) = self.sock_table_view.handle_event(event) {
            return Some(ui_event);
        }

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
                KeyCode::Char('r') => {
                    return self
                        .sock_table_view
                        .selected()
                        .map(|entry| UiEvent::SelectRemoteIp(entry.socket.remote.ip()))
                }
                _ => {}
            }
        }

        None
    }

    fn render<B: Backend>(&mut self, frame: &mut Frame<B>, rect: Rect, ctx: &UiContext<'_>) {
        let rects = Layout::default()
            .constraints(
                [
                    Constraint::Percentage(15),
                    Constraint::Percentage(15),
                    Constraint::Percentage(70),
                ]
                .as_ref(),
            )
            .split(rect);

        self.process_details_view.render(frame, rects[0], ctx);

        self.traffic_sparkline_view.render(frame, rects[1], ctx);

        self.sock_table_view.render(frame, rects[2], ctx);
    }
}
