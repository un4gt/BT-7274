//! 设置弹窗：左分类 + 右内容的双栏布局，外加新增供应商三步向导。
//!
//! 弹窗文案跟随**草稿**语言——语言字段 Enter 切换即整体即时预览，
//! Esc 放弃或 Ctrl+S 保存后回到已保存语言。

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap},
};

use super::{
    dim_background, popup_rect,
    theme::{self, Palette},
};
use crate::app::{
    ContextField, KeybindingField, LineEdit, McpServerWizard, ModelsSection, NetworkField,
    ProviderWizard, SettingsCategory, SettingsField, SettingsPane, SettingsUi,
};
use crate::i18n::{Lang, Texts};
use crate::runtime::mcp::McpServerStatus;
use crate::text::{truncate_width, visible_slice_with_cursor};
use unicode_width::UnicodeWidthStr;

/// spinner 动画帧。
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// 标签列宽（右对齐）。
const LABEL_WIDTH: usize = 10;
const SETTINGS_MODAL_WIDTH: u16 = 116;
const SETTINGS_MODAL_HEIGHT: u16 = 36;

fn inset_rect(area: Rect, horizontal: u16, vertical: u16) -> Rect {
    let horizontal = horizontal.min(area.width / 2);
    let vertical = vertical.min(area.height / 2);
    Rect::new(
        area.x.saturating_add(horizontal),
        area.y.saturating_add(vertical),
        area.width.saturating_sub(horizontal.saturating_mul(2)),
        area.height.saturating_sub(vertical.saturating_mul(2)),
    )
}

fn vertical_slice(area: Rect, top: u16, height: u16) -> Rect {
    let top = top.min(area.height);
    Rect::new(
        area.x,
        area.y.saturating_add(top),
        area.width,
        height.min(area.height.saturating_sub(top)),
    )
}

fn render_section_heading(area: Rect, title: &str, buf: &mut Buffer, palette: Palette) -> Rect {
    if area.width == 0 || area.height < 3 {
        return area;
    }

    let title = truncate_width(title, area.width as usize);
    let remaining = (area.width as usize).saturating_sub(title.width());
    let mut spans = vec![Span::styled(
        title,
        Style::default()
            .fg(palette.primary)
            .bg(palette.surface)
            .add_modifier(Modifier::BOLD),
    )];
    if remaining >= 3 {
        spans.push(Span::styled("  ", palette.surface()));
        spans.push(Span::styled(
            "─".repeat(remaining - 2),
            Style::default().fg(palette.border).bg(palette.surface),
        ));
    }
    Paragraph::new(Line::from(spans))
        .style(palette.surface())
        .render(vertical_slice(area, 0, 1), buf);
    vertical_slice(area, 2, area.height)
}

pub fn render(frame: &mut Frame, modal: &SettingsUi, ticks: u64) {
    let palette = theme::palette(modal.draft.theme);
    dim_background(frame, palette);

    // 向导打开时占满整个弹窗区域。
    if let Some(wizard) = &modal.mcp_wizard {
        render_mcp_wizard(frame, wizard, modal.draft.language, palette);
        return;
    }
    if let Some(wizard) = &modal.wizard {
        render_wizard(frame, wizard, modal.draft.language, ticks, palette);
        return;
    }

    let texts = modal.draft.language.texts();
    let area = popup_rect(frame.area(), SETTINGS_MODAL_WIDTH, SETTINGS_MODAL_HEIGHT);
    Clear.render(area, frame.buffer_mut());
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(Span::styled(
            texts.settings_title,
            Style::default()
                .fg(palette.primary)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        )))
        .title_bottom(
            Line::from(Span::styled(texts.settings_bottom_hint, palette.muted)).right_aligned(),
        )
        .border_style(palette.border(true))
        .style(palette.surface());
    let mut inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    if inner.width >= 72 && inner.height >= 24 {
        inner = inset_rect(inner, 1, 1);
    }

    let category_width = if inner.width >= 72 { 18 } else { 14 };
    let column_gap = if inner.width >= 72 { 2 } else { 1 };
    let [category_area, _, content_area] = Layout::horizontal([
        Constraint::Length(category_width),
        Constraint::Length(column_gap),
        Constraint::Fill(1),
    ])
    .areas(inner);

    render_categories(modal, category_area, frame.buffer_mut(), palette);
    match modal.category {
        SettingsCategory::Models => {
            render_models(modal, content_area, frame, &texts, ticks, palette)
        }
        SettingsCategory::Context => {
            let body = render_section_heading(
                content_area,
                texts.cat_context,
                frame.buffer_mut(),
                palette,
            );
            render_context(modal, body, frame.buffer_mut(), &texts, palette);
        }
        SettingsCategory::Mcp => render_mcp(modal, content_area, frame, ticks, palette),
        SettingsCategory::Network => {
            let body = render_section_heading(
                content_area,
                texts.cat_network,
                frame.buffer_mut(),
                palette,
            );
            render_network(modal, body, frame, &texts, palette);
        }
        SettingsCategory::Keyboard => {
            let body = render_section_heading(
                content_area,
                texts.cat_keyboard,
                frame.buffer_mut(),
                palette,
            );
            render_keyboard(modal, body, frame, palette);
        }
        SettingsCategory::Appearance => {
            let body = render_section_heading(
                content_area,
                texts.cat_appearance,
                frame.buffer_mut(),
                palette,
            );
            render_appearance(modal, body, frame.buffer_mut(), &texts, palette);
        }
    }
}

