use super::*;
use crate::{
    config::{Settings, Theme},
    session::Session,
    ui::theme,
};
use ratatui::buffer::Cell;

fn render(app: &App, area: Rect) -> Buffer {
    let mut buffer = Buffer::filled(
        Rect::new(0, 0, area.right() + 2, area.bottom() + 2),
        Cell::new("x"),
    );
    app.mouse.borrow_mut().clear();
    render_messages(app, area, &mut buffer, theme::palette(app.settings.theme));
    buffer
}

fn viewport(buffer: &Buffer, area: Rect) -> Vec<Vec<Cell>> {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].clone())
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn message_cards_scroll_together_and_keep_history_anchored() {
    let mut session = Session::new();
    session.messages = (0..5)
        .map(|i| Message::user(format!("message {i}"), None))
        .collect();
    let mut app = App::new(Settings::default(), vec![session]);
    let area = Rect::new(3, 2, 48, 26);
    let content = content_area(area);
    let buffer = render(&app, area);
    let tops = (content.y..content.bottom())
        .filter(|y| buffer[(content.x, *y)].symbol() == "╭")
        .collect::<Vec<_>>();
    assert_eq!(tops.len(), 5, "each message must have its own frame");
    for (index, y) in tops.iter().copied().enumerate() {
        assert_eq!(buffer[(content.right() - 1, y)].symbol(), "╮");
        assert_eq!(buffer[(content.x, y + 2)].symbol(), "╰");
        assert_eq!(buffer[(content.right() - 1, y + 2)].symbol(), "╯");
        let body = (content.x + 2..content.right() - 2)
            .map(|x| buffer[(x, y + 1)].symbol())
            .collect::<String>();
        assert_eq!(body.trim_end(), format!("message {index}"));
        assert!((content.x..content.right()).all(|x| buffer[(x, y + 3)].symbol() == " "));
    }

    let area = Rect::new(3, 2, 48, 14);
    let content = content_area(area);
    let before = viewport(&render(&app, area), content);
    app.chat_view.borrow_mut().scroll_by(-3);
    let scrolled = viewport(&render(&app, area), content);
    assert_eq!(&scrolled[3..], &before[..before.len() - 3]);
    assert!(!app.chat_view.borrow().follow_tail);
    app.sessions[0]
        .messages
        .push(Message::user("new message".into(), None));
    assert_eq!(viewport(&render(&app, area), content), scrolled);
}

#[tokio::test]
async fn markdown_and_scrollbar_stay_in_separate_regions_during_scroll_and_resize() {
    for theme in Theme::ALL {
        let settings = Settings {
            theme,
            ..Settings::default()
        };
        let mut session = Session::new();
        let mut message = Message::assistant_streaming(settings.default_selection());
        message.append_text(&format!(
            "# 中文标题\n\n{}\n\n```bash\n{}\n```\n\n| A | B |\n|---|---|\n| 中文 | {} |",
            "正文 ☕️\n\n".repeat(20),
            "echo 你好 ".repeat(40),
            "cell ".repeat(40),
        ));
        message.finish(MessageStatus::Completed);
        session.messages.push(message);
        let app = App::new(settings, vec![session]);
        for (width, height) in [(48, 18), (22, 9), (8, 5), (3, 3), (1, 1), (60, 24)] {
            for scroll in [0, -7, -500, 4, 500] {
                app.chat_view.borrow_mut().scroll_by(scroll);
                let area = Rect::new(3, 2, width, height);
                let buffer = render(&app, area);
                let layout = TranscriptLayout::new(area);
                for y in buffer.area.y..buffer.area.bottom() {
                    for x in buffer.area.x..buffer.area.right() {
                        if !area.contains((x, y).into()) {
                            assert_eq!(
                                buffer[(x, y)],
                                Cell::new("x"),
                                "paint leaked outside panel"
                            );
                        }
                    }
                }
                if !layout.scrollbar.is_empty() {
                    assert!(layout.viewport.right() < layout.scrollbar.x);
                    for y in layout.viewport.y..layout.viewport.bottom() {
                        assert_eq!(buffer[(layout.viewport.right(), y)].symbol(), " ");
                        let cell = &buffer[(layout.scrollbar.x, y)];
                        assert!(
                            ["▲", "▼", "║", "█"].contains(&cell.symbol()),
                            "Markdown reached scrollbar: {cell:?}"
                        );
                    }
                }
            }
        }
    }
}
