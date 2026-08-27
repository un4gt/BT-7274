//! 结构化运行时错误详情：分类、恢复建议、request id 与脱敏诊断链。

use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Wrap},
};

use crate::{app::App, i18n::Lang};

use super::{dim_background, popup_rect, theme};

pub fn render(frame: &mut Frame, app: &App) {
    let Some(error) = app.detailed_error() else {
        return;
    };
    let palette = theme::palette(app.settings.theme);
    let lang = app.settings.language;
    dim_background(frame, palette);
    let area = popup_rect(frame.area(), 88, 22);
    if area.width < 4 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);

    let (title, hint, category, summary, recovery, request_id, retry_after, diagnostics) =
        match lang {
            Lang::Zh => (
                "错误详情（已脱敏）",
                "Esc/F2 返回 · ↑↓/PgUp/PgDn 滚动",
                "分类",
                "摘要",
                "恢复建议",
                "Request ID",
                "Retry-After",
                "诊断链",
            ),
            Lang::En => (
                "Error Details (Redacted)",
                "Esc/F2 Back · ↑↓/PgUp/PgDn Scroll",
                "Category",
                "Summary",
                "Recovery",
                "Request ID",
                "Retry-After",
                "Diagnostic chain",
            ),
        };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(palette.border(true))
        .style(palette.surface())
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(palette.danger)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(hint).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        field_line(
            category,
            lang.runtime_error_kind_label(error.kind),
            palette.danger,
            palette.text,
        ),
        field_line(summary, &error.summary, palette.primary, palette.text),
        field_line(
            recovery,
            lang.runtime_error_recovery(error.kind),
            palette.success,
            palette.text,
        ),
    ];
    if let Some(value) = &error.request_id {
        lines.push(field_line(request_id, value, palette.accent, palette.text));
    }
    if let Some(milliseconds) = error.retry_after_ms {
        lines.push(field_line(
            retry_after,
            &format!("{milliseconds} ms"),
            palette.warning,
            palette.text,
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{diagnostics}:"),
        Style::default()
            .fg(palette.primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.extend(error.detail.lines().map(|line| {
        Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(palette.muted),
        ))
    }));

    let scroll = app
        .error_detail
        .as_ref()
        .map(|detail| detail.scroll)
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(lines)
            .style(palette.surface())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

fn field_line(
    label: &str,
    value: &str,
    label_color: ratatui::style::Color,
    value_color: ratatui::style::Color,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default()
                .fg(label_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}
