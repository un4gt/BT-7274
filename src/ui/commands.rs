//! Visually distinct local command menus, feedback, and live MCP inspection.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::{
    chat::input_is_focused,
    dim_background,
    mouse::MouseTarget,
    popup_rect,
    theme::{self, Palette},
};
use crate::{
    app::{
        App,
        commands::{CommandOverlay, effort_label},
    },
    i18n::Lang,
    runtime::{
        mcp::{McpServerSnapshot, McpServerStatus},
        tool::ToolDefinition,
    },
    text::{truncate_width, wrap_lines},
};

pub(super) fn local_label(language: Lang) -> &'static str {
    match language {
        Lang::Zh => "本地命令",
        Lang::En => "Local command",
    }
}

/// Reserve feedback space outside the transcript, so no assistant role or Markdown is implied.
pub(super) fn render_reply(frame: &mut Frame, app: &App, area: Rect, palette: Palette) -> Rect {
    let Some(reply) = app.command_reply() else {
        return area;
    };
    if area.height < 5 || area.width < 4 {
        return area;
    }
    let height = (wrap_lines(&reply.body, area.width.saturating_sub(2) as usize).len() as u16)
        .saturating_add(2)
        .clamp(3, 5)
        .min(area.height.saturating_sub(2));
    let [messages, feedback] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height)]).areas(area);
    let hint = match app.settings.language {
        Lang::Zh => "Esc 关闭",
        Lang::En => "Esc Dismiss",
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(
            " {} · {} ",
            local_label(app.settings.language),
            reply.command
        ))
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(Style::default().fg(palette.accent))
        .style(palette.panel());
    frame.render_widget(
        Paragraph::new(reply.body.as_str())
            .wrap(Wrap { trim: false })
            .block(block),
        feedback,
    );
    // Feedback is not part of the scrollable message history.
    app.mouse
        .borrow_mut()
        .register(feedback, MouseTarget::CommandMenu);
    messages
}

pub(super) fn render_menu(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    if !input_is_focused(app) || app.startup.is_some() {
        return;
    }
    let matches = app.command_matches();
    if matches.is_empty() || area.height < 3 || area.width < 4 {
        return;
    }
    let height = (matches.len() as u16 + 2).min(area.height);
    let area = Rect::new(area.x, area.bottom() - height, area.width.min(76), height);
    let hint = match app.settings.language {
        Lang::Zh => format!(
            "↑↓ 选择 · Tab 补全 · {} 执行 · Esc 关闭",
            app.settings.keybindings.submit
        ),
        Lang::En => format!(
            "↑↓ Select · Tab Complete · {} Run · Esc Close",
            app.settings.keybindings.submit
        ),
    };
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" / {} ", local_label(app.settings.language)))
        .title_bottom(Line::from(truncate_width(
            &hint,
            area.width.saturating_sub(4) as usize,
        )))
        .border_style(Style::default().fg(palette.accent))
        .style(palette.surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let selected = app.command_menu_selection(&matches);
    let offset = selected
        .saturating_add(1)
        .saturating_sub(inner.height as usize);
    app.mouse
        .borrow_mut()
        .register(area, MouseTarget::CommandMenu);
    for (row, (index, command)) in matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height as usize)
        .enumerate()
    {
        let text = format!(
            "{} {:<14} {}",
            if selected == index { "❯" } else { " " },
            command.name(),
            command.description(app.settings.language)
        );
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        frame.render_widget(
            Paragraph::new(truncate_width(&text, inner.width as usize)).style(
                if selected == index {
                    palette.selected()
                } else {
                    palette.surface()
                },
            ),
            rect,
        );
        app.mouse
            .borrow_mut()
            .register(rect, MouseTarget::SlashCommand(*command));
    }
}

