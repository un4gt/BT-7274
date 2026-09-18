//! Transcript layout: ordered answers, compact process groups, and anchored history reading.
use super::{
    activity::BlockRef, markdown_view::MarkdownView, mouse::MouseTarget, spinner_frame,
    theme::Palette, tool_view,
};
use crate::{
    app::{App, NoticeLevel},
    i18n::Lang,
    session::{BlockKind, Message, MessageBlock, MessageStatus, Role, ToolStatus},
    text::{truncate_width, wrap_lines},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Widget,
    },
};

#[derive(Debug, Clone)]
struct RowKey {
    block: BlockRef,
    offset: usize,
}

#[derive(Debug)]
pub struct ChatViewState {
    session_id: String,
    anchor: Option<RowKey>,
    pending_scroll: isize,
    follow_tail: bool,
    pub visible_activity: Option<BlockRef>,
    activity_rows: Vec<(usize, BlockRef)>,
    markdown: MarkdownView,
}

impl Default for ChatViewState {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            anchor: None,
            pending_scroll: 0,
            follow_tail: true,
            visible_activity: None,
            activity_rows: Vec::new(),
            markdown: MarkdownView::default(),
        }
    }
}

impl ChatViewState {
    pub fn follow_latest(&mut self) {
        self.anchor = None;
        self.pending_scroll = 0;
        self.follow_tail = true;
    }
    pub fn scroll_by(&mut self, delta: isize) {
        self.pending_scroll = self.pending_scroll.saturating_add(delta);
    }
}

struct Row {
    line: Line<'static>,
    key: Option<RowKey>,
    activity: Option<BlockRef>,
}

pub(super) struct ScrollbarMetrics {
    pub content_length: usize,
    pub position: usize,
}
pub(super) struct MessageWindow {
    pub lines: Vec<Line<'static>>,
    pub scrollbar: Option<ScrollbarMetrics>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub rendered_messages: usize,
}

pub(super) fn content_area(area: Rect) -> Rect {
    let inner = Block::bordered().inner(area);
    Rect {
        width: inner.width.saturating_sub(u16::from(inner.width >= 4)),
        ..inner
    }
}

pub(super) fn render_messages(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    let hint = match app.settings.language {
        Lang::Zh => "点击过程 / F3 详情 · 滚轮 / PgUp/PgDn 历史",
        Lang::En => "Click process / F3 Details · Wheel / PgUp/PgDn History",
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(palette.border(false))
        .style(palette.surface())
        .title_bottom(
            Line::from(truncate_width(hint, area.width.saturating_sub(4) as usize)).right_aligned(),
        );
    let inner = block.inner(area);
    block.render(area, buf);
    let show_scrollbar = inner.width >= 4;
    let content = content_area(area);
    let window = build_message_window(
        app,
        content.width as usize,
        inner.height as usize,
        0,
        palette,
    );
    {
        let mut mouse = app.mouse.borrow_mut();
        mouse.register(area, MouseTarget::Messages);
        for (row, key) in &app.chat_view.borrow().activity_rows {
            mouse.register(
                Rect::new(content.x, content.y + *row as u16, content.width, 1),
                MouseTarget::Activity(key.clone()),
            );
        }
    }
    Paragraph::new(window.lines)
        .style(palette.surface())
        .render(content, buf);
    if app.startup.is_none() && app.idle_titan_area_available() {
        titan::Idle::new(palette.surface, palette.art_body, palette.accent)
            .animate(app.idle_titan.animation())
            .render_with_opacity(content, buf, app.idle_titan.opacity());
    }
    if show_scrollbar && let Some(metrics) = window.scrollbar {
        StatefulWidget::render(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(palette.border).bg(palette.surface))
                .thumb_style(Style::default().fg(palette.primary).bg(palette.surface)),
            inner,
            buf,
            &mut ScrollbarState::new(metrics.content_length.saturating_sub(inner.height as usize))
                .position(metrics.position)
                .viewport_content_length(inner.height as usize),
        );
    }
}

