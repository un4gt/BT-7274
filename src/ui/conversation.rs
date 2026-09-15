//! 会话重命名、搜索、崩溃恢复与 compaction 预览弹窗。

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Wrap,
    },
};

use crate::{
    app::{App, ConversationOverlay},
    i18n::Lang,
    text::{truncate_width, visible_slice_with_cursor, wrap_lines},
};

use super::{dim_background, popup_rect, theme};
use unicode_width::UnicodeWidthStr;

pub fn render(frame: &mut Frame, app: &App) {
    let Some(overlay) = &app.conversation_overlay else {
        return;
    };
    let palette = theme::palette(app.settings.theme);
    dim_background(frame, palette);
    match overlay {
        ConversationOverlay::Rename {
            edit,
            error,
            session_id: _,
        } => render_rename(frame, app, edit, error.as_deref()),
        ConversationOverlay::Search { edit, selected } => {
            render_search(frame, app, edit, *selected)
        }
        ConversationOverlay::Recovery(state) => render_recovery(frame, app, state),
        ConversationOverlay::Compact { plan, scroll, .. } => {
            render_compact(frame, app, plan, *scroll)
        }
    }
}

fn render_rename(frame: &mut Frame, app: &App, edit: &crate::app::LineEdit, error: Option<&str>) {
    let palette = theme::palette(app.settings.theme);
    let (title, hint, label) = match app.settings.language {
        Lang::Zh => ("重命名会话", "Enter 保存 · Esc 取消", "标题"),
        Lang::En => ("Rename Session", "Enter Save · Esc Cancel", "Title"),
    };
    let area = popup_rect(frame.area(), 72, 8);
    if area.width < 4 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let prefix = format!("{label}: ");
    let prefix_width = prefix.width();
    let available = (inner.width as usize).saturating_sub(prefix_width + 1);
    let (visible, cursor) = visible_slice_with_cursor(&edit.buffer, edit.cursor, available);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix, palette.surface().fg(palette.primary)),
            Span::styled(visible, palette.surface()),
        ])),
        Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
    );
    frame.set_cursor_position(Position::new(
        inner.x + ((prefix_width + cursor) as u16).min(inner.width.saturating_sub(1)),
        inner.y.saturating_add(1),
    ));
    if let Some(error) = error
        && inner.height > 3
    {
        frame.render_widget(
            Paragraph::new(Span::styled(format!("! {error}"), palette.danger)),
            Rect::new(inner.x, inner.y.saturating_add(3), inner.width, 1),
        );
    }
}

fn render_search(frame: &mut Frame, app: &App, edit: &crate::app::LineEdit, selected: usize) {
    let palette = theme::palette(app.settings.theme);
    let texts = app.settings.language.texts();
    let (title, hint, label, empty) = match app.settings.language {
        Lang::Zh => (
            "搜索会话",
            "输入筛选 · ↑↓ 选择 · Enter 打开 · Esc 关闭",
            "搜索",
            "没有匹配的会话",
        ),
        Lang::En => (
            "Search Sessions",
            "Type to filter · ↑↓ Select · Enter Open · Esc Close",
            "Search",
            "No matching sessions",
        ),
    };
    let area = popup_rect(frame.area(), 88, 20);
    if area.width < 4 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [query_area, results_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
    if query_area.width > 0 && query_area.height > 0 {
        let prefix = format!("{label}: ");
        let prefix_width = prefix.width();
        let available = (query_area.width as usize).saturating_sub(prefix_width + 1);
        let (visible, cursor) = visible_slice_with_cursor(&edit.buffer, edit.cursor, available);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prefix, palette.surface().fg(palette.primary)),
                Span::styled(visible, palette.surface()),
            ])),
            query_area,
        );
        frame.set_cursor_position(Position::new(
            query_area.x + ((prefix_width + cursor) as u16).min(query_area.width.saturating_sub(1)),
            query_area.y,
        ));
    }

    let results = app.search_results(&edit.text());
    if results.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(empty, palette.muted)),
            results_area,
        );
        return;
    }
    let width = results_area.width as usize;
    let items: Vec<ListItem> = results
        .iter()
        .map(|index| {
            let session = &app.sessions[*index];
            let model = session
                .messages
                .iter()
                .rev()
                .find_map(|message| message.model.as_ref())
                .map(|selection| format!("{} / {}", selection.provider_name, selection.model))
                .unwrap_or_else(|| "—".to_owned());
            let title = session.display_title(texts.new_session_title);
            let message_count = match app.settings.language {
                Lang::Zh => format!("{} 条消息", session.messages.len()),
                Lang::En => format!("{} messages", session.messages.len()),
            };
            ListItem::new(vec![
                Line::from(Span::styled(
                    truncate_width(&title, width.saturating_sub(4)),
                    Style::default()
                        .fg(palette.text)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    truncate_width(
                        &format!("{message_count} · {model}"),
                        width.saturating_sub(4),
                    ),
                    palette.muted,
                )),
            ])
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(selected.min(results.len() - 1)));
    StatefulWidget::render(
        List::new(items)
            .highlight_style(palette.selected())
            .highlight_symbol("❯ "),
        results_area,
        frame.buffer_mut(),
        &mut state,
    );
}

