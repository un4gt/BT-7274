//! Full-screen-friendly fenced code block viewer with independent x/y scrolling.

use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Widget},
};

use crate::{app::App, i18n::Lang};

use super::{dim_background, markdown, popup_rect, theme};

pub fn render(frame: &mut Frame, app: &App) {
    let Some(state) = &app.code_overlay else {
        return;
    };
    let palette = theme::palette(app.settings.theme);
    dim_background(frame, palette);
    let area = popup_rect(frame.area(), 108, 34);
    Clear.render(area, frame.buffer_mut());
    let selected = state.selected.min(state.blocks.len().saturating_sub(1));
    let Some(code) = state.blocks.get(selected) else {
        return;
    };
    let language = if code.language.trim().is_empty() {
        "text"
    } else {
        code.language.trim()
    };
    let title = match app.settings.language {
        Lang::Zh => format!(
            "代码块 {}/{} · {language}",
            selected + 1,
            state.blocks.len()
        ),
        Lang::En => format!("Code {}/{} · {language}", selected + 1, state.blocks.len()),
    };
    let bottom = state
        .status
        .as_deref()
        .unwrap_or(match app.settings.language {
            Lang::Zh => "[ / ] 切换 · 方向键滚动 · c 复制 · Esc 关闭",
            Lang::En => "[ / ] Switch · Arrows Scroll · c Copy · Esc Close",
        });
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(bottom, palette.muted)).right_aligned())
        .border_style(palette.border(true))
        .style(palette.surface());
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let line_count = code.content.lines().count().max(1);
    let digits = line_count.to_string().len().min(6);
    let prefix_width = digits.saturating_add(2);
    let content_width = (inner.width as usize).saturating_sub(prefix_width);
    let highlighted = markdown::render_code_block(
        code,
        content_width,
        state.horizontal_scroll,
        palette,
        app.settings.theme,
        false,
    );
    let visible = inner.height as usize;
    let start = (state.vertical_scroll as usize).min(
        highlighted
            .len()
            .saturating_sub(visible.min(highlighted.len())),
    );
    let lines = highlighted
        .into_iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, line)| {
            Line::from(
                std::iter::once(Span::styled(
                    format!("{:>digits$} │", index + 1),
                    Style::default().fg(palette.muted),
                ))
                .chain(line.spans)
                .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines)
        .style(palette.surface())
        .render(inner, frame.buffer_mut());
}