/// 左侧分类栏（无标题，选中项用 ❯ 标记）。
fn render_categories(modal: &SettingsUi, area: Rect, buf: &mut Buffer, palette: Palette) {
    let focused = modal.pane == SettingsPane::Category;
    let texts = modal.draft.language.texts();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(palette.border(focused))
        .style(palette.panel());
    let inner = inset_rect(block.inner(area), 1, 0);
    block.render(area, buf);

    let labels = [
        texts.cat_models,
        texts.cat_context,
        texts.cat_mcp,
        texts.cat_network,
        texts.cat_keyboard,
        texts.cat_appearance,
    ];
    let stride = if inner.height as usize >= labels.len().saturating_mul(2).saturating_sub(1) {
        2
    } else {
        1
    };
    let visible = (inner.height as usize).div_ceil(stride);
    let start = selection_offset(modal.category.index(), SettingsCategory::ALL.len(), visible);
    for (display_row, (index, label)) in labels
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let selected = index == modal.category.index();
        let style = if selected && focused {
            Style::default()
                .fg(palette.accent)
                .bg(palette.selection)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(palette.text)
                .bg(palette.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            palette.panel()
        };
        Paragraph::new(Span::styled(
            format!("{} {label}", if selected { "❯" } else { " " }),
            style,
        ))
        .style(style)
        .render(vertical_slice(inner, (display_row * stride) as u16, 1), buf);
    }
}

