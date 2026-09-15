//! Content：上部分聊天消息区（底部对齐、可滚动），下部分输入框（带光标）。

use super::{mouse::MouseTarget, theme::Palette, transcript::render_messages};
#[cfg(test)]
use super::{spinner_frame, transcript::build_message_window};
use crate::{
    app::{App, Focus},
    i18n::Lang,
    text::truncate_width,
};
#[cfg(test)]
use ratatui::buffer::Buffer;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Widget},
};

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
    app.mouse.borrow_mut().register(inner, MouseTarget::Input);

    let viewport = app
        .editor
        .viewport(inner.width as usize, inner.height as usize);
    let lines = viewport
        .rows
        .iter()
        .map(|row| {
            Line::from(
                row.cells
                    .iter()
                    .map(|cell| {
                        Span::styled(
                            cell.text.as_str(),
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
    app.sparkle.render(
        inner,
        &viewport,
        focused,
        frame.buffer_mut(),
        palette,
        std::time::Instant::now(),
    );
    if focused && inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position(Position::new(
            inner.x + (viewport.cursor_column as u16).min(inner.width - 1),
            inner.y + (viewport.cursor_row as u16).min(inner.height - 1),
        ));
    }
}

pub(crate) fn input_is_focused(app: &App) -> bool {
    app.focus == Focus::Input
        && app.modal.is_none()
        && app.picker.is_none()
        && app.conversation_overlay.is_none()
        && app.code_overlay.is_none()
        && app.activity_overlay.is_none()
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
        message.append_text(
            "# 中文标题\n\nemoji 👩‍💻 · e\u{301} · 全角，。\n\nsupercalifragilisticexpialidocious",
        );
        session.messages.push(message);
        let app = App::new(settings, vec![session]);
        let palette = theme::palette(app.settings.theme);

        for width in [4, 7, 16, 40] {
            let window = build_message_window(&app, width, 24, 0, palette);
            assert!(window.lines.iter().all(|line| line.width() <= width));
        }
    }

    #[tokio::test]
    async fn empty_streaming_reply_uses_an_animated_indicator_without_waiting_copy() {
        let settings = Settings::default();
        let mut session = Session::new();
        session
            .messages
            .push(Message::assistant_streaming(settings.default_selection()));
        let mut app = App::new(settings, vec![session]);
        let palette = theme::palette(app.settings.theme);

        let first = build_message_window(&app, 40, 8, 0, palette)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(first.contains(spinner_frame(0)));
        assert!(!first.contains("等待首个响应片段"));

        app.ticks = 1;
        let second = build_message_window(&app, 40, 8, 0, palette)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(second.contains(spinner_frame(1)));
        assert_ne!(spinner_frame(0), spinner_frame(1));
    }

    #[tokio::test]
    async fn overflowing_messages_render_a_scrollbar_that_tracks_page_scroll() {
        let settings = Settings::default();
        let mut session = Session::new();
        session.messages = (0..30)
            .map(|index| Message::user(format!("message {index}"), None))
            .collect();
        let app = App::new(settings, vec![session]);
        let palette = theme::palette(app.settings.theme);

        let bottom = build_message_window(&app, 30, 8, 0, palette)
            .scrollbar
            .unwrap();
        let scrolled = build_message_window(&app, 30, 8, 8, palette)
            .scrollbar
            .unwrap();
        assert!(scrolled.position < bottom.position);

        let area = Rect::new(0, 0, 32, 10);
        let mut buffer = Buffer::empty(area);
        render_messages(&app, area, &mut buffer, palette);
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains('█'));
        assert!(rendered.contains('║'));
    }

    #[tokio::test]
    async fn process_details_leave_the_transcript_collapsed() {
        let settings = Settings::default();
        let mut session = Session::new();
        let mut message = Message::assistant_streaming(settings.default_selection());
        message.append_reasoning("hidden reasoning body");
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
            serde_json::json!({"temperature":24,"payload":"full-tool-result-body"}),
            false,
        );
        message.append_text("Visible answer");
        message.finish(MessageStatus::Completed);
        session.messages.push(message);
        let mut app = App::new(settings, vec![session]);
        let palette = theme::palette(app.settings.theme);

        let narrow = build_message_window(&app, 24, 30, 0, palette);
        assert!(narrow.lines.iter().all(|line| line.width() <= 24));
        let rendered = build_message_window(&app, 80, 30, 0, palette)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("1 项工具"));
        assert!(rendered.contains('▸'));
        assert!(!rendered.contains("洛杉矶天气"));
        assert!(!rendered.contains("full-tool-result-body"));
        assert!(!rendered.contains("hidden reasoning body"));
        assert!(rendered.find('▸').unwrap() < rendered.find("Visible answer").unwrap());

        app.handle_key_events(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::F(3),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        let expanded = build_message_window(&app, 80, 30, 0, palette)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(app.activity_overlay.is_some());
        assert_eq!(expanded, rendered);
        assert!(!expanded.contains("full-tool-result-body"));

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        app.editor.set_text("keep my draft");
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &app)).unwrap();
        let details = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(details.contains("full-tool-result-body"));
        assert!(!terminal.backend().cursor_visible());
        app.handle_key_events(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.activity_overlay.as_ref().unwrap().tab, 0);
        terminal.draw(|frame| crate::ui::draw(frame, &app)).unwrap();
        // A terminal does not display the trailing cells covered by a wide glyph.
        let mut parameters = String::new();
        for row in terminal.backend().buffer().content.chunks(120) {
            let mut column = 0;
            while column < row.len() {
                let symbol = row[column].symbol();
                parameters.push_str(symbol);
                column += unicode_width::UnicodeWidthStr::width(symbol).max(1);
            }
            parameters.push('\n');
        }
        assert!(parameters.contains("洛杉矶天气"), "{parameters}");
        // A compact terminal has separate list/detail pages; both remain safe down to one cell.
        app.handle_key_events(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        for (width, height) in [(70, 24), (30, 8), (10, 3), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| crate::ui::draw(frame, &app)).unwrap();
        }
        app.handle_key_events(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.editor.text(), "keep my draft");
        assert!(input_is_focused(&app));

        app.sessions[0].messages[0].append_text(&"\nmore answer".repeat(60));
        build_message_window(&app, 48, 8, 0, palette);
        app.chat_view.borrow_mut().scroll_by(-16);
        let before = build_message_window(&app, 48, 8, 0, palette).lines;
        app.sessions[0].messages[0].append_text(&"\nnew stream output".repeat(25));
        let after = build_message_window(&app, 48, 8, 0, palette).lines;
        assert_eq!(
            before, after,
            "new output must not displace history being read"
        );
    }
}
