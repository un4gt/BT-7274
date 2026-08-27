//! Header：居中显示当前会话标题。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::Span,
    widgets::{Block, BorderType, Paragraph, Widget},
};

use crate::app::App;
use crate::ui::theme::Palette;

pub fn render(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    let texts = app.settings.language.texts();
    let title = app.open_session().display_title(texts.new_session_title);
    Paragraph::new(title)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    " BT-7274 ",
                    Style::default()
                        .fg(palette.primary)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Span::styled(texts.header_chat, palette.muted))
                .border_style(palette.border(false))
                .style(palette.surface()),
        )
        .centered()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD)
        .render(area, buf);
}
