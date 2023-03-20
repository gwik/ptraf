use std::borrow::Cow;

use tui::{
    style::{Modifier, Style},
    text::Span,
};

pub(crate) struct Styled;

impl Styled {
    #[inline]
    pub(crate) fn label_style() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    #[inline]
    pub(crate) fn label_span<'a, T>(text: T) -> Span<'a>
    where
        T: Into<Cow<'a, str>>,
    {
        Span::styled(text, Self::label_style())
    }
}
