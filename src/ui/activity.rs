//! Modal process browser. Selection and scroll belong to the browser, not the transcript.
use super::{
    dim_background, markdown, mouse::MouseTarget, popup_rect, theme, think_view, tool_view,
};
use crate::{
    app::App,
    config::Theme,
    i18n::Lang,
    session::{BlockKind, Message, MessageBlock, Role, Session, ToolStatus},
    text::truncate_width,
};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{
        Block, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs, Widget,
    },
};
use std::cell::RefCell;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRef {
    pub message_id: String,
    pub block_id: u64,
}

#[derive(Debug)]
pub struct ActivityOverlay {
    pub session_id: String,
    pub selected: Option<BlockRef>,
    pub detail_focus: bool,
    pub tab: usize,
    viewport: RefCell<DetailViewport>,
    status: Option<String>,
}

#[derive(Default, Debug)]
struct DetailViewport {
    vertical: usize,
    horizontal: usize,
    max_vertical: usize,
    max_horizontal: usize,
    height: usize,
    follow_tail: bool,
    cache: Option<DetailCache>,
}

#[derive(Debug)]
struct DetailCache {
    key: BlockRef,
    revision: u64,
    tab: usize,
    width: usize,
    theme: Theme,
    lang: Lang,
    lines: Vec<Line<'static>>,
}

struct Item<'a> {
    key: BlockRef,
    turn: usize,
    message: &'a Message,
    block: Option<&'a MessageBlock>,
}

fn items(session: &Session) -> Vec<Item<'_>> {
    let mut items = Vec::new();
    let mut turn = 0;
    for message in &session.messages {
        if message.role != Role::Assistant {
            continue;
        }
        turn += 1;
        for block in &message.blocks {
            if matches!(
                block.kind,
                BlockKind::Reasoning { .. } | BlockKind::Tool { .. }
            ) {
                items.push(Item {
                    key: BlockRef {
                        message_id: message.id.clone(),
                        block_id: block.id,
                    },
                    turn,
                    message,
                    block: Some(block),
                });
            }
        }
        if message
            .blocks
            .iter()
            .any(|b| matches!(b.kind, BlockKind::System { .. }))
        {
            items.push(Item {
                key: BlockRef {
                    message_id: message.id.clone(),
                    block_id: 0,
                },
                turn,
                message,
                block: None,
            });
        }
    }
    items
}

impl ActivityOverlay {
    pub fn new(session: &Session, preferred: Option<BlockRef>) -> Self {
        let entries = items(session);
        let chosen = preferred
            .and_then(|key| entries.iter().find(|item| item.key == key))
            .or_else(|| entries.last());
        let mut overlay = Self {
            session_id: session.id.clone(),
            selected: None,
            detail_focus: false,
            tab: 0,
            viewport: RefCell::default(),
            status: None,
        };
        if let Some(item) = chosen {
            overlay.select(item);
        }
        overlay
    }

    fn select(&mut self, item: &Item<'_>) {
        self.selected = Some(item.key.clone());
        self.tab = usize::from(
            matches!(item.block.map(|b| &b.kind), Some(BlockKind::Tool { tool }) if tool.output.is_some()),
        );
        *self.viewport.get_mut() = DetailViewport {
            follow_tail: matches!(
                item.block.map(|b| &b.kind),
                Some(BlockKind::Reasoning {
                    status: crate::session::MessageStatus::Streaming,
                    ..
                })
            ),
            ..DetailViewport::default()
        };
        self.status = None;
    }
}