pub(super) fn build_message_window(
    app: &App,
    width: usize,
    visible: usize,
    requested_scroll: usize,
    palette: Palette,
) -> MessageWindow {
    if width == 0 || visible == 0 {
        let mut state = app.chat_view.borrow_mut();
        state.activity_rows.clear();
        state.visible_activity = None;
        return MessageWindow {
            lines: Vec::new(),
            scrollbar: None,
            rendered_messages: 0,
        };
    }
    let session = app.open_session();
    let mut state = app.chat_view.borrow_mut();
    if state.session_id != session.id {
        *state = ChatViewState {
            session_id: session.id.clone(),
            ..ChatViewState::default()
        };
    }
    if requested_scroll > 0 {
        state.scroll_by(-(requested_scroll.min(isize::MAX as usize) as isize));
    }
    let count = session.messages.len();
    if count == 0 {
        let lines = notice_rows(app, width, palette)
            .into_iter()
            .map(|row| row.line)
            .collect::<Vec<_>>();
        state.visible_activity = None;
        state.activity_rows.clear();
        return MessageWindow {
            lines,
            scrollbar: None,
            rendered_messages: 0,
        };
    }
    let anchored = state.anchor.as_ref().and_then(|anchor| {
        session
            .messages
            .iter()
            .position(|m| m.id == anchor.block.message_id)
    });
    let mut first = if state.follow_tail {
        count - 1
    } else {
        anchored.unwrap_or(count - 1)
    };
    let mut last = first;
    let mut rows = message_rows(
        app,
        &session.messages[first],
        width,
        palette,
        &mut state.markdown,
    );
    if first == 0 {
        let mut prefix = summary_rows(app, width, palette);
        prefix.append(&mut rows);
        rows = prefix;
    }
    if last == count - 1 {
        rows.extend(notice_rows(app, width, palette));
    }
    let mut start = if state.follow_tail {
        rows.len() as isize - visible as isize
    } else {
        state
            .anchor
            .as_ref()
            .and_then(|anchor| {
                rows.iter()
                    .enumerate()
                    .filter(|(_, row)| {
                        row.key.as_ref().is_some_and(|key| {
                            key.block == anchor.block && key.offset <= anchor.offset
                        })
                    })
                    .map(|(i, _)| i as isize)
                    .next_back()
            })
            .unwrap_or(0)
    } + state.pending_scroll;
    state.pending_scroll = 0;
    while start < 0 && first > 0 {
        first -= 1;
        let mut prefix = message_rows(
            app,
            &session.messages[first],
            width,
            palette,
            &mut state.markdown,
        );
        if first == 0 {
            let mut summary = summary_rows(app, width, palette);
            summary.append(&mut prefix);
            prefix = summary;
        }
        start += prefix.len() as isize;
        prefix.append(&mut rows);
        rows = prefix;
    }
    start = start.max(0);
    while start as usize + visible > rows.len() && last + 1 < count {
        last += 1;
        rows.extend(message_rows(
            app,
            &session.messages[last],
            width,
            palette,
            &mut state.markdown,
        ));
        if last + 1 == count {
            rows.extend(notice_rows(app, width, palette));
        }
    }
    let start = (start as usize).min(rows.len().saturating_sub(visible));
    let end = (start + visible).min(rows.len());
    state.follow_tail = last == count - 1 && end == rows.len();
    state.anchor = rows[start..end].iter().find_map(|row| row.key.clone());
    state.visible_activity = rows[start..end]
        .iter()
        .rev()
        .find_map(|row| row.activity.clone());
    let rendered_messages = last - first + 1;
    let average = rows.len().div_ceil(rendered_messages).max(1);
    let content_length = rows.len() + (count - rendered_messages) * average;
    let scrollbar = (content_length > visible).then_some(ScrollbarMetrics {
        content_length,
        position: first * average + start,
    });
    let padding = visible.saturating_sub(end - start);
    state.activity_rows = rows[start..end]
        .iter()
        .enumerate()
        .filter_map(|(row, entry)| entry.activity.clone().map(|key| (padding + row, key)))
        .collect();
    let mut lines = vec![Line::default(); padding];
    lines.extend(
        rows.into_iter()
            .skip(start)
            .take(end - start)
            .map(|row| row.line),
    );
    MessageWindow {
        lines,
        scrollbar,
        rendered_messages,
    }
}