fn render_recovery(frame: &mut Frame, app: &App, state: &crate::app::RecoveryState) {
    let palette = theme::palette(app.settings.theme);
    let texts = app.settings.language.texts();
    let (title, hint, explanation, partial_label, choices) = match app.settings.language {
        Lang::Zh => (
            "发现未完成的生成",
            "←/→ 选择 · Enter 确认 · Esc 安全保留",
            "应用上次退出时该回复仍在生成。请选择如何处理已落盘的部分内容。",
            "部分回复",
            ["继续生成", "保留并标记取消", "丢弃部分回复"],
        ),
        Lang::En => (
            "Unfinished Generation Found",
            "←/→ Select · Enter Confirm · Esc Keep Safely",
            "This response was still streaming when the app exited. Choose how to handle the saved partial content.",
            "Partial response",
            ["Continue", "Keep as cancelled", "Discard partial"],
        ),
    };
    let area = popup_rect(frame.area(), 92, 19);
    if area.width < 4 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(session) = app
        .sessions
        .iter()
        .find(|session| session.id == state.session_id)
    else {
        return;
    };
    let partial = session
        .messages
        .get(state.message_index)
        .map(|message| message.content())
        .unwrap_or_default();
    let mut lines = vec![
        Line::from(Span::styled(explanation, palette.surface())),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{}: ", texts.header_chat),
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                session.display_title(texts.new_session_title),
                palette.surface(),
            ),
        ]),
        Line::from(Span::styled(
            format!("{partial_label}:"),
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let partial = if partial.trim().is_empty() {
        app.settings
            .language
            .empty_assistant_placeholder(crate::session::MessageStatus::Streaming)
    } else {
        &partial
    };
    lines.extend(
        wrap_lines(partial, inner.width as usize)
            .into_iter()
            .take(5)
            .map(|line| Line::from(Span::styled(line, palette.muted))),
    );
    lines.push(Line::from(""));
    for (index, choice) in choices.iter().enumerate() {
        let selected = index == state.selected;
        let style = if selected {
            Style::default()
                .fg(palette.accent)
                .bg(palette.panel)
                .add_modifier(Modifier::BOLD)
        } else {
            palette.surface()
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "❯ " } else { "  " }, style),
            Span::styled(*choice, style),
        ]));
    }
    if let Some(error) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("! {error}"),
            palette.danger,
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(palette.surface())
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_compact(
    frame: &mut Frame,
    app: &App,
    plan: &crate::runtime::context::CompactionPlan,
    scroll: u16,
) {
    let palette = theme::palette(app.settings.theme);
    let (title, hint, stats, preview, detail) = match app.settings.language {
        Lang::Zh => (
            "压缩预览",
            "Enter 确认 · Esc 取消 · ↑↓ 滚动",
            "预计变化",
            "将保留的摘要",
            format!(
                "~{} → ~{} 输入 token · 归档 {} 条消息",
                plan.before_tokens, plan.after_tokens, plan.archive_count
            ),
        ),
        Lang::En => (
            "Compaction Preview",
            "Enter Confirm · Esc Cancel · ↑↓ Scroll",
            "Estimated change",
            "Summary to retain",
            format!(
                "~{} → ~{} input tokens · {} messages archived",
                plan.before_tokens, plan.after_tokens, plan.archive_count
            ),
        ),
    };
    let area = popup_rect(frame.area(), 94, 24);
    if area.width < 4 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{stats}: "),
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(detail, palette.surface()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("{preview}:"),
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    lines.extend(
        plan.summary
            .context_text()
            .lines()
            .map(|line| Line::from(Span::styled(line.to_owned(), palette.muted))),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .style(palette.surface())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}