pub(super) fn render_overlay(frame: &mut Frame, app: &App) {
    let Some(overlay) = &app.commands.overlay else {
        return;
    };
    let palette = theme::palette(app.settings.theme);
    let language = app.settings.language;
    dim_background(frame, palette);
    let (command, height, hint) = match overlay {
        CommandOverlay::Effort(_) => (
            "/effort",
            21,
            match language {
                Lang::Zh => "↑↓ 选择 · Enter 保存 · Esc 返回",
                Lang::En => "↑↓ Select · Enter Save · Esc Back",
            },
        ),
        CommandOverlay::Mcp { .. } => (
            "/mcp",
            28,
            match language {
                Lang::Zh => "↑↓/PgUp/PgDn 滚动 · Home/End · Esc 返回",
                Lang::En => "↑↓/PgUp/PgDn Scroll · Home/End · Esc Back",
            },
        ),
    };
    let area = popup_rect(frame.area(), 88, height);
    if area.width < 4 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {} · {command} ", local_label(language)))
        .title_bottom(
            Line::from(truncate_width(hint, area.width.saturating_sub(4) as usize)).right_aligned(),
        )
        .border_style(Style::default().fg(palette.accent))
        .style(palette.surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match overlay {
        CommandOverlay::Effort(picker) => {
            let scope = match language {
                Lang::Zh => "保存到此模型配置，对后续请求生效；可选值以供应商支持为准。",
                Lang::En => {
                    "Saved per model for future requests; values depend on provider support."
                }
            };
            let mut info = vec![
                Line::from(Span::styled(
                    truncate_width(
                        &format!("{} · {}", picker.model.provider_name, picker.model.model),
                        inner.width as usize,
                    ),
                    palette.primary,
                )),
                Line::from(scope),
            ];
            if picker.unsupported {
                info.push(Line::from(Span::styled(
                    match language {
                        Lang::Zh => "此模型不支持思考参数，仅可恢复默认。",
                        Lang::En => {
                            "This model does not support reasoning; only Default is available."
                        }
                    },
                    palette.warning,
                )));
            }
            let [header, list, error] = Layout::vertical([
                Constraint::Length(if picker.unsupported { 4 } else { 3 }),
                Constraint::Fill(1),
                Constraint::Length(if picker.error.is_some() { 3 } else { 0 }),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new(info).wrap(Wrap { trim: false }), header);
            let items = picker
                .options
                .iter()
                .map(|option| {
                    let marker = if *option == picker.active { "●" } else { " " };
                    ListItem::new(format!(
                        "{marker} {}",
                        effort_label(option.as_ref(), language)
                    ))
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(picker.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("❯ ")
                    .highlight_style(palette.selected())
                    .style(palette.surface()),
                list,
                &mut state,
            );
            if let Some(message) = &picker.error {
                frame.render_widget(
                    Paragraph::new(message.as_str())
                        .style(Style::default().fg(palette.danger))
                        .wrap(Wrap { trim: false }),
                    error,
                );
            }
        }
        CommandOverlay::Mcp { scroll } => {
            let catalog = app.mcp_command_catalog();
            let lines = mcp_lines(&catalog, language, palette, inner.width as usize);
            let offset = scroll
                .get()
                .min(lines.len().saturating_sub(inner.height as usize));
            scroll.set(offset);
            frame.render_widget(
                Paragraph::new(
                    lines
                        .into_iter()
                        .skip(offset)
                        .take(inner.height as usize)
                        .collect::<Vec<_>>(),
                ),
                inner,
            );
        }
    }
}

fn display_text(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn mcp_lines(
    catalog: &[(McpServerSnapshot, Vec<ToolDefinition>)],
    language: Lang,
    palette: Palette,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut push = |text: String, style: Style| {
        lines.extend(
            wrap_lines(&text, width)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, style))),
        );
    };
    if catalog.is_empty() {
        push(
            match language {
                Lang::Zh => "尚未配置 MCP 服务。可在设置 → MCP 中添加。",
                Lang::En => "No MCP servers configured. Add one in Settings → MCP.",
            }
            .to_owned(),
            palette.surface(),
        );
        return lines;
    }
    let connected = catalog
        .iter()
        .filter(|(server, _)| server.status == McpServerStatus::Connected)
        .count();
    let tool_count: usize = catalog.iter().map(|(_, tools)| tools.len()).sum();
    push(
        match language {
            Lang::Zh => format!(
                "{} 个服务 · {connected} 个已连接 · {tool_count} 个可用工具（实时）",
                catalog.len()
            ),
            Lang::En => format!(
                "{} servers · {connected} connected · {tool_count} available tools (live)",
                catalog.len()
            ),
        },
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    for (server, tools) in catalog {
        push(String::new(), palette.surface());
        let (label, color) = match (language, server.status) {
            (Lang::Zh, McpServerStatus::Connected) => ("已连接", palette.success),
            (Lang::Zh, McpServerStatus::Starting) => ("连接中（未连接）", palette.warning),
            (Lang::Zh, McpServerStatus::Failed) => ("连接失败（未连接）", palette.danger),
            (Lang::Zh, McpServerStatus::Disabled) => ("已禁用（未连接）", palette.muted),
            (Lang::Zh, McpServerStatus::Stopping) => ("断开中（不可用）", palette.warning),
            (Lang::En, McpServerStatus::Connected) => ("Connected", palette.success),
            (Lang::En, McpServerStatus::Starting) => {
                ("Connecting (not connected)", palette.warning)
            }
            (Lang::En, McpServerStatus::Failed) => ("Failed (not connected)", palette.danger),
            (Lang::En, McpServerStatus::Disabled) => ("Disabled (not connected)", palette.muted),
            (Lang::En, McpServerStatus::Stopping) => {
                ("Disconnecting (unavailable)", palette.warning)
            }
        };
        push(
            format!(
                "{} {} · {label}",
                if server.status == McpServerStatus::Connected {
                    "●"
                } else {
                    "○"
                },
                display_text(&server.name)
            ),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
        if let Some(error) = &server.error {
            push(
                format!("  {}", display_text(error)),
                Style::default().fg(palette.danger),
            );
        }
        if tools.is_empty() {
            push(
                match language {
                    Lang::Zh => "  暂无可用工具",
                    Lang::En => "  No available tools",
                }
                .to_owned(),
                Style::default().fg(palette.muted),
            );
        }
        for tool in tools {
            push(
                format!("  • {}", display_text(&tool.remote_name)),
                palette.surface(),
            );
            if let Some(description) = &tool.description {
                push(
                    format!("    {}", display_text(description)),
                    Style::default().fg(palette.muted),
                );
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpCapabilityMetadata, Theme};

    #[test]
    fn mcp_report_includes_connection_errors_and_tool_descriptions_with_bounded_lines() {
        let snapshot = |name: &str, status, error| McpServerSnapshot {
            name: name.to_owned(),
            status,
            generation: 1,
            capabilities: McpCapabilityMetadata::default(),
            error,
        };
        let catalog = vec![
            (
                snapshot("docs", McpServerStatus::Connected, None),
                vec![ToolDefinition {
                    model_name: "docs_search".into(),
                    server_name: "docs".into(),
                    remote_name: "docs.search".into(),
                    description: Some("查找文档\nsearch documentation".into()),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            ),
            (
                snapshot(
                    "offline",
                    McpServerStatus::Failed,
                    Some("connection refused".into()),
                ),
                vec![],
            ),
            (
                snapshot("disabled", McpServerStatus::Disabled, None),
                vec![],
            ),
        ];
        for language in [Lang::Zh, Lang::En] {
            let lines = mcp_lines(&catalog, language, theme::palette(Theme::Carbon), 88);
            let text = lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            for value in [
                "docs.search",
                "查找文档 search documentation",
                "connection refused",
                "offline",
                "disabled",
            ] {
                assert!(text.contains(value), "missing {value}: {text}");
            }
            assert!(text.contains(match language {
                Lang::Zh => "3 个服务 · 1 个已连接 · 1 个可用工具",
                Lang::En => "3 servers · 1 connected · 1 available tools",
            }));
            for width in [4, 16, 32] {
                let lines = mcp_lines(&catalog, language, theme::palette(Theme::Carbon), width);
                assert!(lines.iter().all(|line| line.width() <= width));
            }
        }
    }
}
