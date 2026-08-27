//! 模型快速切换弹窗：列出当前供应商的模型，Tab 循环切换供应商。

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

pub fn render(frame: &mut Frame, app: &App) {
    let Some(picker) = &app.picker else { return };
    let texts = app.settings.language.texts();
    let palette = theme::palette(app.settings.theme);
    let provider_idx = picker.provider_idx.min(app.settings.providers.len() - 1);
    let provider = &app.settings.providers[provider_idx];
    let (title, hint) = match picker.mode {
        PickerMode::SessionDefault => (texts.picker_session_title, texts.picker_session_hint),
        PickerMode::NextTurn => (texts.picker_turn_title, texts.picker_turn_hint),
    };

    dim_background(frame, palette);
    let area = popup_rect(frame.area(), 64, 20);
    Clear.render(area, frame.buffer_mut());

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!("{} · {}", title, provider.name))
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

    if provider.models.is_empty() {
        Paragraph::new(Span::styled(
            texts.picker_empty,
            Style::default().fg(palette.muted).bg(palette.surface),
        ))
        .render(list_area, frame.buffer_mut());
    } else {
        let width = list_area.width as usize;
        let items: Vec<ListItem> = provider
            .models
            .iter()
            .map(|model| {
                let active =
                    provider.id == picker.active.provider_id && *model == picker.active.model;
                let marker = if active { "● " } else { "  " };
                let style = if active {
                    Style::default()
                        .fg(palette.primary)
                        .bg(palette.surface)
                        .add_modifier(Modifier::BOLD)
                } else {
                    palette.surface()
                };
                ListItem::from(format!(
                    "{marker}{}",
                    truncate_width(model, width.saturating_sub(4))
                ))
                .style(style)
            })
            .collect();

        // 与侧栏一致：窗口尽量靠上，选择项必须可见
        let visible = list_area.height as usize;
        let len = provider.models.len();
        let max_offset = len.saturating_sub(visible.min(len));
        let offset = picker
            .selected
            .checked_sub(visible)
            .map(|value| value + 1)
            .unwrap_or(0)
            .min(max_offset);
        let mut state = ListState::default()
            .with_selected(Some(picker.selected))
            .with_offset(offset);
        let list = List::new(items)
            .highlight_style(palette.selected())
            .highlight_symbol("❯ ");
        StatefulWidget::render(list, list_area, frame.buffer_mut(), &mut state);
    }

    if picker.editing {
        let input_area = chunks[1];
        let prefix_len = texts.picker_new_label.chars().count() + 2; // "标签: "
        let avail = (input_area.width as usize).saturating_sub(prefix_len + 1);
        let (visible_text, cursor_col) =
            visible_slice_with_cursor(&picker.buffer, picker.cursor, avail);
        let line = Line::from(vec![
            Span::styled(
                format!("{}: ", texts.picker_new_label),
                Style::default().fg(palette.primary).bg(palette.surface),
            ),
            Span::styled(visible_text, palette.surface()),
        ]);
        Paragraph::new(line)
            .style(palette.surface())
            .render(input_area, frame.buffer_mut());
        if input_area.width > 0 && input_area.height > 0 {
            frame.set_cursor_position(Position::new(
                input_area.x + ((prefix_len + cursor_col) as u16).min(input_area.width - 1),
                input_area.y,
            ));
        }
    }
}
