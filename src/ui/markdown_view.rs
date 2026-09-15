//! Layout cache for answer blocks. Animation ticks do not reparse unchanged Markdown.
use super::{markdown, theme::Palette};
use crate::{config::Theme, session::MessageBlock};
use ratatui::text::Line;
use std::collections::HashMap;

#[derive(Default, Debug)]
pub struct MarkdownView {
    entries: HashMap<(String, u64), Entry>,
}

#[derive(Debug)]
struct Entry {
    revision: u64,
    width: usize,
    theme: Theme,
    lines: Vec<Line<'static>>,
}

impl MarkdownView {
    pub fn lines(
        &mut self,
        message: &str,
        block: &MessageBlock,
        text: &str,
        width: usize,
        palette: Palette,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let key = (message.to_owned(), block.id);
        if let Some(entry) = self.entries.get(&key)
            && entry.revision == block.revision
            && entry.width == width
            && entry.theme == theme
        {
            return entry.lines.clone();
        }
        let lines = markdown::render_markdown(text, width, palette, theme);
        // Keep the cache bounded when browsing very long histories.
        if self.entries.len() >= 512 {
            self.entries.clear();
        }
        self.entries.insert(
            key,
            Entry {
                revision: block.revision,
                width,
                theme,
                lines: lines.clone(),
            },
        );
        lines
    }
}
