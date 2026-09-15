//! Streaming-safe Markdown rendering and fenced-code extraction for the chat viewport.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options as ParserOptions, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use tui_markdown::{
    AlertKind, BuiltinCodeTheme, Options as MarkdownOptions, StyleSheet, from_str_with_options,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{config::Theme, text::truncate_width};

use super::theme::Palette;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownCodeBlock {
    pub language: String,
    pub content: String,
}

#[derive(Debug, Clone)]
struct ParsedCodeBlock {
    block: MarkdownCodeBlock,
    source: Range<usize>,
}

#[derive(Debug, Clone, Copy)]
struct MarkdownStyles {
    palette: Palette,
    hide_code_fence: bool,
}

impl StyleSheet for MarkdownStyles {
    fn heading(&self, level: u8) -> Style {
        let color = if level <= 2 {
            self.palette.primary
        } else {
            self.palette.accent
        };
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    }

    fn code(&self) -> Style {
        Style::default()
            .fg(self.palette.text)
            .bg(self.palette.panel)
    }

    fn link(&self) -> Style {
        Style::default()
            .fg(self.palette.primary)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::default()
            .fg(self.palette.muted)
            .add_modifier(Modifier::ITALIC)
    }

    fn heading_meta(&self) -> Style {
        Style::default().fg(self.palette.muted)
    }

    fn metadata_block(&self) -> Style {
        Style::default().fg(self.palette.warning)
    }

    fn code_block_fence(&self) -> &str {
        if self.hide_code_fence { "" } else { "```" }
    }

    fn html(&self) -> Style {
        Style::default()
            .fg(self.palette.muted)
            .add_modifier(Modifier::DIM)
    }

    fn math_inline(&self) -> Style {
        Style::default()
            .fg(self.palette.accent)
            .add_modifier(Modifier::ITALIC)
    }

    fn math_display(&self) -> Style {
        Style::default().fg(self.palette.accent)
    }

    fn alert(&self, kind: AlertKind) -> Style {
        let color = match kind {
            AlertKind::Note => self.palette.primary,
            AlertKind::Tip => self.palette.success,
            AlertKind::Important => self.palette.accent,
            AlertKind::Warning => self.palette.warning,
            AlertKind::Caution => self.palette.danger,
        };
        Style::default().fg(color)
    }

    fn alert_icon(&self, kind: AlertKind) -> &str {
        match kind {
            AlertKind::Note => "i",
            AlertKind::Tip => "+",
            AlertKind::Important => "*",
            AlertKind::Warning | AlertKind::Caution => "!",
        }
    }

    fn table_header(&self) -> Style {
        Style::default()
            .fg(self.palette.primary)
            .add_modifier(Modifier::BOLD)
    }

    fn table_cell(&self) -> Style {
        Style::default().fg(self.palette.text)
    }

    fn table_border(&self) -> Style {
        Style::default().fg(self.palette.border)
    }

    fn image_alt(&self) -> Style {
        Style::default()
            .fg(self.palette.muted)
            .add_modifier(Modifier::ITALIC)
    }
}

pub(crate) fn extract_code_blocks(markdown: &str) -> Vec<MarkdownCodeBlock> {
    parse_code_blocks(markdown)
        .into_iter()
        .map(|parsed| parsed.block)
        .collect()
}