fn keyed_lines(message: &Message, block_id: u64, lines: Vec<Line<'static>>) -> Vec<Row> {
    let mut offset = 0;
    lines
        .into_iter()
        .map(|line| {
            let key = RowKey {
                block: BlockRef {
                    message_id: message.id.clone(),
                    block_id,
                },
                offset,
            };
            offset += line.width().max(1);
            Row {
                line,
                key: Some(key),
                activity: None,
            }
        })
        .collect()
}

fn message_rows(
    app: &App,
    message: &Message,
    width: usize,
    palette: Palette,
    markdown_view: &mut MarkdownView,
) -> Vec<Row> {
    let lang = app.settings.language;
    let (name, color) = match message.role {
        Role::User => (lang.texts().role_user, palette.user),
        Role::Assistant => ("BT-7274", palette.assistant),
    };
    let mut label = format!("❯ {name}");
    if message.role == Role::Assistant {
        if message.status == MessageStatus::Streaming {
            label.push_str(&format!(" · {}", spinner_frame(app.ticks)));
        }
        if let Some(status) = lang.message_status_label(message.status) {
            label.push_str(&format!(" · {status}"));
        }
    }
    let mut rows = keyed_lines(
        message,
        0,
        vec![Line::from(Span::styled(
            truncate_width(&label, width),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))],
    );
    let mut group = Vec::new();
    for block in &message.blocks {
        match &block.kind {
            BlockKind::Text { content } => {
                flush_group(message, &mut group, &mut rows, width, app, palette);
                rows.extend(keyed_lines(
                    message,
                    block.id,
                    markdown_view.lines(
                        &message.id,
                        block,
                        content,
                        width,
                        palette,
                        app.settings.theme,
                    ),
                ));
            }
            BlockKind::Reasoning { .. } | BlockKind::Tool { .. } => group.push(block),
            BlockKind::System { .. } => {}
        }
    }
    flush_group(message, &mut group, &mut rows, width, app, palette);
    if rows.len() == 1 && message.status != MessageStatus::Streaming {
        rows.extend(keyed_lines(
            message,
            u64::MAX - 1,
            wrap_lines(lang.empty_assistant_placeholder(message.status), width)
                .into_iter()
                .map(Line::from)
                .collect(),
        ));
    }
    rows.extend(keyed_lines(message, u64::MAX, vec![Line::default()]));
    rows
}

