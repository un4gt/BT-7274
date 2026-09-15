//! 模型快速切换弹窗：按供应商分组列出全部模型，Tab 可快速跳转供应商。

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
    },
};

use super::{dim_background, popup_rect, theme};
use crate::app::{App, PickerMode};
use crate::text::{truncate_width, visible_slice_with_cursor};
use unicode_width::UnicodeWidthStr;

pub fn render(frame: &mut Frame, app: &App) {
    let Some(picker) = &app.picker else { return };
    let texts = app.settings.language.texts();
    let palette = theme::palette(app.settings.theme);
    let provider_idx = picker.provider_idx.min(app.settings.providers.len() - 1);
    let (title, hint) = match picker.mode {
        PickerMode::SessionDefault => (texts.picker_session_title, texts.picker_session_hint),
        PickerMode::NextTurn => (texts.picker_turn_title, texts.picker_turn_hint),
    };

    dim_background(frame, palette);
    let area = popup_rect(frame.area(), 64, 20);
    Clear.render(area, frame.buffer_mut());

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    // 列表区 + 新增输入行（仅编辑时显示）
    let constraints = if picker.editing {
        vec![Constraint::Fill(1), Constraint::Length(1)]
    } else {
        vec![Constraint::Fill(1)]
    };
    let chunks = Layout::vertical(constraints).split(inner);
    let list_area = chunks[0];

    let width = list_area.width as usize;
    let row_width = width.saturating_sub(4);
    let mut items = Vec::new();
    let mut selected_row = 0;
    for (index, provider) in app.settings.providers.iter().enumerate() {
        let selected_provider = index == provider_idx;
        let provider_row = items.len();
        if selected_provider && provider.models.is_empty() {
            selected_row = provider_row;
        }
        let marker = if selected_provider { "▸ " } else { "  " };
        let label = format!(
            "{} [{}] ({})",
            provider.name,
            provider.api_kind.short_label(),
            provider.models.len()
        );
        let header_style = if selected_provider {
            Style::default()
                .fg(palette.primary)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(palette.muted)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD)
        };
        items.push(
            ListItem::from(format!("{marker}{}", truncate_width(&label, row_width)))
                .style(header_style),
        );

        for (model_idx, model) in provider.models.iter().enumerate() {
            if selected_provider && model_idx == picker.selected {
                selected_row = items.len();
            }
            let active = provider.id == picker.active.provider_id && *model == picker.active.model;
            let marker = if active { "● " } else { "  " };
            let style = if active {
                Style::default()
                    .fg(palette.primary)
                    .bg(palette.surface)
                    .add_modifier(Modifier::BOLD)
            } else {
                palette.surface()
            };
            items.push(
                ListItem::from(format!(
                    "  {marker}{}",
                    truncate_width(model, row_width.saturating_sub(2))
                ))
                .style(style),
            );
        }
    }

    // 与侧栏一致：窗口尽量靠上，选择项必须可见。
    let visible = list_area.height as usize;
    let len = items.len();
    let max_offset = len.saturating_sub(visible.min(len));
    let offset = selected_row
        .checked_sub(visible)
        .map(|value| value + 1)
        .unwrap_or(0)
        .min(max_offset);
    let mut state = ListState::default()
        .with_selected(Some(selected_row))
        .with_offset(offset);
    let list = List::new(items)
        .highlight_style(palette.selected())
        .highlight_symbol("❯ ");
    StatefulWidget::render(list, list_area, frame.buffer_mut(), &mut state);

    if picker.editing {
        let input_area = chunks[1];
        let prefix = format!("{}: ", texts.picker_new_label);
        let prefix_width = prefix.width();
        let avail = (input_area.width as usize).saturating_sub(prefix_width + 1);
        let (visible_text, cursor_col) =
            visible_slice_with_cursor(&picker.buffer, picker.cursor, avail);
        let line = Line::from(vec![
            Span::styled(
                prefix,
                Style::default().fg(palette.primary).bg(palette.surface),
            ),
            Span::styled(visible_text, palette.surface()),
        ]);
        Paragraph::new(line)
            .style(palette.surface())
            .render(input_area, frame.buffer_mut());
        if input_area.width > 0 && input_area.height > 0 {
            frame.set_cursor_position(Position::new(
                input_area.x + ((prefix_width + cursor_col) as u16).min(input_area.width - 1),
                input_area.y,
            ));
        }
    }
}