pub(crate) fn render_markdown(
    markdown: &str,
    width: usize,
    palette: Palette,
    theme: Theme,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let parsed = parse_code_blocks(markdown);
    let token_prefix = unique_code_token_prefix(markdown);
    let mut source = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    for (index, code) in parsed.iter().enumerate() {
        source.push_str(&markdown[cursor..code.source.start]);
        if !source.ends_with('\n') {
            source.push('\n');
        }
        source.push_str(&code_token(&token_prefix, index));
        source.push('\n');
        cursor = code.source.end;
    }
    source.push_str(&markdown[cursor..]);

    let styles = MarkdownStyles {
        palette,
        hide_code_fence: false,
    };
    let options = themed_options(styles, theme);
    let rendered = from_str_with_options(&source, &options);
    let mut lines = Vec::new();
    for line in rendered.lines {
        let plain = line.to_string();
        let code_index = parsed.iter().enumerate().find_map(|(index, _)| {
            plain
                .contains(&code_token(&token_prefix, index))
                .then_some(index)
        });
        if let Some(index) = code_index {
            lines.extend(render_code_block(
                &parsed[index].block,
                width,
                0,
                palette,
                theme,
                true,
            ));
        } else if is_fixed_width_structure(&plain) {
            lines.push(crop_line(&own_line(line), 0, width, palette));
        } else {
            lines.extend(wrap_styled_line(&own_line(line), width));
        }
    }
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

pub(crate) fn render_code_block(
    block: &MarkdownCodeBlock,
    width: usize,
    horizontal_offset: usize,
    palette: Palette,
    theme: Theme,
    frame: bool,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let frame = frame && width >= 4;
    let styles = MarkdownStyles {
        palette,
        hide_code_fence: true,
    };
    let options = themed_options(styles, theme);
    let fence_len = longest_backtick_run(&block.content)
        .saturating_add(1)
        .max(3);
    let fence = "`".repeat(fence_len);
    let fenced = format!("{fence}{}\n{}\n{fence}", block.language, block.content);
    let highlighted = from_str_with_options(&fenced, &options);
    let mut lines = Vec::new();
    if frame {
        let language = if block.language.trim().is_empty() {
            "text".to_owned()
        } else {
            truncate_width(block.language.trim(), width.saturating_sub(4))
        };
        lines.push(Line::from(vec![
            Span::styled("┌─ ", Style::default().fg(palette.border)),
            Span::styled(
                language,
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    for line in highlighted.lines {
        let line = own_line(line);
        if frame {
            let content_width = width.saturating_sub(2);
            lines.push(Line::from(
                std::iter::once(Span::styled("│ ", Style::default().fg(palette.border)))
                    .chain(crop_line(&line, horizontal_offset, content_width, palette).spans)
                    .collect::<Vec<_>>(),
            ));
        } else {
            lines.push(crop_line(&line, horizontal_offset, width, palette));
        }
    }
    if lines.is_empty() {
        lines.push(if frame {
            Line::from(Span::styled("│ ", Style::default().fg(palette.border)))
        } else {
            Line::default()
        });
    }
    if frame {
        lines.push(Line::from(Span::styled(
            "└─",
            Style::default().fg(palette.border),
        )));
    }
    lines
}

fn themed_options(styles: MarkdownStyles, theme: Theme) -> MarkdownOptions<MarkdownStyles> {
    let code_theme = match theme {
        Theme::Paper => BuiltinCodeTheme::InspiredGitHub,
        Theme::Vanguard => BuiltinCodeTheme::SolarizedDark,
        Theme::Carbon => BuiltinCodeTheme::Base16OceanDark,
    };
    MarkdownOptions::new(styles).code_theme(code_theme)
}

fn parse_code_blocks(markdown: &str) -> Vec<ParsedCodeBlock> {
    let mut options = ParserOptions::empty();
    options.insert(ParserOptions::ENABLE_TABLES);
    options.insert(ParserOptions::ENABLE_STRIKETHROUGH);
    options.insert(ParserOptions::ENABLE_TASKLISTS);
    options.insert(ParserOptions::ENABLE_GFM);
    let mut parsed = Vec::new();
    let mut active: Option<(usize, String, String)> = None;
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                active = Some((range.start, language, String::new()));
            }
            Event::Text(text) if active.is_some() => {
                if let Some((_, _, content)) = &mut active {
                    content.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak if active.is_some() => {
                if let Some((_, _, content)) = &mut active {
                    content.push('\n');
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, language, content)) = active.take() {
                    parsed.push(ParsedCodeBlock {
                        block: MarkdownCodeBlock {
                            language,
                            content: content.trim_end_matches('\n').to_owned(),
                        },
                        source: start..range.end,
                    });
                }
            }
            _ => {}
        }
    }
    if let Some((start, language, content)) = active {
        parsed.push(ParsedCodeBlock {
            block: MarkdownCodeBlock {
                language,
                content: content.trim_end_matches('\n').to_owned(),
            },
            source: start..markdown.len(),
        });
    }
    parsed
}

fn unique_code_token_prefix(markdown: &str) -> String {
    let mut prefix = "BT7274CODEBLOCKTOKEN".to_owned();
    while markdown.contains(&prefix) {
        prefix.push('X');
    }
    prefix
}

fn code_token(prefix: &str, index: usize) -> String {
    format!("{prefix}{index}END")
}

fn is_fixed_width_structure(line: &str) -> bool {
    line.chars()
        .any(|character| "┌┬┐├┼┤└┴┘│".contains(character))
}

fn own_line(line: Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect(),
    }
}

pub(crate) fn wrap_styled_line(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = vec![Line::default().style(line.style)];
    let mut used = 0usize;
    for span in &line.spans {
        for grapheme in span.content.graphemes(true) {
            let mut text = grapheme;
            let mut grapheme_width = text.width();
            if grapheme_width > width {
                text = "…";
                grapheme_width = 1;
            }
            if used > 0 && used.saturating_add(grapheme_width) > width {
                rows.push(Line::default().style(line.style));
                used = 0;
            }
            push_styled(
                &mut rows.last_mut().expect("row exists").spans,
                text,
                span.style,
            );
            used = used.saturating_add(grapheme_width);
            if used >= width {
                rows.push(Line::default().style(line.style));
                used = 0;
            }
        }
    }
    if rows.len() > 1 && rows.last().is_some_and(|row| row.spans.is_empty()) {
        rows.pop();
    }
    rows
}

pub(crate) fn crop_line(
    line: &Line<'_>,
    horizontal_offset: usize,
    width: usize,
    palette: Palette,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let total = line.width();
    let show_left = horizontal_offset > 0;
    let remaining_after_left = width.saturating_sub(usize::from(show_left));
    let show_right =
        remaining_after_left > 0 && total > horizontal_offset.saturating_add(remaining_after_left);
    let content_width = width
        .saturating_sub(usize::from(show_left))
        .saturating_sub(usize::from(show_right));
    let mut spans = Vec::new();
    if show_left {
        spans.push(Span::styled("‹", Style::default().fg(palette.muted)));
    }
    let mut source_column = 0usize;
    let mut output_width = 0usize;
    'spans: for span in &line.spans {
        for grapheme in span.content.graphemes(true) {
            let grapheme_width = grapheme.width();
            let end = source_column.saturating_add(grapheme_width);
            if end <= horizontal_offset {
                source_column = end;
                continue;
            }
            if source_column < horizontal_offset {
                source_column = end;
                continue;
            }
            if output_width.saturating_add(grapheme_width) > content_width {
                break 'spans;
            }
            push_styled(&mut spans, grapheme, span.style);
            source_column = end;
            output_width = output_width.saturating_add(grapheme_width);
        }
    }
    if show_right {
        spans.push(Span::styled("›", Style::default().fg(palette.muted)));
    }
    Line {
        style: line.style,
        alignment: line.alignment,
        spans,
    }
}

fn push_styled(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(text);
        return;
    }
    spans.push(Span::styled(text.to_owned(), style));
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme;

    #[test]
    fn markdown_covers_required_blocks_and_inline_styles() {
        let source = "# Heading\n\n**bold** and `code`\n\n- item\n\n> quote\n\n| A | B |\n|---|---|\n| 你 | ok |\n\n```rust\nfn main() { println!(\"hi\"); }\n```";
        let palette = theme::palette(Theme::Carbon);
        let lines = render_markdown(source, 32, palette, Theme::Carbon);
        let plain = lines.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("Heading")));
        assert!(plain.iter().any(|line| line.contains("bold")));
        assert!(plain.iter().any(|line| line.contains("quote")));
        assert!(plain.iter().any(|line| line.contains('┌')));
        assert!(plain.iter().any(|line| line.contains("fn main")));
        assert!(lines.iter().all(|line| line.width() <= 32));
    }

    #[test]
    fn incomplete_streaming_markdown_is_stable_and_extracts_code() {
        let palette = theme::palette(Theme::Vanguard);
        for source in [
            "**unfinished",
            "- item\n  - nested",
            "| A | B |\n|---|",
            "```rust\nfn main() {",
        ] {
            assert!(!render_markdown(source, 12, palette, Theme::Vanguard).is_empty());
        }
        let blocks = extract_code_blocks("before\n```rust\nlet x = 1;");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, "rust");
        assert!(blocks[0].content.contains("let x"));
    }

    #[test]
    fn code_lines_crop_horizontally_without_wrapping() {
        let palette = theme::palette(Theme::Paper);
        let block = MarkdownCodeBlock {
            language: "rust".to_owned(),
            content: "let very_long_identifier = \"你好世界\";".to_owned(),
        };
        let lines = render_code_block(&block, 14, 8, palette, Theme::Paper, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].width() <= 14);
        assert!(lines[0].to_string().starts_with('‹'));
        assert!(lines[0].to_string().ends_with('›'));
    }

    #[test]
    fn user_text_that_looks_like_an_internal_token_is_preserved() {
        let source = "BT7274CODEBLOCKTOKEN0END\n\n```text\nactual code\n```";
        let palette = theme::palette(Theme::Carbon);
        let rendered = render_markdown(source, 40, palette, Theme::Carbon)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("BT7274CODEBLOCKTOKEN0END"))
        );
        assert!(rendered.iter().any(|line| line.contains("actual code")));
    }
}
