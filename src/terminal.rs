//! 将画面与光标作为完整的一帧提交，避免动画暴露重绘的中间状态。

use std::io::{self, Write};

use crossterm::{
    ExecutableCommand, QueueableCommand,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};

pub(crate) fn draw<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    render: impl FnOnce(&mut Frame),
) -> io::Result<()> {
    terminal.backend_mut().queue(BeginSynchronizedUpdate)?;
    // 由 Ratatui 根据焦点决定光标显隐；不在每帧开始时额外隐藏光标。
    let draw_result = terminal.draw(render).map(|_| ());
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