fn render_context(
    modal: &SettingsUi,
    area: Rect,
    buf: &mut Buffer,
    texts: &Texts,
    palette: Palette,
) {
    let focused = modal.pane == SettingsPane::Content;
    let stride = if area.height >= 13 { 3 } else { 2 };
    let values = [
        if modal.draft.context.auto_compact {
            texts.value_on.to_owned()
        } else {
            texts.value_off.to_owned()
        },
        format!("{}%", modal.draft.context.auto_compact_threshold_percent),
        format!("{} tokens", modal.draft.context.reserved_output_tokens),
    ];
    for (index, field) in ContextField::ALL.iter().enumerate() {
        let selected = index == modal.context_pos;
        let marker = if selected && focused { "❯ " } else { "  " };
        let value_color =
            if *field == ContextField::AutoCompact && !modal.draft.context.auto_compact {
                palette.muted
            } else if selected {
                palette.accent
            } else {
                palette.text
            };
        let line = Line::from(vec![
            Span::styled(
                marker,
                Style::default()
                    .fg(if selected {
                        palette.accent
                    } else {
                        palette.muted
                    })
                    .bg(palette.surface),
            ),
            Span::styled(
                format!(
                    "{:>width$}  ",
                    texts.context_field_label(*field),
                    width = LABEL_WIDTH
                ),
                Style::default()
                    .fg(palette.primary)
                    .bg(palette.surface)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("[{}]", values[index]),
                Style::default()
                    .fg(value_color)
                    .bg(palette.surface)
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]);
        Paragraph::new(line).render(vertical_slice(area, 1 + index as u16 * stride, 1), buf);
    }
    let hint_top = 1 + ContextField::ALL.len() as u16 * stride;
    Paragraph::new(Span::styled(texts.context_settings_hint, palette.muted))
        .wrap(Wrap { trim: false })
        .render(inset_rect(vertical_slice(area, hint_top, 3), 1, 0), buf);
}

/// 「模型」分类：上半供应商列表，下半该供应商的模型。
fn render_models(
    modal: &SettingsUi,
    area: Rect,
    frame: &mut Frame,
    texts: &Texts,
    ticks: u64,
    palette: Palette,
) {
    let providers = &modal.draft.providers;
    let view_pos = modal.provider_pos.min(providers.len() - 1);
    let viewed = &providers[view_pos];

    let panel_gap = u16::from(area.height >= 12);
    let available_height = area.height.saturating_sub(panel_gap);
    let providers_height = (providers.len().clamp(1, 6) as u16 + 2).min(available_height / 2);
    let [providers_area, _, models_area] = Layout::vertical([
        Constraint::Length(providers_height),
        Constraint::Length(panel_gap),
        Constraint::Fill(1),
    ])
    .areas(area);

    // —— 供应商列表 ——
    let section_focus =
        modal.pane == SettingsPane::Content && modal.section == ModelsSection::Providers;
    let spinner = SPINNER[ticks as usize % SPINNER.len()];
    let mut provider_title = format!("{} ({})", texts.providers_title, providers.len());
    if modal.sync_id.is_some() {
        provider_title = format!("{spinner} {provider_title}");
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(provider_title)
        .title_bottom(Line::from(texts.providers_hint).right_aligned())
        .border_style(palette.border(section_focus))
        .style(palette.surface());
    let provider_inner = inset_rect(block.inner(providers_area), 1, 0);
    block.render(providers_area, frame.buffer_mut());

    let width = provider_inner.width as usize;
    let provider_visible = provider_inner.height as usize;
    let provider_offset = selection_offset(view_pos, providers.len(), provider_visible);
    for (row_index, (index, provider)) in providers
        .iter()
        .enumerate()
        .skip(provider_offset)
        .take(provider_visible)
        .enumerate()
    {
        let selected = index == view_pos;
        let current = index == modal.draft.current_provider;
        let row = format!(
            "{}{}{}",
            if selected && section_focus {
                "❯ "
            } else {
                "  "
            },
            if current { "● " } else { "  " },
            truncate_width(
                &format!("{} [{}]", provider.name, provider.api_kind.short_label()),
                width.saturating_sub(5),
            ),
        );
        let style = if current {
            Style::default().fg(palette.primary).bg(palette.surface)
        } else {
            palette.surface()
        }
        .add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
        Paragraph::new(Span::styled(row, style)).render(
            vertical_slice(provider_inner, row_index as u16, 1),
            frame.buffer_mut(),
        );
    }

    // —— 模型列表 ——
    let list_focus =
        modal.pane == SettingsPane::Content && modal.section == ModelsSection::ModelsList;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(
            "{} · {} ({})",
            texts.models_list_title,
            viewed.name,
            viewed.models.len()
        ))
        .title_bottom(Line::from(texts.models_hint).right_aligned())
        .border_style(palette.border(list_focus))
        .style(palette.surface());
    let list_inner = inset_rect(block.inner(models_area), 1, 0);
    block.render(models_area, frame.buffer_mut());

    let mut next_y = list_inner.y;
    let mut available = list_inner.height;
    if let Some(err) = &modal.sync_error
        && modal.sync_error_provider == Some(view_pos)
        && available > 0
    {
        Paragraph::new(Line::from(Span::styled(
            format!("✗ {err}"),
            Style::default().fg(palette.danger).bg(palette.surface),
        )))
        .render(
            Rect::new(list_inner.x, next_y, list_inner.width, 1),
            frame.buffer_mut(),
        );
        next_y += 1;
        available = available.saturating_sub(1);
    }
    if modal.manual_active {
        available = available.saturating_sub(1);
    }
    let model_area = Rect::new(list_inner.x, next_y, list_inner.width, available);
    let viewing_current = view_pos == modal.draft.current_provider;
    if viewed.models.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            texts.picker_empty,
            Style::default().fg(palette.muted).bg(palette.surface),
        )))
        .render(model_area, frame.buffer_mut());
    } else {
        let model_visible = model_area.height as usize;
        let model_offset = selection_offset(modal.model_pos, viewed.models.len(), model_visible);
        let mut lines = Vec::with_capacity(model_visible);
        for (index, model) in viewed
            .models
            .iter()
            .enumerate()
            .skip(model_offset)
            .take(model_visible)
        {
            let selected = index == modal.model_pos;
            let current = viewing_current && *model == modal.draft.model;
            let row = format!(
                "{}{}{}",
                if selected && list_focus { "❯ " } else { "  " },
                if current { "● " } else { "  " },
                model,
            );
            let style = if current {
                Style::default().fg(palette.primary).bg(palette.surface)
            } else {
                palette.surface()
            }
            .add_modifier(if selected && list_focus {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
            lines.push(Line::from(Span::styled(row, style)));
        }
        Paragraph::new(lines).render(model_area, frame.buffer_mut());
    }

    // —— 手动添加行（覆盖在模型列表最后一行之上） ——
    if modal.manual_active && list_inner.height > 0 {
        let row = Rect {
            y: list_inner.bottom() - 1,
            x: list_inner.x,
            width: list_inner.width,
            height: 1,
        };
        render_manual_line(frame, row, &modal.manual, texts.manual_add_label, palette);
    }
}

fn selection_offset(selected: usize, len: usize, visible: usize) -> usize {
    selected
        .saturating_add(1)
        .saturating_sub(visible)
        .min(len.saturating_sub(visible.min(len)))
}

/// 「网络」分类：代理类型选择与 URL 编辑。
fn render_mcp(modal: &SettingsUi, area: Rect, frame: &mut Frame, ticks: u64, palette: Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = modal.pane == SettingsPane::Content;
    let lang = modal.draft.language;
    let [body_area, hint_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let panel_gap = u16::from(body_area.height >= 8 && body_area.width >= 24);
    let [servers_area, _, detail_area] = if body_area.width >= 64 {
        Layout::horizontal([
            Constraint::Length(32),
            Constraint::Length(panel_gap),
            Constraint::Fill(1),
        ])
        .areas(body_area)
    } else {
        Layout::vertical([
            Constraint::Length(body_area.height.min(9)),
            Constraint::Length(panel_gap),
            Constraint::Fill(1),
        ])
        .areas(body_area)
    };
    let servers_title = match lang {
        Lang::Zh => "MCP Servers",
        Lang::En => "MCP Servers",
    };
    let servers_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(servers_title)
        .border_style(palette.border(focused))
        .style(palette.panel());
    let servers_inner = inset_rect(servers_block.inner(servers_area), 1, 0);
    servers_block.render(servers_area, frame.buffer_mut());

    if modal.draft.mcp_servers.is_empty() {
        let empty = match lang {
            Lang::Zh => "尚未配置\n\n按 a 添加 Server",
            Lang::En => "No servers\n\nPress a to add one",
        };
        Paragraph::new(empty)
            .style(palette.panel().fg(palette.muted))
            .wrap(Wrap { trim: false })
            .render(servers_inner, frame.buffer_mut());
    } else {
        let visible_rows = servers_inner.height as usize;
        let selected = modal
            .mcp_pos
            .min(modal.draft.mcp_servers.len().saturating_sub(1));
        let start = selected
            .saturating_add(1)
            .saturating_sub(visible_rows.max(1));
        for (row, config) in modal
            .draft
            .mcp_servers
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_rows)
        {
            let is_selected = row == selected;
            let snapshot = modal.mcp_statuses.get(&config.id);
            let (status, marker, color) = if !config.enabled {
                (
                    mcp_status_text(lang, McpServerStatus::Disabled),
                    "○",
                    palette.muted,
                )
            } else if let Some(snapshot) = snapshot {
                match snapshot.status {
                    McpServerStatus::Disabled => (
                        mcp_status_text(lang, McpServerStatus::Disabled),
                        "○",
                        palette.muted,
                    ),
                    McpServerStatus::Starting => (
                        mcp_status_text(lang, McpServerStatus::Starting),
                        SPINNER[ticks as usize % SPINNER.len()],
                        palette.warning,
                    ),
                    McpServerStatus::Connected => (
                        mcp_status_text(lang, McpServerStatus::Connected),
                        "●",
                        palette.success,
                    ),
                    McpServerStatus::Failed => (
                        mcp_status_text(lang, McpServerStatus::Failed),
                        "!",
                        palette.danger,
                    ),
                    McpServerStatus::Stopping => (
                        mcp_status_text(lang, McpServerStatus::Stopping),
                        SPINNER[ticks as usize % SPINNER.len()],
                        palette.warning,
                    ),
                }
            } else {
                (
                    match lang {
                        Lang::Zh => "待保存",
                        Lang::En => "pending save",
                    },
                    "◇",
                    palette.warning,
                )
            };
            let name_width = servers_inner
                .width
                .saturating_sub(4 + status.width() as u16) as usize;
            let name = truncate_width(&config.name, name_width);
            let name_padding = " ".repeat(name_width.saturating_sub(name.width()));
            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "❯ " } else { "  " },
                    Style::default().fg(if is_selected {
                        palette.accent
                    } else {
                        palette.muted
                    }),
                ),
                Span::styled(format!("{marker} "), Style::default().fg(color)),
                Span::styled(
                    format!("{name}{name_padding}"),
                    Style::default()
                        .fg(if is_selected {
                            palette.text
                        } else {
                            palette.muted
                        })
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(status, Style::default().fg(color)),
            ]);
            Paragraph::new(line)
                .style(if is_selected {
                    palette.selected()
                } else {
                    palette.panel()
                })
                .render(
                    Rect::new(
                        servers_inner.x,
                        servers_inner.y + (row - start) as u16,
                        servers_inner.width,
                        1,
                    ),
                    frame.buffer_mut(),
                );
        }
    }

    let details_title = match lang {
        Lang::Zh => "连接与能力",
        Lang::En => "Connection & Capabilities",
    };
    let details_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(details_title)
        .border_style(palette.border(false))
        .style(palette.surface());
    let details_inner = inset_rect(details_block.inner(detail_area), 1, 0);
    details_block.render(detail_area, frame.buffer_mut());

    if let Some(config) = modal.draft.mcp_servers.get(
        modal
            .mcp_pos
            .min(modal.draft.mcp_servers.len().saturating_sub(1)),
    ) {
        let snapshot = modal.mcp_statuses.get(&config.id);
        let crate::config::McpTransportConfig::StreamableHttp { url, .. } = &config.transport;
        let transport = format!("HTTP/SSE · {}", redacted_mcp_url(url));
        let capabilities = snapshot
            .map(|snapshot| &snapshot.capabilities)
            .unwrap_or(&config.capabilities);
        let implementation = match (&capabilities.server_name, &capabilities.server_version) {
            (Some(name), Some(version)) => format!("{name} {version}"),
            (Some(name), None) => name.clone(),
            _ => "—".to_owned(),
        };
        let capability_text = format!(
            "resources={} · prompts={}",
            yes_no(capabilities.resources, lang),
            yes_no(capabilities.prompts, lang),
        );
        let labels = match lang {
            Lang::Zh => ["名称", "ID", "Transport", "协议", "实现", "能力", "超时"],
            Lang::En => [
                "Name",
                "ID",
                "Transport",
                "Protocol",
                "Server",
                "Capabilities",
                "Timeout",
            ],
        };
        let values = [
            config.name.clone(),
            config.id.clone(),
            transport,
            capabilities
                .protocol_version
                .clone()
                .unwrap_or_else(|| "—".to_owned()),
            implementation,
            capability_text,
            format!("{} s", config.timeout_seconds),
        ];
        for (index, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
            if index as u16 >= details_inner.height {
                break;
            }
            let line = Line::from(vec![
                Span::styled(
                    format!("{label:>12}  "),
                    Style::default()
                        .fg(palette.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    truncate_width(
                        value,
                        details_inner.width.saturating_sub(16).max(1) as usize,
                    ),
                    Style::default().fg(palette.text),
                ),
            ]);
            Paragraph::new(line).style(palette.surface()).render(
                Rect::new(
                    details_inner.x,
                    details_inner.y + index as u16,
                    details_inner.width,
                    1,
                ),
                frame.buffer_mut(),
            );
        }
        let mut next_y = details_inner.y.saturating_add(8);
        if let Some(error) = snapshot.and_then(|snapshot| snapshot.error.as_deref())
            && next_y < details_inner.y.saturating_add(details_inner.height)
        {
            let prefix = match lang {
                Lang::Zh => "! 连接失败：",
                Lang::En => "! Connection failed: ",
            };
            let height = details_inner
                .y
                .saturating_add(details_inner.height)
                .saturating_sub(next_y)
                .min(3);
            Paragraph::new(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default()
                        .fg(palette.danger)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(error, Style::default().fg(palette.danger)),
            ]))
            .wrap(Wrap { trim: false })
            .render(
                Rect::new(details_inner.x, next_y, details_inner.width, height),
                frame.buffer_mut(),
            );
            next_y = next_y.saturating_add(height);
        }
        if let Some(error) = &modal.validation_error
            && next_y < details_inner.y.saturating_add(details_inner.height)
        {
            Paragraph::new(Span::styled(format!("! {error}"), palette.danger))
                .wrap(Wrap { trim: false })
                .render(
                    Rect::new(
                        details_inner.x,
                        next_y,
                        details_inner.width,
                        details_inner
                            .y
                            .saturating_add(details_inner.height)
                            .saturating_sub(next_y)
                            .min(2),
                    ),
                    frame.buffer_mut(),
                );
        }
    }

    if hint_area.height > 0 {
        let hint = match lang {
            Lang::Zh => "a 新增 · e 编辑 · d 删除 · Space 启停 · c 连接 · r 重连 · Ctrl+S 保存",
            Lang::En => {
                "a Add · e Edit · d Remove · Space Enable · c Connect · r Reconnect · Ctrl+S Save"
            }
        };
        Paragraph::new(Span::styled(hint, palette.muted)).render(
            Rect::new(
                hint_area.x.saturating_add(2),
                hint_area.y,
                hint_area.width.saturating_sub(4),
                1,
            ),
            frame.buffer_mut(),
        );
    }
}

