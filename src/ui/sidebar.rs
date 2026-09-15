//! Sidebar：上 90% 历史会话列表，下 10% Settings 入口。

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::app::{App, Focus};
use crate::text::truncate_width;
use crate::ui::mouse::MouseTarget;
use crate::ui::theme::Palette;

pub fn render(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    let [list_area, settings_area] =
        Layout::vertical([Constraint::Percentage(90), Constraint::Min(3)]).areas(area);
    render_session_list(app, list_area, buf, palette);
    render_settings_entry(app, settings_area, buf, palette);
}

fn render_session_list(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    let texts = app.settings.language.texts();
    let focused = app.focus == Focus::Sidebar
        && app.modal.is_none()
        && app.picker.is_none()
        && app.conversation_overlay.is_none()
        && app.error_detail.is_none();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(texts.sidebar_history)
        .title_bottom(Line::from(texts.sidebar_hint).right_aligned())
        .border_style(palette.border(focused))
        .style(palette.surface());
    let inner = block.inner(area);
    block.render(area, buf);

    let width = inner.width as usize;
    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(index, session)| {
            let marker = if index == app.current { "● " } else { "  " };
            let style = if index == app.current {
                Style::default()
                    .fg(palette.primary)
                    .bg(palette.surface)
                    .add_modifier(Modifier::BOLD)
            } else {
                palette.surface()
            };
            let title = truncate_width(
                &session.display_title(texts.new_session_title),
                width.saturating_sub(4),
            );
            ListItem::from(format!("{marker}{title}")).style(style)
        })
        .collect();

    // 选择项必须可见：窗口尽量靠上，放不下时下移
    let visible = inner.height as usize;
    let selected = app.sidebar_pos;
    let max_offset = app
        .sessions
        .len()
        .saturating_sub(visible.min(app.sessions.len()));
    let offset = selected
        .checked_sub(visible)
        .map(|value| value + 1)
        .unwrap_or(0)
        .min(max_offset);

    let mut state = ListState::default()
        .with_selected((!app.sessions.is_empty()).then_some(selected))
        .with_offset(offset);
    let list = List::new(items)
        .highlight_style(palette.selected())
        .highlight_symbol("❯ ");
    StatefulWidget::render(list, inner, buf, &mut state);
    let mut mouse = app.mouse.borrow_mut();
    mouse.register(area, MouseTarget::Sessions);
    for (row, session) in app
        .sessions
        .iter()
        .skip(state.offset())
        .take(visible)
        .enumerate()
    {
        mouse.register(
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            MouseTarget::Session(session.id.clone()),
        );
    }
}

/// Settings 入口：不参与 ↑/↓ 选择，侧栏焦点下按 `s` 打开。
fn render_settings_entry(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    app.mouse.borrow_mut().register(area, MouseTarget::Settings);
    let texts = app.settings.language.texts();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(texts.sidebar_settings)
        .border_style(palette.border(false))
        .style(palette.panel());
    let inner = block.inner(area);
    block.render(area, buf);
    Paragraph::new(texts.sidebar_settings_hint)
        .fg(palette.muted)
        .bg(palette.panel)
        .render(inner, buf);
}
