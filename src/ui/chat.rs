//! Content：上部分聊天消息区（底部对齐、可滚动），下部分输入框（带光标）。

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Widget},
};

use crate::app::{App, Focus, NoticeLevel};
use crate::i18n::Lang;
use crate::runtime::conversation::{MessagePart, MessageStatus, Role};
use crate::text::{truncate_width, wrap_lines};

use super::{art, markdown, theme::Palette};

pub fn render(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let input_height = match area.height {
        14.. => 7,
        8..=13 => 5,
        _ => 3,
    }
    .min(area.height);
    let [messages_area, input_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(input_height)]).areas(area);
    render_messages(app, messages_area, frame.buffer_mut(), palette);
    render_input(frame, app, input_area, palette);
}

fn render_messages(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(palette.border(false))
        .style(palette.surface());
    let inner = block.inner(area);
    block.render(area, buf);

    // BT-7274 像素画背景：拉伸铺满聊天区，消息文本覆盖在其上
    if app.settings.show_titan {
        art::render_fill(inner, buf, palette);
    }

    let width = inner.width as usize;
    let visible = inner.height as usize;
    let window = build_message_window(app, width, visible, app.scroll as usize, palette);
    Paragraph::new(window.lines)
        .style(palette.surface())
        .render(inner, buf);
}

struct MessageWindow {
    lines: Vec<Line<'static>>,
    #[cfg_attr(not(test), allow(dead_code))]
    rendered_messages: usize,
}

fn build_message_window(
    app: &App,
    width: usize,
    visible: usize,
    requested_scroll: usize,
    palette: Palette,
) -> MessageWindow {
    if visible == 0 || width == 0 {
        return MessageWindow {
            lines: Vec::new(),
            rendered_messages: 0,
        };
    }
    let session = app.open_session();
    let suffix = notice_lines(app, width, palette);
    let target = visible.saturating_add(requested_scroll).max(1);
    let mut collected = suffix.len();
    let mut chunks = Vec::new();
    for message_index in (0..session.messages.len()).rev() {
        let chunk = message_lines(app, message_index, width, palette);
        collected = collected.saturating_add(chunk.len());
        chunks.push(chunk);
        if collected >= target {
            break;
        }
    }
    let rendered_messages = chunks.len();
    let exhausted = rendered_messages == session.messages.len();
    let mut lines = if exhausted {
        summary_lines(app, width, palette)
    } else {
        Vec::new()
    };
    for chunk in chunks.into_iter().rev() {
        lines.extend(chunk);
    }
    lines.extend(suffix);

    let scroll = requested_scroll.min(lines.len().saturating_sub(visible.min(lines.len())));
    let end = lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let mut viewport = lines
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect::<Vec<_>>();
    if viewport.len() < visible {
        let mut padded = vec![Line::default(); visible - viewport.len()];
        padded.append(&mut viewport);
        viewport = padded;
    }
    MessageWindow {
        lines: viewport,
        rendered_messages,
    }
}

fn summary_lines(app: &App, width: usize, palette: Palette) -> Vec<Line<'static>> {
    let Some(summary) = &app.open_session().summary else {
        return Vec::new();
    };
    let label = match app.settings.language {
        Lang::Zh => "◇ 已压缩上下文（原文可撤销）",
        Lang::En => "◇ Compacted context (original is reversible)",
    };
    let mut lines = vec![Line::from(Span::styled(
        label,
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        wrap_lines(&summary.context_text(), width)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, palette.muted))),
    );
    lines.push(Line::default());
    lines
}