fn mcp_status_text(lang: Lang, status: McpServerStatus) -> &'static str {
    match (lang, status) {
        (Lang::Zh, McpServerStatus::Disabled) => "已禁用",
        (Lang::Zh, McpServerStatus::Starting) => "连接中",
        (Lang::Zh, McpServerStatus::Connected) => "已连接",
        (Lang::Zh, McpServerStatus::Failed) => "失败",
        (Lang::Zh, McpServerStatus::Stopping) => "关闭中",
        (Lang::En, McpServerStatus::Disabled) => "disabled",
        (Lang::En, McpServerStatus::Starting) => "starting",
        (Lang::En, McpServerStatus::Connected) => "connected",
        (Lang::En, McpServerStatus::Failed) => "failed",
        (Lang::En, McpServerStatus::Stopping) => "stopping",
    }
}

fn yes_no(value: bool, lang: Lang) -> &'static str {
    match (lang, value) {
        (Lang::Zh, true) => "是",
        (Lang::Zh, false) => "否",
        (Lang::En, true) => "yes",
        (Lang::En, false) => "no",
    }
}

fn redacted_mcp_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return "<invalid URL>".to_owned();
    };
    if url.query().is_some() {
        url.set_query(Some("[REDACTED]"));
    }
    url.to_string()
}

