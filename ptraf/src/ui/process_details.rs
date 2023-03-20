use std::collections::HashSet;

use procfs::{
    net::{TcpNetEntry, TcpState, UdpNetEntry, UdpState},
    process::{FDInfo, FDTarget, Process},
};
use tui::{
    backend::Backend,
    layout::Rect,
    style::{Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::{styles::Styled, UiContext};

#[derive(Debug)]
pub(super) struct ProcessDetails {
    pid: i32,
    name: Option<String>,
    exe: Option<String>,
}

impl ProcessDetails {
    pub(super) fn empty(pid: i32) -> Self {
        Self {
            pid,
            name: None,
            exe: None,
        }
    }

    fn process(&self) -> Option<procfs::process::Process> {
        Process::new(self.pid).ok()
    }

    fn fd_sockets(&self) -> impl Iterator<Item = FDInfo> {
        self.process()
            .and_then(|proc| proc.fd().ok())
            .into_iter()
            .flatten()
            .flat_map(|fd| {
                fd.ok()
                    .filter(|fd| matches!(fd.target, FDTarget::Socket(_)))
            })
    }

    fn socket_inodes(&self) -> impl Iterator<Item = u64> {
        self.fd_sockets().flat_map(|fd| {
            if let FDTarget::Socket(inode) = fd.target {
                inode.into()
            } else {
                None
            }
        })
    }

    pub(super) fn tcp_conns(&self) -> impl Iterator<Item = TcpNetEntry> {
        self.process()
            .map(|proc| {
                proc.tcp()
                    .ok()
                    .into_iter()
                    .flatten()
                    .chain(proc.tcp6().ok().into_iter().flatten())
            })
            .into_iter()
            .flatten()
    }

    pub(super) fn udp_conns(&self) -> impl Iterator<Item = UdpNetEntry> {
        self.process()
            .map(|proc| {
                proc.udp()
                    .ok()
                    .into_iter()
                    .flatten()
                    .chain(proc.udp6().ok().into_iter().flatten())
            })
            .into_iter()
            .flatten()
    }

    pub(super) fn from_procfs(process: procfs::process::Process) -> Self {
        let exe = process.exe().ok();

        Self {
            pid: process.pid(),
            exe: exe
                .as_ref()
                .and_then(|exe| exe.to_str())
                .map(|exe| exe.to_string()),
            name: exe
                .as_ref()
                .and_then(|exe| exe.iter().last())
                .and_then(|name| name.to_str())
                .map(|name| name.to_string()),
        }
    }

    pub(super) fn from_procfs_pid(pid: i32) -> Self {
        procfs::process::Process::new(pid)
            .ok()
            .map(Self::from_procfs)
            .unwrap_or_else(|| Self::empty(pid))
    }
}

#[derive(Debug)]
pub(super) struct ProcessDetailsView {
    pid: u32,
    details: ProcessDetails,
}

impl ProcessDetailsView {
    pub(super) fn new(pid: u32) -> Self {
        Self {
            pid,
            // FIXME(gwik): i32 ?
            details: ProcessDetails::from_procfs_pid(pid as i32),
        }
    }

    pub(super) fn render<B: Backend>(
        &mut self,
        frame: &mut Frame<B>,
        rect: Rect,
        _ctx: &UiContext<'_>,
    ) {
        let title_style = Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED);
        let title = match &self.details.name {
            Some(name) => format!("{name} ({})", self.pid),
            None => format!("<PID {}>", self.pid),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(title, title_style));

        let inodes: HashSet<_> = self.details.socket_inodes().collect();

        let text = vec![
            Spans::from(vec![
                Styled::label_span("exe "),
                Span::styled(
                    self.details.exe.clone().unwrap_or_default(),
                    Style::default(),
                ),
            ]),
            Spans::from(vec![
                Styled::label_span("established tcp conns: "),
                Span::styled(
                    self.details
                        .tcp_conns()
                        .filter(|conn| {
                            conn.state == TcpState::Established && inodes.contains(&conn.inode)
                        })
                        .count()
                        .to_string(),
                    Style::default(),
                ),
            ]),
            Spans::from(vec![
                Styled::label_span("established udp conns: "),
                Span::styled(
                    self.details
                        .udp_conns()
                        .filter(|conn| {
                            conn.state == UdpState::Established && inodes.contains(&conn.inode)
                        })
                        .count()
                        .to_string(),
                    Style::default(),
                ),
            ]),
        ];

        let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: true });

        frame.render_widget(paragraph, rect);
    }
}
