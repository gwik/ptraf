use crossterm::event::{Event, KeyEvent};
use std::fmt::Debug;
use std::io;
use tui::backend::Backend;
use tui::layout::{Constraint, Layout, Rect};
use tui::style::{Color, Style};
use tui::widgets::{Block, Borders};
use tui::Frame;
use tui_textarea::{Input, Key, TextArea};

use ptraf_filter::Interpretor;

use super::{UiContext, UiEvent, View};

#[derive(Default)]
pub(super) struct FilterView {
    interpretor: Option<Interpretor>,
    textarea: TextArea<'static>,
    active: bool,
}

impl Debug for FilterView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterView")
            .field("active", &self.active)
            .field("interpretor", &self.interpretor)
            .finish_non_exhaustive()
    }
}

impl FilterView {
    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    pub(super) fn set_active(&mut self) {
        // TODO(gwik): remove
        self.update();
        self.active = true;
    }

    pub(super) fn interpretor(&self) -> Option<&Interpretor> {
        self.interpretor.as_ref()
    }

    fn enabled(&self) -> bool {
        self.interpretor.is_some()
    }

    fn is_valid(&self) -> bool {
        self.interpretor.is_some() || self.content().filter(|s| s.is_empty()).is_some()
    }

    fn content(&self) -> Option<&str> {
        self.textarea
            .lines()
            .get(0)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }

    fn update(&mut self) -> bool {
        // TODO(gwik): split state and ui state.

        if let Some(result) = self.content().map(Interpretor::parse) {
            match result {
                Err(err) => {
                    self.textarea
                        .set_style(Style::default().fg(Color::LightRed));
                    self.textarea.set_block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!("ERROR: {}", err)),
                    );
                    false
                }
                Ok(interpretor) => {
                    // FIXME(gwik): repetition
                    self.textarea
                        .set_style(Style::default().fg(Color::LightGreen));
                    self.textarea
                        .set_block(Block::default().borders(Borders::ALL).title("OK"));
                    self.interpretor.replace(interpretor);
                    true
                }
            }
        } else {
            self.interpretor.take();
            self.textarea
                .set_style(Style::default().fg(Color::LightGreen));
            self.textarea
                .set_block(Block::default().borders(Borders::ALL).title("OK"));
            true
        }
    }
}

impl View for FilterView {
    fn handle_event(&mut self, event: &Event) -> Option<super::UiEvent> {
        if !self.is_active() {
            return None;
        }

        match (event.clone()).into() {
            Input { key: Key::Esc, .. } => {
                self.active = false;
                UiEvent::Back.into()
            }
            // Input {
            //     key: Key::Enter, ..
            // } if self.update() => {}
            Input {
                key: Key::Char('m'),
                ctrl: true,
                ..
            }
            | Input {
                key: Key::Enter, ..
            } => {
                self.update();
                UiEvent::Change.into()
            }
            input => {
                if self.textarea.input(input) {
                    self.update();
                }
                UiEvent::Change.into()
            }
        }
    }

    fn render<B: Backend>(&mut self, f: &mut Frame<B>, rect: Rect, _ctx: &UiContext<'_>) {
        if self.is_active() {
            self.textarea.set_cursor_line_style(Style::default());
        } else {
            self.textarea
                .set_cursor_line_style(Style::default().fg(Color::Yellow));
        }
        let layout =
            Layout::default().constraints([Constraint::Length(3), Constraint::Min(1)].as_slice());

        let chunks = layout.split(rect);
        f.render_widget(self.textarea.widget(), chunks[0]);
    }
}