fn flush_group(
    message: &Message,
    group: &mut Vec<&MessageBlock>,
    rows: &mut Vec<Row>,
    width: usize,
    app: &App,
    palette: Palette,
) {
    if group.is_empty() {
        return;
    }
    let lang = app.settings.language;
    let mut thinking = false;
    let mut tools = 0;
    let mut failures = 0;
    let mut cancelled = false;
    let mut incomplete = false;
    let mut active = false;
    let mut current_tool = None;
    let mut last_tool = None;
    let mut selected = group.last().unwrap().id;
    for block in group.iter() {
        match &block.kind {
            BlockKind::Reasoning { status, .. } => {
                thinking = true;
                if *status == MessageStatus::Streaming {
                    active = true;
                    selected = block.id;
                }
                cancelled |= *status == MessageStatus::Cancelled;
                incomplete |= *status == MessageStatus::Failed;
            }
            BlockKind::Tool { tool } => {
                tools += 1;
                last_tool = Some(block.id);
                failures += usize::from(tool.status == ToolStatus::Failed);
                cancelled |= tool.status == ToolStatus::Cancelled;
                incomplete |= tool.status == ToolStatus::Incomplete;
                if tool.status.is_active() {
                    active = true;
                    if current_tool.is_none() || tool.status == ToolStatus::Running {
                        current_tool = Some(tool);
                        selected = block.id;
                    }
                }
            }
            _ => {}
        }
    }
    if !active && let Some(id) = last_tool {
        selected = id;
    }
    let mut labels = Vec::new();
    if thinking {
        labels.push(
            match lang {
                Lang::Zh => "思考",
                Lang::En => "Thought",
            }
            .to_owned(),
        );
    }
    if tools > 0 {
        labels.push(match lang {
            Lang::Zh => format!("{tools} 项工具"),
            Lang::En => format!("{tools} tools"),
        });
    }
    if active {
        labels.push(if let Some(tool) = current_tool {
            format!(
                "{} {}",
                tool_view::status_label(tool.status, lang),
                tool_view::name(tool, lang)
            )
        } else {
            match lang {
                Lang::Zh => "思考中",
                Lang::En => "Thinking",
            }
            .to_owned()
        });
    } else if cancelled {
        labels.push(
            match lang {
                Lang::Zh => "已取消",
                Lang::En => "Cancelled",
            }
            .to_owned(),
        );
    } else if incomplete {
        labels.push(
            match lang {
                Lang::Zh => "未完成",
                Lang::En => "Incomplete",
            }
            .to_owned(),
        );
    } else if failures == 0 {
        labels.push(
            match lang {
                Lang::Zh => "已完成",
                Lang::En => "Complete",
            }
            .to_owned(),
        );
    }
    if failures > 0 {
        labels.push(match lang {
            Lang::Zh => format!("{failures} 项失败"),
            Lang::En => format!("{failures} failed"),
        });
    }
    let color = if failures > 0 {
        palette.danger
    } else if active {
        palette.warning
    } else {
        palette.muted
    };
    let label = format!(
        "▸ {}{}",
        if active {
            format!("{} ", spinner_frame(app.ticks))
        } else {
            String::new()
        },
        labels.join(" · ")
    );
    let mut summary = keyed_lines(
        message,
        group[0].id,
        vec![Line::from(Span::styled(
            truncate_width(&label, width),
            Style::default().fg(color),
        ))],
    );
    summary[0].activity = Some(BlockRef {
        message_id: message.id.clone(),
        block_id: selected,
    });
    rows.extend(summary);
    group.clear();
}

fn summary_rows(app: &App, width: usize, palette: Palette) -> Vec<Row> {
    let Some(summary) = &app.open_session().summary else {
        return Vec::new();
    };
    let text = match app.settings.language {
        Lang::Zh => "◇ 已压缩上下文（原文可撤销）",
        Lang::En => "◇ Compacted context (original is reversible)",
    };
    let lines = std::iter::once(Line::from(truncate_width(text, width)))
        .chain(
            wrap_lines(&summary.context_text(), width)
                .into_iter()
                .map(|text| Line::from(Span::styled(text, palette.muted))),
        )
        .collect();
    keyed_lines(&app.open_session().messages[0], u64::MAX - 2, lines)
}

fn notice_rows(app: &App, width: usize, palette: Palette) -> Vec<Row> {
    let Some(notice) = app.notice() else {
        return Vec::new();
    };
    let color = match notice.level {
        NoticeLevel::Info => palette.primary,
        NoticeLevel::Warning => palette.warning,
        NoticeLevel::Error => palette.danger,
    };
    vec![Row {
        line: Line::from(Span::styled(
            truncate_width(&notice.summary.replace(['\n', '\r'], " "), width),
            Style::default().fg(color),
        )),
        key: None,
        activity: None,
    }]
}
