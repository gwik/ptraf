use std::{borrow::Cow, net::IpAddr};

use tui::{
    style::{Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::promise::Promise;

use super::{styles::Styled, UiContext, View};

#[derive(Debug)]
pub(super) struct RemoteIpDetailsView {
    ip: IpAddr,
    hostname: Promise<String>,
}

impl RemoteIpDetailsView {
    pub(super) fn new(ip: IpAddr) -> Self {
        let hostname = Promise::spawn_blocking(move || {
            dns_lookup::lookup_addr(&ip).unwrap_or_else(|e| format!("[FAILED: {e}]"))
        });

        Self { ip, hostname }
    }
}

impl View for RemoteIpDetailsView {
    fn render<B: tui::backend::Backend>(
        &mut self,
        frame: &mut tui::Frame<B>,
        rect: tui::layout::Rect,
        _ctx: &UiContext<'_>,
    ) {
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            format!("remote IP: {}", self.ip),
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ));

        let text = vec![Spans::from(vec![
            Styled::label_span("hostname: "),
            Span::styled(
                self.hostname
                    .value()
                    .map(|hostname| Cow::Borrowed(hostname.as_str()))
                    .unwrap_or(Cow::Borrowed("[RESOLVING]")),
                Style::default(),
            ),
        ])];

        let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: true });
        frame.render_widget(paragraph, rect);
    }
}
