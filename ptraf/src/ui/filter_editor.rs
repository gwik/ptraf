use crossterm::event::Event;
use std::fmt::Debug;
use tui::backend::Backend;
use tui::layout::{Constraint, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Block, Borders};
use tui::Frame;
use tui_textarea::{Input, Key, TextArea};

use ptraf_filter::Interpretor;

use super::{CustomFilter, UiContext, UiEvent, View};

pub(super) struct FilterView {
    committed_state: Option<CustomFilter>,
    draft_interpretor: Result<Option<Interpretor>, ptraf_filter::Error>,
    textarea: TextArea<'static>,
    editing: bool,
}

impl Default for FilterView {
    fn default() -> Self {
        Self {
            committed_state: None,
            draft_interpretor: Ok(None),
            textarea: TextArea::default(),
            editing: false,
        }
    }
}

impl Debug for FilterView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterView")
            .field("active", &self.editing)
            .field("committed_state", &self.committed_state)
            .field("draft_interpretor", &self.committed_state)
            .finish_non_exhaustive()
    }
}

impl FilterView {
    pub(super) fn with_filter(filter: Option<&CustomFilter>) -> Self {
        Self {
            committed_state: filter.cloned(),
            draft_interpretor: Ok(None),
            textarea: TextArea::default(),
            editing: false,
        }
    }

    fn committed_content(&self) -> Option<&str> {
        self.committed_state
            .as_ref()
            .map(|state| state.content.as_ref())
    }

    fn committed_interpretor(&self) -> Option<&Interpretor> {
        self.committed_state
            .as_ref()
            .map(|state| &state.interpretor)
    }

    pub(super) fn is_editing(&self) -> bool {
        self.editing
    }

    pub(super) fn set_editing(&mut self) {
        self.editing = true;
        self.draft_interpretor = Ok(self.committed_interpretor().cloned());

        self.textarea = TextArea::new(vec![self
            .committed_content()
            .map(ToOwned::to_owned)
            .unwrap_or_default()]);

        self.textarea.move_cursor(tui_textarea::CursorMove::End)
    }

    pub(super) fn commit(&mut self) {
        self.editing = false;
        self.committed_state = if let Some(draft_interpretor) = self
            .draft_interpretor
            .as_mut()
            .ok()
            .and_then(|int| int.take())
        {
            CustomFilter {
                content: self
                    .textarea
                    .lines()
                    .get(0)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_default(),
                interpretor: draft_interpretor,
            }
            .into()
        } else {
            None
        }
    }

    pub(super) fn abort(&mut self) {
        self.editing = false;
        self.draft_interpretor = Ok(None);

        self.textarea = TextArea::new(vec![self
            .committed_content()
            .map(ToOwned::to_owned)
            .unwrap_or_default()]);
    }

    pub(super) fn interpretor(&self) -> Option<&Interpretor> {
        if self.is_editing() {
            self.draft_interpretor
                .as_ref()
                .ok()
                .and_then(|int| int.as_ref())
        } else {
            self.committed_interpretor()
        }
    }

    fn is_valid(&self) -> bool {
        self.draft_interpretor.is_ok()
    }

    fn draft_content(&self) -> Option<&str> {
        self.textarea
            .lines()
            .get(0)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }

    fn update(&mut self) {
        self.draft_interpretor = self.draft_content().map(Interpretor::parse).transpose();
    }
}

impl View for FilterView {
    fn handle_event(&mut self, event: &Event) -> Option<super::UiEvent> {
        if !self.is_editing() {
            return None;
        }

        match (event.clone()).into() {
            Input { key: Key::Esc, .. } => {
                self.abort();
                None
            }
            Input {
                key: Key::Char('m'),
                ctrl: true,
                ..
            }
            | Input {
                key: Key::Enter, ..
            } => {
                if self.is_valid() {
                    self.commit();
                    UiEvent::SetCustomFilter(self.committed_state.clone()).into()
                } else {
                    None
                }
            }
            input => {
                if self.textarea.input(input) {
                    self.update()
                }
                UiEvent::Change.into()
            }
        }
    }

    fn render<B: Backend>(&mut self, f: &mut Frame<B>, rect: Rect, _ctx: &UiContext<'_>) {
        self.textarea.set_cursor_line_style(Style::default());

        if self.is_editing() {
            self.textarea
                .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

            match &self.draft_interpretor {
                Ok(_) => {
                    self.textarea
                        .set_style(Style::default().fg(Color::LightGreen));
                    self.textarea.set_block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("OK - accept: Enter abort: Esc"),
                    );
                }
                Err(err) => {
                    self.textarea
                        .set_style(Style::default().fg(Color::LightRed));
                    self.textarea.set_block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!("ERROR: {}", err)),
                    );
                }
            }
        } else {
            self.textarea.set_style(Style::default());
            self.textarea.set_cursor_style(Style::default());
            self.textarea.set_block(
                Block::default()
                    .title("Filter (press '/'), ex: \"udp and (rport[443] or rport[80]) and ipv6\"")
                    .borders(Borders::ALL),
            );
        }

        let layout =
            Layout::default().constraints([Constraint::Length(3), Constraint::Min(1)].as_slice());

        let chunks = layout.split(rect);
        f.render_widget(self.textarea.widget(), chunks[0]);
    }
}
