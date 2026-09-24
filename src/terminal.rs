//! 将画面与光标作为完整的一帧提交，避免动画暴露重绘的中间状态。

use std::{
    io::{self, Write},
    num::NonZeroU16,
};

use crossterm::{
    ExecutableCommand, QueueableCommand,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    buffer::{CellDiffOption, CellWidth},
};

pub(crate) fn draw<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    render: impl FnOnce(&mut Frame),
) -> io::Result<()> {
    terminal.backend_mut().queue(BeginSynchronizedUpdate)?;
    // 由 Ratatui 根据焦点决定光标显隐；不在每帧开始时额外隐藏光标。
    let draw_result = terminal
        .draw(|frame| {
            render(frame);
            // ratatui-core 0.1.2 会在 VS16 emoji 后追加尾格清除；ratatui-crossterm 0.1.2
            // 连续输出时把这个空格写到字形之后，导致文字、边框和滚动条偏移。
            // 显式保留字形宽度，使差异输出跳过已被 emoji 覆盖的尾格。
            for cell in &mut frame.buffer_mut().content {
                if cell.diff_option == CellDiffOption::None
                    && cell.symbol().contains('\u{fe0f}')
                    && cell.cell_width() > 1
                    && let Some(width) = NonZeroU16::new(cell.cell_width())
                {
                    cell.set_diff_option(CellDiffOption::ForcedWidth(width));
                }
            }
        })
        .map(|_| ());
    // 绘制返回错误时也要结束同步更新，并优先保留原始绘制错误。
    let end_result = terminal
        .backend_mut()
        .execute(EndSynchronizedUpdate)
        .map(|_| ());
    draw_result.and(end_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    use ratatui::{TerminalOptions, Viewport, layout::Rect, widgets::Paragraph};

    const BEGIN: &str = "\x1b[?2026h";
    const END: &str = "\x1b[?2026l";
    const HIDE: &str = "\x1b[?25l";
    const SHOW: &str = "\x1b[?25h";

    #[derive(Default)]
    struct Output {
        bytes: Vec<u8>,
        flushes: Vec<usize>,
        flush_errors: VecDeque<io::ErrorKind>,
    }

    #[derive(Clone, Default)]
    struct RecordingWriter(Rc<RefCell<Output>>);

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut output = self.0.borrow_mut();
            let offset = output.bytes.len();
            output.flushes.push(offset);
            if let Some(kind) = output.flush_errors.pop_front() {
                return Err(io::Error::from(kind));
            }
            Ok(())
        }
    }

    fn terminal(writer: RecordingWriter) -> Terminal<CrosstermBackend<RecordingWriter>> {
        Terminal::with_options(
            CrosstermBackend::new(writer),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 20, 3)),
            },
        )
        .unwrap()
    }

    #[test]
    fn redraws_commit_content_and_cursor_together() {
        let writer = RecordingWriter::default();
        let mut terminal = terminal(writer.clone());
        // 连续动画帧、隐藏光标的覆盖层、返回输入；也覆盖无内容变化的重绘。
        for (text, focused) in [("a", true), ("b", true), ("b", false), ("b", true)] {
            draw(&mut terminal, |frame| {
                frame.render_widget(Paragraph::new(text), frame.area());
                if focused {
                    frame.set_cursor_position((5, 1));
                }
            })
            .unwrap();

            let output = std::mem::take(&mut *writer.0.borrow_mut());
            let bytes = String::from_utf8(output.bytes).unwrap();
            assert!(bytes.starts_with(BEGIN));
            assert!(bytes.ends_with(END));
            assert_eq!(bytes.matches(BEGIN).count(), 1);
            assert_eq!(bytes.matches(END).count(), 1);
            // 即使后端在帧内多次 flush，结束标记也只能在最后一次提交。
            assert_eq!(output.flushes.last(), Some(&bytes.len()));
            for offset in &output.flushes[..output.flushes.len() - 1] {
                assert!(*offset <= bytes.len() - END.len());
            }
            if focused {
                assert!(!bytes.contains(HIDE), "input cursor hidden during redraw");
                assert!(bytes.contains(SHOW));
                assert!(bytes.ends_with(&format!("\x1b[2;6H{END}")));
            } else {
                assert!(bytes.contains(HIDE));
                assert!(!bytes.contains(SHOW));
            }
        }
    }

    #[test]
    fn emoji_redraws_preserve_text_and_scrollbar_columns() {
        use ratatui::style::{Color, Style};
        use unicode_width::UnicodeWidthStr;

        let writer = RecordingWriter::default();
        let mut terminal = terminal(writer.clone());
        let mut screen = vt100::Parser::new(3, 20, 0);
        // VS16 emoji replace non-blank cells, then move and disappear as history scrolls.
        // TestBackend alone cannot detect cursor drift in the actual ANSI output.
        for text in [
            "0123456789abcdefgh",
            "a☕\u{fe0f} 文本",
            "☕\u{fe0f}☕\u{fe0f} 中文",
            "中文 ☕\u{fe0f}",
            "正常文本",
            "",
        ] {
            for _ in 0..2 {
                draw(&mut terminal, |frame| {
                    frame.render_widget(
                        Paragraph::new(text).style(Style::new().bg(Color::Black)),
                        frame.area(),
                    );
                    frame.buffer_mut()[(18, 0)]
                        .set_symbol("█")
                        .set_fg(Color::Blue);
                    frame.buffer_mut()[(19, 0)].set_symbol("│");
                })
                .unwrap();
                screen.process(&std::mem::take(&mut writer.0.borrow_mut().bytes));
                assert_eq!(
                    screen.screen().contents().trim_end(),
                    format!("{text}{}█│", " ".repeat(18 - text.width())),
                    "redrawing {text:?} displaced terminal content",
                );
            }
        }
    }

    #[tokio::test]
    async fn scrolling_markdown_cards_matches_a_fresh_terminal_frame() {
        use crate::{
            app::App,
            config::{Settings, Theme},
            session::{Message, MessageStatus, Session},
            ui,
        };

        let settings = Settings {
            theme: Theme::Carbon,
            ..Settings::default()
        };
        let mut session = Session::new();
        for index in 0..8 {
            session
                .messages
                .push(Message::user(format!("问题 {index}"), None));
            let mut reply = Message::assistant_streaming(settings.default_selection());
            reply.append_text(&format!(
                "## 回答 {index}\n\n☕️ 中文说明\n\n```bash\ngit fetch origin\n```\n\n| 分支 | 状态 |\n|---|---|\n| main | ready |",
            ));
            reply.finish(MessageStatus::Completed);
            session.messages.push(reply);
        }
        let mut app = App::new(settings, vec![session]);
        let writer = RecordingWriter::default();
        let options = TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 90, 32)),
        };
        let mut terminal =
            Terminal::with_options(CrosstermBackend::new(writer.clone()), options.clone()).unwrap();
        let mut screen = vt100::Parser::new(32, 90, 0);
        for delta in [0, -3, -8, 5, 0, 50] {
            app.chat_view.borrow_mut().scroll_by(delta);
            if delta == 5 {
                app.sessions[0]
                    .messages
                    .last_mut()
                    .unwrap()
                    .append_text("\n\n新增内容 ☕️");
            }
            draw(&mut terminal, |frame| ui::draw(frame, &app)).unwrap();
            screen.process(&std::mem::take(&mut writer.0.borrow_mut().bytes));

            let fresh_writer = RecordingWriter::default();
            let mut fresh = Terminal::with_options(
                CrosstermBackend::new(fresh_writer.clone()),
                options.clone(),
            )
            .unwrap();
            draw(&mut fresh, |frame| ui::draw(frame, &app)).unwrap();
            let mut expected = vt100::Parser::new(32, 90, 0);
            expected.process(&fresh_writer.0.borrow().bytes);
            assert_eq!(
                screen.screen().contents(),
                expected.screen().contents(),
                "scroll {delta} left artifacts"
            );
            assert_eq!(
                screen.screen().cursor_position(),
                expected.screen().cursor_position()
            );
        }
    }

    #[test]
    fn failed_draw_still_ends_update_and_preserves_original_error() {
        for errors in [
            vec![io::ErrorKind::BrokenPipe],
            vec![io::ErrorKind::BrokenPipe, io::ErrorKind::PermissionDenied],
        ] {
            let writer = RecordingWriter::default();
            writer.0.borrow_mut().flush_errors = errors.into();
            let mut terminal = terminal(writer.clone());
            let error = draw(&mut terminal, |frame| {
                frame.render_widget(Paragraph::new("partial frame"), frame.area());
                frame.set_cursor_position((5, 1));
            })
            .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            let output = writer.0.borrow();
            let bytes = std::str::from_utf8(&output.bytes).unwrap();
            assert!(bytes.starts_with(BEGIN));
            assert!(bytes.ends_with(END));
            assert_eq!(output.flushes.len(), 2);
            assert_eq!(output.flushes.last(), Some(&bytes.len()));
        }
    }
}