fn render_network(
    modal: &SettingsUi,
    area: Rect,
    frame: &mut Frame,
    texts: &Texts,
    palette: Palette,
) {
    let focused = modal.pane == SettingsPane::Content;
    let roomy = area.height >= 13;
    let mode_top = 1;
    let url_top = if roomy { 4 } else { 3 };
    let hint_top = if roomy { 7 } else { 5 };
    let error_top = if roomy { 11 } else { 9 };
    let mode_selected = modal.network_pos == 0;
    let mode_line = Line::from(vec![
        Span::styled(
            if mode_selected && focused {
                "❯ "
            } else {
                "  "
            },
            Style::default()
                .fg(if mode_selected {
                    palette.accent
                } else {
                    palette.muted
                })
                .bg(palette.surface),
        ),
        Span::styled(
            format!(
                "{:>width$}  ",
                texts.network_field_label(NetworkField::ProxyMode),
                width = LABEL_WIDTH
            ),
            Style::default()
                .fg(palette.primary)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "[{}] {}",
                texts.proxy_mode_label(modal.draft.proxy.mode),
                texts.settings_toggle_hint
            ),
            Style::default()
                .fg(palette.success)
                .bg(palette.surface)
                .add_modifier(if mode_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ]);
    Paragraph::new(mode_line)
        .style(palette.surface())
        .render(vertical_slice(area, mode_top, 1), frame.buffer_mut());

    let url_selected = modal.network_pos == 1;
    let prefix = format!(
        "{:>width$}  ",
        texts.network_field_label(NetworkField::ProxyUrl),
        width = LABEL_WIDTH
    );
    let value_col = 2 + prefix.chars().count();
    let available = (area.width as usize).saturating_sub(value_col + 1);
    let (value, cursor_col) = if modal.proxy_editing {
        visible_slice_with_cursor(&modal.proxy_edit.buffer, modal.proxy_edit.cursor, available)
    } else {
        let display = redacted_proxy_url(modal.draft.proxy.url.as_deref());
        (truncate_width(&display, available), 0)
    };
    let url_line = Line::from(vec![
        Span::styled(
            if url_selected && focused {
                "❯ "
            } else {
                "  "
            },
            Style::default()
                .fg(if url_selected {
                    palette.accent
                } else {
                    palette.muted
                })
                .bg(palette.surface),
        ),
        Span::styled(
            prefix,
            Style::default()
                .fg(palette.primary)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            value,
            Style::default()
                .fg(
                    if modal.draft.proxy.mode == crate::config::ProxyMode::Disabled {
                        palette.muted
                    } else {
                        palette.text
                    },
                )
                .bg(palette.surface),
        ),
    ]);
    let url_area = vertical_slice(area, url_top, 1);
    Paragraph::new(url_line)
        .style(palette.surface())
        .render(url_area, frame.buffer_mut());
    if modal.proxy_editing && url_area.width > 0 && url_area.height > 0 {
        frame.set_cursor_position(Position::new(
            url_area.x + ((value_col + cursor_col) as u16).min(url_area.width - 1),
            url_area.y,
        ));
    }

    Paragraph::new(vec![
        Line::from(Span::styled(texts.proxy_url_hint, palette.muted)),
        Line::from(Span::styled(texts.proxy_edit_hint, palette.muted)),
    ])
    .style(palette.surface())
    .wrap(Wrap { trim: false })
    .render(
        inset_rect(vertical_slice(area, hint_top, 3), 1, 0),
        frame.buffer_mut(),
    );

    if let Some(error) = &modal.validation_error {
        Paragraph::new(Line::from(vec![
            Span::styled("! ", palette.danger),
            Span::styled(error, palette.danger),
        ]))
        .style(palette.surface())
        .wrap(Wrap { trim: false })
        .render(
            inset_rect(vertical_slice(area, error_top, 2), 1, 0),
            frame.buffer_mut(),
        );
    }
}

fn redacted_proxy_url(raw: Option<&str>) -> String {
    let Some(raw) = raw.filter(|url| !url.trim().is_empty()) else {
        return "—".to_owned();
    };
    let Ok(url) = url::Url::parse(raw) else {
        return raw.to_owned();
    };
    if url.password().is_none() {
        return raw.to_owned();
    }

    let Some(authority_start) = raw.find("://").map(|index| index + 3) else {
        return raw.to_owned();
    };
    let authority_end = raw[authority_start..]
        .find(['/', '?', '#'])
        .map(|index| authority_start + index)
        .unwrap_or(raw.len());
    let authority = &raw[authority_start..authority_end];
    let Some(at) = authority.rfind('@') else {
        return raw.to_owned();
    };
    let Some(colon) = authority[..at].find(':') else {
        return raw.to_owned();
    };
    let password_start = authority_start + colon + 1;
    let password_end = authority_start + at;
    format!("{}••••{}", &raw[..password_start], &raw[password_end..])
}

/// 「键盘」分类：每个命令独立显示并校验，避免冲突或拦截普通字符。
fn render_keyboard(modal: &SettingsUi, area: Rect, frame: &mut Frame, palette: Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let labels = match modal.draft.language {
        Lang::Zh => [
            "发送消息",
            "插入换行",
            "上一条历史",
            "下一条历史",
            "向左一词",
            "向右一词",
            "全选输入",
            "复制选区",
        ],
        Lang::En => [
            "Send message",
            "Insert newline",
            "Previous history",
            "Next history",
            "Word left",
            "Word right",
            "Select input",
            "Copy selection",
        ],
    };
    let focused = modal.pane == SettingsPane::Content;
    let body_height = area.height.saturating_sub(5) as usize;
    let stride = if body_height >= 14 { 3 } else { 2 };
    let visible = if body_height >= 2 {
        (body_height - 2) / stride + 1
    } else {
        1
    };
    let selected = modal
        .keybinding_pos
        .min(KeybindingField::ALL.len().saturating_sub(1));
    let start = selection_offset(selected, KeybindingField::ALL.len(), visible);

    for (display_index, (index, field)) in KeybindingField::ALL
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let row_y = area.y.saturating_add((display_index * stride) as u16);
        if row_y >= area.bottom() {
            break;
        }
        let is_selected = index == selected;
        let editing = is_selected && modal.keybinding_edit.is_some();
        let state = match (modal.draft.language, editing) {
            (Lang::Zh, true) => " [编辑中]",
            (Lang::En, true) => " [editing]",
            _ => "",
        };
        Paragraph::new(Line::from(vec![
            Span::styled(
                if is_selected && focused { "❯ " } else { "  " },
                Style::default().fg(if is_selected {
                    palette.accent
                } else {
                    palette.muted
                }),
            ),
            Span::styled(
                format!("{}{}", labels[index], state),
                Style::default()
                    .fg(if is_selected {
                        palette.primary
                    } else {
                        palette.text
                    })
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]))
        .style(palette.surface())
        .render(Rect::new(area.x, row_y, area.width, 1), frame.buffer_mut());

        if row_y.saturating_add(1) >= area.bottom() {
            continue;
        }
        let available = (area.width as usize).saturating_sub(5);
        let (value, cursor) = if editing {
            let edit = modal
                .keybinding_edit
                .as_ref()
                .expect("editing state exists");
            visible_slice_with_cursor(&edit.buffer, edit.cursor, available)
        } else {
            (
                truncate_width(
                    &field.binding(&modal.draft.keybindings).to_string(),
                    available,
                ),
                0,
            )
        };
        Paragraph::new(Line::from(vec![
            Span::styled("  [", Style::default().fg(palette.muted)),
            Span::styled(
                value,
                Style::default()
                    .fg(if is_selected {
                        palette.accent
                    } else {
                        palette.text
                    })
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled("]", Style::default().fg(palette.muted)),
        ]))
        .style(palette.surface())
        .render(
            Rect::new(area.x, row_y + 1, area.width, 1),
            frame.buffer_mut(),
        );
        if editing && area.width > 3 {
            frame.set_cursor_position(Position::new(
                area.x + 3 + (cursor as u16).min(area.width.saturating_sub(4)),
                row_y + 1,
            ));
        }
    }

    let hint = match modal.draft.language {
        Lang::Zh => "Enter 编辑/确认 · 例：ctrl+left、alt+c · Esc 取消 · Ctrl+S 保存",
        Lang::En => "Enter edit/confirm · e.g. ctrl+left, alt+c · Esc cancel · Ctrl+S save",
    };
    let hint_y = area.bottom().saturating_sub(4).max(area.y);
    Paragraph::new(Span::styled(hint, palette.muted))
        .wrap(Wrap { trim: false })
        .render(
            Rect::new(
                area.x.saturating_add(2),
                hint_y,
                area.width.saturating_sub(2),
                2.min(area.bottom().saturating_sub(hint_y)),
            ),
            frame.buffer_mut(),
        );
    if let Some(error) = &modal.validation_error {
        let error_y = area.bottom().saturating_sub(2).max(area.y);
        Paragraph::new(Line::from(vec![
            Span::styled("! ", palette.danger),
            Span::styled(error, palette.danger),
        ]))
        .wrap(Wrap { trim: false })
        .render(
            Rect::new(
                area.x.saturating_add(2),
                error_y,
                area.width.saturating_sub(2),
                area.bottom().saturating_sub(error_y),
            ),
            frame.buffer_mut(),
        );
    }
}

/// 「外观」分类。
fn render_appearance(
    modal: &SettingsUi,
    area: Rect,
    buf: &mut Buffer,
    texts: &Texts,
    palette: Palette,
) {
    let focus = modal.pane == SettingsPane::Content;
    let stride = if area.height >= 11 { 3 } else { 2 };
    let values = [
        modal.draft.language.label(),
        modal.draft.theme.label(),
        if modal.draft.show_titan {
            texts.value_on
        } else {
            texts.value_off
        },
    ];
    let value_colors = [
        palette.primary,
        palette.accent,
        if modal.draft.show_titan {
            palette.success
        } else {
            palette.muted
        },
    ];
    for (index, field) in SettingsField::ALL.iter().enumerate() {
        let selected = index == modal.appearance_pos;
        let label = Span::styled(
            format!(
                "{:>width$}  ",
                texts.field_label(*field),
                width = LABEL_WIDTH
            ),
            Style::default()
                .fg(palette.primary)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        );
        let line = Line::from(vec![
            Span::styled(
                if selected && focus { "❯ " } else { "  " },
                Style::default()
                    .fg(if selected {
                        palette.accent
                    } else {
                        palette.muted
                    })
                    .bg(palette.surface),
            ),
            label,
            Span::styled(
                format!("[{}] {}", values[index], texts.settings_toggle_hint),
                Style::default().fg(value_colors[index]).bg(palette.surface),
            ),
        ])
        .style(if selected {
            palette.surface().add_modifier(Modifier::BOLD)
        } else {
            palette.surface()
        });
        Paragraph::new(line).render(vertical_slice(area, 1 + index as u16 * stride, 1), buf);
    }
}

/// 手动添加模型行（带光标）。
fn render_manual_line(
    frame: &mut Frame,
    area: Rect,
    edit: &LineEdit,
    label: &str,
    palette: Palette,
) {
    let prefix = format!("{label}: ");
    let prefix_len = prefix.chars().count();
    let avail = (area.width as usize).saturating_sub(prefix_len + 1);
    let (visible, col) = visible_slice_with_cursor(&edit.buffer, edit.cursor, avail);
    let line = Line::from(vec![
        Span::styled(prefix, palette.surface().fg(palette.primary)),
        Span::styled(visible, palette.surface()),
    ]);
    Paragraph::new(line)
        .style(palette.surface())
        .render(area, frame.buffer_mut());
    if area.width > 0 && area.height > 0 {
        frame.set_cursor_position(Position::new(
            area.x + ((prefix_len + col) as u16).min(area.width - 1),
            area.y,
        ));
    }
}

fn render_mcp_wizard(frame: &mut Frame, wizard: &McpServerWizard, lang: Lang, palette: Palette) {
    let title = match (lang, wizard.is_editing()) {
        (Lang::Zh, false) => "新增远程 MCP Server",
        (Lang::Zh, true) => "编辑远程 MCP Server",
        (Lang::En, false) => "Add Remote MCP Server",
        (Lang::En, true) => "Edit Remote MCP Server",
    };
    let hint = match lang {
        Lang::Zh => "Tab/↑/↓ 切换字段 · Enter 保存 · Esc 取消 · Secret 可使用 ${ENV_VAR}",
        Lang::En => "Tab/↑/↓ Field · Enter Save · Esc Cancel · Secrets support ${ENV_VAR}",
    };
    let area = popup_rect(frame.area(), 92, 28);
    Clear.render(area, frame.buffer_mut());
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = inset_rect(block.inner(area), 1, 1);
    block.render(area, frame.buffer_mut());
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let labels: &[&str] = match lang {
        Lang::Zh => &[
            "名称",
            "ID",
            "URL",
            "Headers JSON",
            "Bearer Token",
            "超时秒数",
        ],
        Lang::En => &[
            "Name",
            "ID",
            "URL",
            "Headers JSON",
            "Bearer Token",
            "Timeout sec",
        ],
    };
    let error_height = wizard.error.as_ref().map_or(0, |_| inner.height.min(3));
    let fields_height = inner.height.saturating_sub(error_height);
    let roomy = fields_height as usize >= labels.len().saturating_mul(2).saturating_sub(1);
    let stride = if roomy { 2 } else { 1 };
    let visible_rows = if fields_height == 0 {
        0
    } else {
        ((fields_height - 1) / stride + 1) as usize
    };
    let start = selection_offset(wizard.field, labels.len(), visible_rows.max(1));
    let label_width = 14usize.min(inner.width.saturating_sub(5) as usize);
    let value_col = 2 + label_width + 2;
    let mut cursor = None;
    for (display_row, (index, label)) in labels
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
        .enumerate()
    {
        let row_y = inner.y.saturating_add(display_row as u16 * stride);
        let selected = wizard.field == index;
        let available = (inner.width as usize).saturating_sub(value_col).max(1);
        let masked;
        let buffer = if wizard.sensitive_field(index) {
            masked = vec!['•'; wizard.line(index).buffer.len()];
            masked.as_slice()
        } else {
            wizard.line(index).buffer.as_slice()
        };
        let (visible, column) =
            visible_slice_with_cursor(buffer, wizard.line(index).cursor, available);
        let label = truncate_width(label, label_width);
        let label_padding = " ".repeat(label_width.saturating_sub(label.width()));
        let line = Line::from(vec![
            Span::styled(
                if selected { "❯ " } else { "  " },
                Style::default().fg(if selected {
                    palette.accent
                } else {
                    palette.muted
                }),
            ),
            Span::styled(
                format!("{label_padding}{label}  "),
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                visible,
                Style::default()
                    .fg(if selected {
                        palette.text
                    } else {
                        palette.muted
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]);
        Paragraph::new(line).style(palette.surface()).render(
            Rect::new(inner.x, row_y, inner.width, 1),
            frame.buffer_mut(),
        );
        if selected {
            cursor = Some(Position::new(inner.x + (value_col + column) as u16, row_y));
        }
    }
    if let Some(error) = &wizard.error {
        let error_y = inner.y.saturating_add(fields_height);
        Paragraph::new(Span::styled(format!("! {error}"), palette.danger))
            .wrap(Wrap { trim: false })
            .render(
                Rect::new(inner.x, error_y, inner.width, error_height),
                frame.buffer_mut(),
            );
    }
    if let Some(position) = cursor
        && inner.width > 0
        && inner.height > 0
    {
        frame.set_cursor_position(Position::new(
            position.x.min(inner.x + inner.width - 1),
            position.y.min(inner.y + inner.height - 1),
        ));
    }
}

/// 新增供应商向导。
fn render_wizard(
    frame: &mut Frame,
    wizard: &ProviderWizard,
    lang: Lang,
    ticks: u64,
    palette: Palette,
) {
    let texts = lang.texts();
    let hint = match wizard.step {
        1 => texts.wizard_step1_hint,
        2 => texts.wizard_step2_hint,
        _ => texts.wizard_step3_hint,
    };
    let title = texts.wizard_title(wizard.is_editing(), wizard.step);
    let area = popup_rect(frame.area(), 84, 24);
    Clear.render(area, frame.buffer_mut());
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_bottom(Line::from(hint).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = inset_rect(block.inner(area), 1, 1);
    block.render(area, frame.buffer_mut());

    match wizard.step {
        // 1/3 选择协议
        1 => {
            let options = [
                (
                    crate::config::ApiKind::ChatCompletions.label(),
                    texts.wizard_kind_chat_hint,
                ),
                (
                    crate::config::ApiKind::Responses.label(),
                    texts.wizard_kind_resp_hint,
                ),
                (
                    crate::config::ApiKind::AnthropicMessages.label(),
                    texts.wizard_kind_anthropic_hint,
                ),
                (
                    crate::config::ApiKind::GeminiGenerateContent.label(),
                    texts.wizard_kind_gemini_hint,
                ),
            ];
            for (index, (name, desc)) in options.iter().enumerate() {
                let selected = index == wizard.kind_pos;
                let line = Line::from(vec![
                    Span::styled(
                        if selected { "❯ " } else { "  " },
                        Style::default()
                            .fg(if selected {
                                palette.accent
                            } else {
                                palette.muted
                            })
                            .bg(palette.surface),
                    ),
                    Span::styled(
                        format!("{name:<20}"),
                        if selected {
                            palette.surface().add_modifier(Modifier::BOLD)
                        } else {
                            palette.surface()
                        },
                    ),
                    Span::styled(
                        *desc,
                        Style::default().fg(palette.muted).bg(palette.surface),
                    ),
                ]);
                Paragraph::new(line).render(
                    vertical_slice(inner, 2 + index as u16 * 2, 1),
                    frame.buffer_mut(),
                );
            }
        }
        // 2/3 连接配置
        2 => {
            let labels = [
                format!("{} {}", texts.wizard_name, texts.wizard_name_hint),
                texts.wizard_url.to_owned(),
                texts.wizard_key.to_owned(),
                texts.wizard_headers.to_owned(),
            ];
            let value_col = 2 + LABEL_WIDTH + 2;
            let mut cursor: Option<Position> = None;
            for (index, label) in labels.iter().enumerate() {
                let selected = index == wizard.field;
                let row = Rect {
                    y: inner.y + 1 + index as u16 * 2,
                    x: inner.x,
                    width: inner.width,
                    height: 1,
                };
                let avail = (row.width as usize).saturating_sub(value_col + 1);
                let masked;
                let buffer = if index >= 2 {
                    masked = vec!['•'; wizard.line(index).buffer.len()];
                    masked.as_slice()
                } else {
                    wizard.line(index).buffer.as_slice()
                };
                let (visible, col) =
                    visible_slice_with_cursor(buffer, wizard.line(index).cursor, avail);
                let line = Line::from(vec![
                    Span::styled(
                        if selected { "❯ " } else { "  " },
                        Style::default()
                            .fg(if selected {
                                palette.accent
                            } else {
                                palette.muted
                            })
                            .bg(palette.surface),
                    ),
                    Span::styled(
                        format!("{:>width$}  ", label, width = LABEL_WIDTH),
                        Style::default()
                            .fg(palette.primary)
                            .bg(palette.surface)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(visible, palette.surface()),
                ]);
                Paragraph::new(line).render(row, frame.buffer_mut());
                if selected {
                    cursor = Some(Position::new(row.x + (value_col + col) as u16, row.y));
                }
            }
            if let Some(position) = cursor
                && inner.width > 0
                && inner.height > 0
            {
                let max_x = inner.right() - 1;
                let max_y = inner.bottom() - 1;
                frame.set_cursor_position(Position::new(
                    position.x.min(max_x),
                    position.y.min(max_y),
                ));
            }
            Paragraph::new(Span::styled(
                texts.wizard_url_hint,
                Style::default().fg(palette.muted).bg(palette.surface),
            ))
            .render(vertical_slice(inner, 9, 1), frame.buffer_mut());
            if let Some(error) = &wizard.error {
                Paragraph::new(Span::styled(
                    format!("✗ {error}"),
                    Style::default().fg(palette.danger).bg(palette.surface),
                ))
                .render(vertical_slice(inner, 11, 1), frame.buffer_mut());
            }
        }
        // 3/3 选择模型
        _ => {
            let spinner = SPINNER[ticks as usize % SPINNER.len()];
            let status: Line = if wizard.fetching {
                Line::from(Span::styled(
                    format!("{spinner} {}", texts.syncing),
                    Style::default().fg(palette.warning).bg(palette.surface),
                ))
            } else if let Some(err) = &wizard.error {
                Line::from(vec![
                    Span::styled(
                        format!("✗ {err}  "),
                        Style::default().fg(palette.danger).bg(palette.surface),
                    ),
                    Span::styled(
                        texts.wizard_retry_hint,
                        Style::default().fg(palette.muted).bg(palette.surface),
                    ),
                ])
            } else if wizard.warned {
                Line::from(Span::styled(
                    texts.wizard_need_one,
                    Style::default().fg(palette.danger).bg(palette.surface),
                ))
            } else if let Some(list) = &wizard.fetched {
                Line::from(Span::styled(
                    format!("✓ {}", lang.wizard_sync_ok(list.len())),
                    Style::default().fg(palette.success).bg(palette.surface),
                ))
            } else {
                Line::from("")
            };
            Paragraph::new(status).render(vertical_slice(inner, 1, 1), frame.buffer_mut());

            // 模型勾选列表（窗口滚动，选中项保持可见）
            let list_area = Rect {
                y: inner.y + 3,
                height: inner.height.saturating_sub(5),
                ..inner
            };
            if let Some(list) = &wizard.fetched {
                let visible = list_area.height as usize;
                let len = list.len();
                let max_offset = len.saturating_sub(visible.min(len));
                let offset = wizard
                    .list_pos
                    .checked_sub(visible)
                    .map(|value| value + 1)
                    .unwrap_or(0)
                    .min(max_offset);
                let mut lines = Vec::new();
                for (index, name) in list.iter().enumerate().skip(offset) {
                    if index >= offset + visible {
                        break;
                    }
                    let checked = wizard.selected.get(index).copied().unwrap_or(false);
                    let selected = index == wizard.list_pos;
                    let row = format!(
                        "{}{} {}",
                        if selected { "❯" } else { " " },
                        if checked { "☑" } else { "☐" },
                        name,
                    );
                    let style = if selected {
                        palette.selected()
                    } else {
                        palette.surface()
                    };
                    lines.push(Line::from(Span::styled(row, style)));
                }
                Paragraph::new(lines).render(list_area, frame.buffer_mut());
            }

            // 手动添加行
            if inner.height > 0 {
                let manual_row = Rect {
                    y: inner.bottom() - 1,
                    x: inner.x,
                    width: inner.width,
                    height: 1,
                };
                if wizard.manual_active {
                    render_manual_line(
                        frame,
                        manual_row,
                        &wizard.manual,
                        texts.manual_add_label,
                        palette,
                    );
                } else {
                    Paragraph::new(Span::styled(
                        format!("n: {}", texts.manual_add_label),
                        Style::default().fg(palette.muted).bg(palette.surface),
                    ))
                    .render(manual_row, frame.buffer_mut());
                }
            }
        }
    }
}