fn content(item: &Item<'_>, tab: usize, lang: Lang) -> tool_view::DetailContent {
    match item.block.map(|block| &block.kind) {
        Some(BlockKind::Reasoning { content, .. }) => think_view::content(content),
        Some(BlockKind::Tool { tool }) => tool_view::content(tool, tab, lang),
        _ => tool_view::DetailContent {
            text: item
                .message
                .blocks
                .iter()
                .filter_map(|block| match &block.kind {
                    BlockKind::System { message } => Some(message.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            markdown: false,
        },
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(mut overlay) = app.activity_overlay.take() else {
        return;
    };
    if matches!(key.code, KeyCode::Esc | KeyCode::F(3)) {
        return;
    }
    let Some(session) = app.sessions.iter().find(|s| s.id == overlay.session_id) else {
        return;
    };
    let entries = items(session);
    let selected = entries
        .iter()
        .position(|item| Some(&item.key) == overlay.selected.as_ref())
        .unwrap_or(entries.len().saturating_sub(1));
    let mut next = selected;
    match key.code {
        KeyCode::Tab | KeyCode::BackTab => overlay.detail_focus = !overlay.detail_focus,
        KeyCode::Up if !overlay.detail_focus => next = selected.saturating_sub(1),
        KeyCode::Down if !overlay.detail_focus => {
            next = (selected + 1).min(entries.len().saturating_sub(1))
        }
        KeyCode::PageUp if !overlay.detail_focus => next = selected.saturating_sub(8),
        KeyCode::PageDown if !overlay.detail_focus => {
            next = (selected + 8).min(entries.len().saturating_sub(1))
        }
        KeyCode::Home if !overlay.detail_focus => next = 0,
        KeyCode::End if !overlay.detail_focus => next = entries.len().saturating_sub(1),
        KeyCode::Enter => overlay.detail_focus = true,
        KeyCode::Char('[' | ']') => {
            if entries.get(selected).is_some_and(|item| {
                matches!(item.block.map(|b| &b.kind), Some(BlockKind::Tool { .. }))
            }) {
                overlay.tab = if key.code == KeyCode::Char('[') {
                    (overlay.tab + 2) % 3
                } else {
                    (overlay.tab + 1) % 3
                };
                *overlay.viewport.get_mut() = DetailViewport::default();
                overlay.status = None;
            }
        }
        KeyCode::Char('c' | 'C') => {
            if let Some(item) = entries.get(selected) {
                overlay.status = Some(
                    match crate::clipboard::copy(
                        &content(item, overlay.tab, app.settings.language).text,
                    ) {
                        Ok(()) => match app.settings.language {
                            Lang::Zh => "已复制完整内容",
                            Lang::En => "Full content copied",
                        }
                        .to_owned(),
                        Err(error) => format!("{error}"),
                    },
                );
            }
        }
        _ => {
            let view = overlay.viewport.get_mut();
            match key.code {
                KeyCode::Up => {
                    view.vertical = view.vertical.saturating_sub(1);
                    view.follow_tail = false;
                }
                KeyCode::Down => {
                    view.vertical = (view.vertical + 1).min(view.max_vertical);
                    view.follow_tail = view.vertical == view.max_vertical;
                }
                KeyCode::PageUp => {
                    view.vertical = view.vertical.saturating_sub(view.height.max(1));
                    view.follow_tail = false;
                }
                KeyCode::PageDown => {
                    view.vertical = (view.vertical + view.height.max(1)).min(view.max_vertical);
                    view.follow_tail = view.vertical == view.max_vertical;
                }
                KeyCode::Home => {
                    view.vertical = 0;
                    view.follow_tail = false;
                }
                KeyCode::End => {
                    view.vertical = view.max_vertical;
                    view.follow_tail = true;
                }
                KeyCode::Left => view.horizontal = view.horizontal.saturating_sub(4),
                KeyCode::Right => view.horizontal = (view.horizontal + 4).min(view.max_horizontal),
                _ => {}
            }
        }
    }
    if next != selected
        && let Some(item) = entries.get(next)
    {
        overlay.select(item);
    }
    app.activity_overlay = Some(overlay);
}

pub fn handle_mouse(app: &mut App, target: MouseTarget, kind: MouseEventKind) -> bool {
    let Some(mut overlay) = app.activity_overlay.take() else {
        return false;
    };
    let Some(session) = app.sessions.iter().find(|s| s.id == overlay.session_id) else {
        return false;
    };
    let entries = items(session);
    let handled = match (kind, target) {
        (MouseEventKind::Down(MouseButton::Left), MouseTarget::ActivityClose) => return true,
        (
            MouseEventKind::Down(MouseButton::Left),
            MouseTarget::ActivityBack | MouseTarget::ActivityList,
        ) => {
            overlay.detail_focus = false;
            true
        }
        (MouseEventKind::Down(MouseButton::Left), MouseTarget::ActivityItem(key)) => {
            if let Some(item) = entries.iter().find(|item| item.key == key) {
                overlay.select(item);
                overlay.detail_focus = true;
            }
            true
        }
        (MouseEventKind::Down(MouseButton::Left), MouseTarget::ActivityDetail) => {
            overlay.detail_focus = true;
            true
        }
        (MouseEventKind::Down(MouseButton::Left), MouseTarget::ActivityTab(tab)) => {
            overlay.detail_focus = true;
            overlay.tab = tab;
            *overlay.viewport.get_mut() = DetailViewport::default();
            overlay.status = None;
            true
        }
        (
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown,
            MouseTarget::ActivityList | MouseTarget::ActivityItem(_),
        ) => {
            let selected = entries
                .iter()
                .position(|item| Some(&item.key) == overlay.selected.as_ref())
                .unwrap_or(entries.len().saturating_sub(1));
            let next = if kind == MouseEventKind::ScrollUp {
                selected.saturating_sub(3)
            } else {
                (selected + 3).min(entries.len().saturating_sub(1))
            };
            if next != selected
                && let Some(item) = entries.get(next)
            {
                overlay.select(item);
            }
            overlay.detail_focus = false;
            true
        }
        (
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight,
            MouseTarget::ActivityDetail | MouseTarget::ActivityTab(_),
        ) => {
            overlay.detail_focus = true;
            let view = overlay.viewport.get_mut();
            match kind {
                MouseEventKind::ScrollUp => {
                    view.vertical = view.vertical.saturating_sub(3);
                    view.follow_tail = false;
                }
                MouseEventKind::ScrollDown => {
                    view.vertical = (view.vertical + 3).min(view.max_vertical);
                    view.follow_tail = view.vertical == view.max_vertical;
                }
                MouseEventKind::ScrollLeft => view.horizontal = view.horizontal.saturating_sub(4),
                MouseEventKind::ScrollRight => {
                    view.horizontal = (view.horizontal + 4).min(view.max_horizontal)
                }
                _ => unreachable!(),
            }
            true
        }
        _ => false,
    };
    app.activity_overlay = Some(overlay);
    handled
}

pub fn render(frame: &mut Frame, app: &App) {
    let Some(overlay) = &app.activity_overlay else {
        return;
    };
    let Some(session) = app.sessions.iter().find(|s| s.id == overlay.session_id) else {
        return;
    };
    let lang = app.settings.language;
    let palette = theme::palette(app.settings.theme);
    dim_background(frame, palette);
    let area = popup_rect(frame.area(), 108, 34);
    let wide = frame.area().width >= 90;
    // A wide background glyph can start just outside the popup and cover its border.
    let left = area.x.saturating_sub(1).max(frame.area().x);
    let right = area.right().saturating_add(1).min(frame.area().right());
    Clear.render(
        ratatui::layout::Rect::new(left, area.y, right.saturating_sub(left), area.height),
        frame.buffer_mut(),
    );
    let hint = overlay.status.as_deref().unwrap_or(match lang {
        Lang::Zh => "Tab 列表/详情 · 方向键/翻页 · [ ] 标签 · c 复制 · Esc 返回",
        Lang::En => "Tab List/Detail · Arrows/PgUp/PgDn · [ ] Tabs · c Copy · Esc Close",
    });
    let mut block = Block::bordered()
        .title(match lang {
            Lang::Zh => "过程详情",
            Lang::En => "Process details",
        })
        .title_bottom(Line::from(truncate_width(
            hint,
            area.width.saturating_sub(4) as usize,
        )))
        .style(palette.surface())
        .border_style(palette.border(true));
    let close = match lang {
        Lang::Zh => "[关闭]",
        Lang::En => "[Close]",
    };
    let back = match lang {
        Lang::Zh => "[列表] ",
        Lang::En => "[List] ",
    };
    let controls = if !wide && overlay.detail_focus {
        format!("{back}{close}")
    } else {
        close.to_owned()
    };
    if area.width >= 30 {
        block = block.title_top(Line::from(controls.clone()).right_aligned());
        let mut mouse = app.mouse.borrow_mut();
        mouse.register(
            Rect::new(
                area.right() - 1 - close.width() as u16,
                area.y,
                close.width() as u16,
                1,
            ),
            MouseTarget::ActivityClose,
        );
        if !wide && overlay.detail_focus {
            mouse.register(
                Rect::new(
                    area.right() - 1 - controls.width() as u16,
                    area.y,
                    back.width() as u16,
                    1,
                ),
                MouseTarget::ActivityBack,
            );
        }
    }
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());
    if inner.width < 2 || inner.height == 0 {
        return;
    }
    let entries = items(session);
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(match lang {
                Lang::Zh => "暂无过程记录",
                Lang::En => "No process records",
            }),
            inner,
        );
        return;
    }
    let selected = entries
        .iter()
        .position(|item| Some(&item.key) == overlay.selected.as_ref())
        .unwrap_or(entries.len() - 1);
    let item = &entries[selected];
    let [list_area, detail_area] = if wide {
        Layout::horizontal([Constraint::Length(30), Constraint::Fill(1)]).areas(inner)
    } else {
        [inner, inner]
    };
    if wide || !overlay.detail_focus {
        let list = List::new(
            entries
                .iter()
                .map(|item| {
                    let label = match item.block.map(|b| &b.kind) {
                        Some(BlockKind::Reasoning { status, .. }) => {
                            think_view::label(*status, lang).to_owned()
                        }
                        Some(BlockKind::Tool { tool }) => format!(
                            "{} · {}",
                            tool_view::status_label(tool.status, lang),
                            tool_view::name(tool, lang)
                        ),
                        _ => match lang {
                            Lang::Zh => "运行信息",
                            Lang::En => "Runtime information",
                        }
                        .to_owned(),
                    };
                    let color = match item.block.map(|b| &b.kind) {
                        Some(BlockKind::Tool { tool }) if tool.status == ToolStatus::Failed => {
                            palette.danger
                        }
                        _ => palette.text,
                    };
                    ListItem::new(Line::from(Span::styled(
                        truncate_width(
                            &format!("#{} {label}", item.turn),
                            list_area.width.saturating_sub(4) as usize,
                        ),
                        Style::default().fg(color),
                    )))
                })
                .collect::<Vec<_>>(),
        )
        .block(Block::bordered().border_style(palette.border(!overlay.detail_focus)))
        .highlight_style(palette.selected())
        .highlight_symbol("▸ ");
        let mut state = ListState::default().with_selected(Some(selected));
        frame.render_stateful_widget(list, list_area, &mut state);
        let inner = Block::bordered().inner(list_area);
        let mut mouse = app.mouse.borrow_mut();
        mouse.register(list_area, MouseTarget::ActivityList);
        for (row, item) in entries
            .iter()
            .skip(state.offset())
            .take(inner.height as usize)
            .enumerate()
        {
            mouse.register(
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
                MouseTarget::ActivityItem(item.key.clone()),
            );
        }
    }
    if !wide && !overlay.detail_focus {
        return;
    }
    let title = match item.block.map(|block| &block.kind) {
        Some(BlockKind::Tool { tool }) => format!(
            "{} · {}",
            tool_view::name(tool, lang),
            tool_view::status_label(tool.status, lang)
        ),
        Some(BlockKind::Reasoning { status, .. }) => think_view::label(*status, lang).to_owned(),
        _ => match lang {
            Lang::Zh => "运行信息",
            Lang::En => "Runtime information",
        }
        .to_owned(),
    };
    let panel = Block::bordered()
        .border_style(palette.border(overlay.detail_focus))
        .title(truncate_width(
            &title,
            detail_area.width.saturating_sub(2) as usize,
        ));
    let body = panel.inner(detail_area);
    frame.render_widget(panel, detail_area);
    app.mouse
        .borrow_mut()
        .register(detail_area, MouseTarget::ActivityDetail);
    let is_tool = matches!(item.block.map(|b| &b.kind), Some(BlockKind::Tool { .. }));
    let [tabs_area, legacy_area, text_area] = Layout::vertical([
        Constraint::Length(u16::from(is_tool)),
        Constraint::Length(u16::from(item.message.legacy_order)),
        Constraint::Fill(1),
    ])
    .areas(body);
    if is_tool {
        let labels = match lang {
            Lang::Zh => ["参数", "结果", "原始"],
            Lang::En => ["Arguments", "Result", "Raw"],
        };
        frame.render_widget(
            Tabs::new(labels)
                .padding(" ", " ")
                .divider("│")
                .select(overlay.tab)
                .highlight_style(palette.selected()),
            tabs_area,
        );
        let mut x = tabs_area.x;
        let mut mouse = app.mouse.borrow_mut();
        for (tab, label) in labels.iter().enumerate() {
            let width = (label.width() as u16 + 2).min(tabs_area.right().saturating_sub(x));
            mouse.register(
                Rect::new(x, tabs_area.y, width, tabs_area.height),
                MouseTarget::ActivityTab(tab),
            );
            x = x.saturating_add(width + 1);
        }
    }
    if item.message.legacy_order {
        frame.render_widget(
            Paragraph::new(match lang {
                Lang::Zh => "历史记录未保存正文与过程的交错位置",
                Lang::En => "Legacy history has no text/process ordering",
            })
            .style(palette.muted),
            legacy_area,
        );
    }
    if text_area.width < 2 || text_area.height == 0 {
        return;
    }
    let width = text_area.width.saturating_sub(1) as usize;
    let revision = item
        .block
        .map_or(item.message.blocks.len() as u64, |block| block.revision);
    let mut view = overlay.viewport.borrow_mut();
    let valid = view.cache.as_ref().is_some_and(|cache| {
        cache.key == item.key
            && cache.revision == revision
            && cache.tab == overlay.tab
            && cache.width == width
            && cache.theme == app.settings.theme
            && cache.lang == lang
    });
    if !valid {
        let source = content(item, overlay.tab, lang);
        let lines = if source.markdown {
            markdown::render_markdown(&source.text, width, palette, app.settings.theme)
        } else {
            source
                .text
                .lines()
                .map(|line| Line::from(line.to_owned()))
                .collect()
        };
        view.cache = Some(DetailCache {
            key: item.key.clone(),
            revision,
            tab: overlay.tab,
            width,
            theme: app.settings.theme,
            lang,
            lines,
        });
    }
    let lines = &view.cache.as_ref().expect("detail layout").lines;
    let count = lines.len();
    let max_horizontal = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .saturating_sub(width.saturating_sub(1));
    view.max_horizontal = max_horizontal;
    view.horizontal = view.horizontal.min(max_horizontal);
    view.height = text_area.height as usize;
    view.max_vertical = count.saturating_sub(view.height);
    view.vertical = if view.follow_tail {
        view.max_vertical
    } else {
        view.vertical.min(view.max_vertical)
    };
    let visible = view
        .cache
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .skip(view.vertical)
        .take(view.height)
        .map(|line| {
            // Clip by terminal columns, including wide and combining characters.
            markdown::crop_line(line, view.horizontal, width, palette)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(visible).style(palette.surface()),
        ratatui::layout::Rect {
            width: width as u16,
            ..text_area
        },
    );
    if count > view.height {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            text_area,
            &mut ScrollbarState::new(view.max_vertical)
                .position(view.vertical)
                .viewport_content_length(view.height),
        );
    }
}