fn message_lines(
    app: &App,
    message_index: usize,
    width: usize,
    palette: Palette,
) -> Vec<Line<'static>> {
    let texts = app.settings.language.texts();
    let session = app.open_session();
    let message = &session.messages[message_index];
    let (name, color) = match message.role {
        Role::User => (texts.role_user, palette.user),
        Role::Assistant => ("BT-7274", palette.assistant),
    };
    let mut role_spans = vec![Span::styled(
        format!("❯ {name}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];
    if message.role == Role::Assistant
        && let Some(status) = app.settings.language.message_status_label(message.status)
    {
        let status_color = match message.status {
            MessageStatus::Streaming => palette.warning,
            MessageStatus::Cancelled => palette.muted,
            MessageStatus::Failed => palette.danger,
            MessageStatus::Completed => palette.success,
        };
        role_spans.push(Span::styled(
            format!(" · {status}"),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![Line::from(role_spans)];
    let content = if message.role == Role::Assistant && message.content.is_empty() {
        app.settings
            .language
            .empty_assistant_placeholder(message.status)
    } else {
        &message.content
    };
    lines.extend(markdown::render_markdown(
        content,
        width,
        palette,
        app.settings.theme,
    ));

    for part in &message.parts {
        match part {
            MessagePart::Reasoning { content } => {
                lines.push(structured_label(
                    app.settings.language,
                    "reasoning",
                    palette.muted,
                ));
                lines.extend(
                    wrap_lines(content, width)
                        .into_iter()
                        .map(|line| Line::from(Span::styled(line, palette.muted))),
                );
            }
            MessagePart::System { message } => {
                lines.push(structured_label(
                    app.settings.language,
                    "system",
                    palette.muted,
                ));
                lines.extend(
                    wrap_lines(system_event_text(app.settings.language, message), width)
                        .into_iter()
                        .map(|line| Line::from(Span::styled(line, palette.muted))),
                );
            }
            MessagePart::ToolCall {
                server,
                name,
                arguments,
                ..
            } => {
                let label = match app.settings.language {
                    Lang::Zh => format!("◇ 工具调用 · {server}/{name}"),
                    Lang::En => format!("◇ Tool call · {server}/{name}"),
                };
                lines.push(Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(palette.primary)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    wrap_lines(&json_preview(arguments), width)
                        .into_iter()
                        .map(|line| Line::from(Span::styled(line, palette.muted))),
                );
            }
            MessagePart::ToolResult {
                server,
                name,
                output,
                is_error,
                ..
            } => {
                let label = match (app.settings.language, is_error) {
                    (Lang::Zh, true) => format!("! 工具错误 · {server}/{name}"),
                    (Lang::Zh, false) => format!("◆ 工具结果 · {server}/{name}"),
                    (Lang::En, true) => format!("! Tool error · {server}/{name}"),
                    (Lang::En, false) => format!("◆ Tool result · {server}/{name}"),
                };
                let color = if *is_error {
                    palette.danger
                } else {
                    palette.success
                };
                lines.push(Line::from(Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    wrap_lines(&json_preview(output), width)
                        .into_iter()
                        .map(|line| Line::from(Span::styled(line, palette.muted))),
                );
            }
        }
    }
    lines.push(Line::default());
    lines
        .into_iter()
        .flat_map(|line| markdown::wrap_styled_line(&line, width))
        .collect()
}

fn json_preview(value: &serde_json::Value) -> String {
    let raw = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let mut preview = raw.chars().take(4_000).collect::<String>();
    if raw.chars().count() > 4_000 {
        preview.push('…');
    }
    preview
}

fn notice_lines(app: &App, width: usize, palette: Palette) -> Vec<Line<'static>> {
    let Some(notice) = app.notice() else {
        return Vec::new();
    };
    let (prefix, color) = match (app.settings.language, notice.level) {
        (Lang::Zh, NoticeLevel::Info) => ("[信息]", palette.primary),
        (Lang::Zh, NoticeLevel::Warning) => ("[警告]", palette.warning),
        (Lang::Zh, NoticeLevel::Error) => ("[错误]", palette.danger),
        (Lang::En, NoticeLevel::Info) => ("[Info]", palette.primary),
        (Lang::En, NoticeLevel::Warning) => ("[Warning]", palette.warning),
        (Lang::En, NoticeLevel::Error) => ("[Error]", palette.danger),
    };
    wrap_lines(&format!("{prefix} {}", notice.summary), width)
        .into_iter()
        .map(|line| {
            Line::from(Span::styled(
                line,
                Style::default().fg(color).bg(palette.surface),
            ))
        })
        .collect()
}

fn system_event_text(lang: Lang, message: &str) -> &str {
    match (lang, message) {
        (Lang::Zh, "response.queued") => "Provider 已接收请求，正在排队。",
        (Lang::En, "response.queued") => "The Provider accepted the request and queued it.",
        (Lang::Zh, "recovery.kept_after_restart") => "应用重启后保留了该部分回复。",
        (Lang::En, "recovery.kept_after_restart") => "The partial response was kept after restart.",
        (Lang::Zh, "recovery.continued_after_restart") => "应用重启后已从该部分回复继续。",
        (Lang::En, "recovery.continued_after_restart") => {
            "Generation continued from this partial response after restart."
        }
        _ => message,
    }
}

fn structured_label(lang: Lang, kind: &str, color: ratatui::style::Color) -> Line<'static> {
    Line::from(structured_label_span(lang, kind, color))
}

fn structured_label_span(lang: Lang, kind: &str, color: ratatui::style::Color) -> Span<'static> {
    let label = match (lang, kind) {
        (Lang::Zh, "reasoning") => "◇ 推理",
        (Lang::Zh, _) => "ℹ 系统事件",
        (Lang::En, "reasoning") => "◇ Reasoning",
        (Lang::En, _) => "ℹ System event",
    };
    Span::styled(
        label.to_owned(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn render_input(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let texts = app.settings.language.texts();
    let focused = input_is_focused(app);
    let hint = match app.settings.language {
        Lang::Zh => format!(
            "{} 发送 · {} 换行 · Tab 侧栏",
            app.settings.keybindings.submit, app.settings.keybindings.newline
        ),
        Lang::En => format!(
            "{} Send · {} Newline · Tab Sidebar",
            app.settings.keybindings.submit, app.settings.keybindings.newline
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(texts.chat_input)
        .title_bottom(
            ratatui::text::Line::from(truncate_width(&hint, area.width.saturating_sub(4) as usize))
                .right_aligned(),
        )
        .border_style(palette.border(focused))
        .style(palette.panel());
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let viewport = app
        .editor
        .viewport(inner.width as usize, inner.height as usize);
    let lines = viewport
        .rows
        .into_iter()
        .map(|row| {
            Line::from(
                row.cells
                    .into_iter()
                    .map(|cell| {
                        Span::styled(
                            cell.text,
                            if cell.selected {
                                palette.selected()
                            } else {
                                palette.panel()
                            },
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines)
        .style(palette.panel())
        .render(inner, frame.buffer_mut());
    if focused && inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position(Position::new(
            inner.x + (viewport.cursor_column as u16).min(inner.width - 1),
            inner.y + (viewport.cursor_row as u16).min(inner.height - 1),
        ));
    }
}

pub(super) fn input_is_focused(app: &App) -> bool {
    app.focus == Focus::Input
        && app.modal.is_none()
        && app.picker.is_none()
        && app.conversation_overlay.is_none()
        && app.code_overlay.is_none()
        && app.error_detail.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Settings,
        runtime::conversation::{Message, MessageStatus, Session},
        ui::theme,
    };
    #[tokio::test]
    async fn long_history_only_formats_messages_near_the_viewport() {
        let mut session = Session::new();
        session.messages = (0..10_000)
            .map(|index| Message::user(format!("message {index}"), None))
            .collect();
        let app = App::new(Settings::default(), vec![session]);
        let palette = theme::palette(app.settings.theme);

        let window = build_message_window(&app, 48, 20, 0, palette);

        assert_eq!(window.lines.len(), 20);
        assert!(window.rendered_messages < 32);
        assert!(
            window
                .lines
                .iter()
                .any(|line| line.to_string().contains("message 9999"))
        );
    }

    #[tokio::test]
    async fn unicode_markdown_stays_inside_narrow_and_resized_viewports() {
        let settings = Settings::default();
        let mut session = Session::new();
        let mut message = Message::assistant_streaming(settings.default_selection());
        message.status = MessageStatus::Completed;
        message.content =
            "# 中文标题\n\nemoji 👩‍💻 · e\u{301} · 全角，。\n\nsupercalifragilisticexpialidocious"
                .to_owned();
        session.messages.push(message);
        let app = App::new(settings, vec![session]);
        let palette = theme::palette(app.settings.theme);

        for width in [4, 7, 16, 40] {
            let window = build_message_window(&app, width, 24, 0, palette);
            assert!(window.lines.iter().all(|line| line.width() <= width));
        }
    }

    #[tokio::test]
    async fn tool_calls_and_results_render_as_bounded_structured_parts() {
        let settings = Settings::default();
        let mut session = Session::new();
        let mut message = Message::assistant_streaming(settings.default_selection());
        message.status = MessageStatus::Completed;
        message.push_tool_call(
            "call-1".to_owned(),
            "docs".to_owned(),
            "search".to_owned(),
            serde_json::json!({"query":"洛杉矶天气"}),
        );
        message.push_tool_result(
            "call-1".to_owned(),
            "docs".to_owned(),
            "search".to_owned(),
            serde_json::json!({"temperature":24}),
            false,
        );
        session.messages.push(message);
        let app = App::new(settings, vec![session]);
        let palette = theme::palette(app.settings.theme);

        let window = build_message_window(&app, 24, 30, 0, palette);
        assert!(window.lines.iter().all(|line| line.width() <= 24));
        let rendered = window
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("工具调用"));
        assert!(rendered.contains("工具结果"));
    }
}
