//! Footer：模型、上下文 token、生成速度与状态。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::app::App;
use crate::config::ProxyMode;
use crate::ui::{spinner_frame, theme::Palette};

pub fn render(app: &App, area: Rect, buf: &mut Buffer, palette: Palette) {
    let texts = app.settings.language.texts();
    let selection = app.displayed_selection();
    let provider = app.provider_for_selection(&selection);
    let context = app.context_budget();
    let block = Block::bordered()
        .border_style(palette.border(false))
        .style(palette.panel());
    let inner = block.inner(area);
    block.render(area, buf);

    let speed_value = app
        .stream
        .as_ref()
        .filter(|state| state.session_id == app.open_session().id)
        .and_then(|state| state.tokens_per_sec())
        .map(|v| format!("{v:.0} tok/s"))
        .unwrap_or_else(|| "— tok/s".to_owned());

    let mut spans = vec![
        Span::styled(
            format!(" {} [{}]", selection.model, provider.api_kind.short_label()),
            Style::default()
                .fg(palette.primary)
                .bg(palette.panel)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} {}", texts.footer_ctx, format_context(&context)),
            if context.overflowed()
                || context.usage_percent().is_some_and(|percent| {
                    percent >= app.settings.context.auto_compact_threshold_percent
                })
            {
                Style::default().fg(palette.warning).bg(palette.panel)
            } else {
                palette.panel()
            },
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} {speed_value}", texts.footer_speed),
            palette.panel(),
        ),
    ];

    if app.has_next_turn_model() {
        spans.push(Span::styled(
            " 1×",
            Style::default()
                .fg(palette.accent)
                .bg(palette.panel)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if app.settings.proxy.mode != ProxyMode::Disabled {
        spans.push(Span::styled(" │ ", palette.muted));
        spans.push(Span::styled(
            format!("Proxy {}", texts.proxy_mode_label(app.settings.proxy.mode)),
            Style::default()
                .fg(palette.accent)
                .bg(palette.panel)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if app.generating() {
        let frame = spinner_frame(app.ticks);
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            format!("{frame} {}", texts.footer_generating),
            Style::default().fg(palette.warning).bg(palette.panel),
        ));
    } else if app
        .stream
        .as_ref()
        .is_some_and(|state| state.aborted && state.session_id == app.open_session().id)
    {
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            texts.footer_aborted,
            Style::default().fg(palette.muted).bg(palette.panel),
        ));
    }
    spans.push(Span::raw(" │ "));
    spans.push(Span::styled(
        if app.generating() {
            app.settings.language.footer_task_hint()
        } else {
            texts.footer_quit_hint
        },
        Style::default().fg(palette.muted).bg(palette.panel),
    ));

    Paragraph::new(Line::from(spans))
        .style(palette.panel())
        .render(inner, buf);
}

/// token 数的紧凑展示：1234 -> 1.2k。
fn format_tokens(tokens: usize) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

fn format_context(context: &crate::runtime::context::ContextBudget) -> String {
    let input = format_tokens(context.input_tokens);
    let reserved = format_tokens(context.reserved_output);
    match context.context_window {
        Some(window) => format!("~{input}+{reserved}/{}", format_tokens(window)),
        None => format!("~{input}+{reserved}/?"),
    }
}
