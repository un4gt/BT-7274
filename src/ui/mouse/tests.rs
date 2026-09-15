use super::*;
use crate::{
    app::{App, Focus},
    config::Settings,
    session::{Message, MessageStatus, Session},
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};
use unicode_width::UnicodeWidthStr;

fn draw(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    let bounds = terminal.backend().buffer().area;
    for (area, _) in &app.mouse.borrow().regions {
        assert_eq!(area.intersection(bounds), *area);
    }
    terminal
}

fn row_from(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    let mut column = x;
    let mut text = String::new();
    while column < buffer.area.right() {
        let symbol = buffer[(column, y)].symbol();
        text.push_str(symbol);
        column += symbol.width().max(1) as u16;
    }
    text
}

fn rendered(terminal: &Terminal<TestBackend>) -> String {
    (0..terminal.backend().buffer().area.height)
        .map(|y| row_from(terminal, 0, y))
        .collect::<Vec<_>>()
        .join("\n")
}

// Locate visible glyphs rather than deriving coordinates from the hit map under test.
fn locate(terminal: &Terminal<TestBackend>, text: &str) -> Position {
    let area = terminal.backend().buffer().area;
    for y in 0..area.height {
        for x in 0..area.width {
            if row_from(terminal, x, y).starts_with(text) {
                return Position::new(x, y);
            }
        }
    }
    panic!("missing {text:?}\n{}", rendered(terminal));
}

fn mouse(app: &mut App, position: Position, kind: MouseEventKind) -> bool {
    app.handle_mouse_events(MouseEvent {
        kind,
        column: position.x,
        row: position.y,
        modifiers: KeyModifiers::NONE,
    })
}

fn click(app: &mut App, terminal: &Terminal<TestBackend>, text: &str) {
    assert!(mouse(
        app,
        locate(terminal, text),
        MouseEventKind::Down(MouseButton::Left)
    ));
}

fn tool_message(settings: &Settings, name: &str, output: &str) -> Message {
    let mut message = Message::assistant_streaming(settings.default_selection());
    message.push_tool_call(
        "call".into(),
        "server".into(),
        name.into(),
        serde_json::json!({"query": name}),
    );
    message.push_tool_result(
        "call".into(),
        "server".into(),
        name.into(),
        serde_json::json!({"content": [{"type": "text", "text": output}]}),
        false,
    );
    message.append_reasoning("post-tool reasoning");
    message.append_text("answer");
    message.finish(MessageStatus::Completed);
    message
}

#[tokio::test]
async fn click_opens_the_chosen_tool_and_tabs_scroll_without_changing_the_chat() {
    let settings = Settings::default();
    let output = format!(
        "first result\n\n{}",
        (0..80)
            .map(|i| format!("row {i:03}\n\n"))
            .collect::<String>()
    );
    let first = tool_message(&settings, "first-query", &output);
    let expected_id = first.id.clone();
    let mut session = Session::new();
    session.messages = vec![
        first,
        tool_message(&settings, "second-query", "second result"),
    ];
    let mut app = App::new(settings, vec![session]);
    app.editor.set_text("keep my draft");
    let terminal = draw(&app, 120, 40);
    let before = rendered(&terminal);
    let input = locate(&terminal, "keep my draft");
    click(&mut app, &terminal, "1 项工具");
    assert_eq!(
        app.activity_overlay
            .as_ref()
            .unwrap()
            .selected
            .as_ref()
            .unwrap()
            .message_id,
        expected_id
    );
    assert_eq!(app.activity_overlay.as_ref().unwrap().tab, 1);
    let terminal = draw(&app, 120, 40);
    assert!(rendered(&terminal).contains("first result"));
    assert!(!terminal.backend().cursor_visible());
    mouse(&mut app, input, MouseEventKind::Down(MouseButton::Left));
    assert_eq!(app.editor.cursor(), "keep my draft".len());
    click(&mut app, &terminal, "参数");
    let terminal = draw(&app, 120, 40);
    assert!(rendered(&terminal).contains("\"query\": \"first-query\""));
    click(&mut app, &terminal, "原始");
    let terminal = draw(&app, 120, 40);
    assert!(rendered(&terminal).contains("\"content\""));
    click(&mut app, &terminal, "结果");
    let terminal = draw(&app, 120, 40);
    assert!(mouse(
        &mut app,
        locate(&terminal, "first result"),
        MouseEventKind::ScrollDown
    ));
    let terminal = draw(&app, 120, 40);
    assert!(!rendered(&terminal).contains("first result"));
    assert!(rendered(&terminal).contains("row 003"));
    click(&mut app, &terminal, "[关闭]");
    assert!(app.activity_overlay.is_none());
    assert_eq!(app.editor.text(), "keep my draft");
    assert_eq!(rendered(&draw(&app, 120, 40)), before);
}

#[tokio::test]
async fn compact_details_support_mouse_return_to_scrolled_step_list() {
    let settings = Settings::default();
    let mut session = Session::new();
    session.messages = (0..40)
        .map(|i| {
            tool_message(
                &settings,
                &format!("search-{i:02}"),
                &format!("result-{i:02}"),
            )
        })
        .collect();
    let expected = session.messages[36].id.clone();
    let mut app = App::new(settings, vec![session]);
    let terminal = draw(&app, 70, 24);
    click(&mut app, &terminal, "1 项工具");
    let terminal = draw(&app, 70, 24);
    assert!(app.activity_overlay.as_ref().unwrap().detail_focus);
    click(&mut app, &terminal, "[列表]");
    let terminal = draw(&app, 70, 24);
    click(&mut app, &terminal, "search-36");
    assert_eq!(
        app.activity_overlay
            .as_ref()
            .unwrap()
            .selected
            .as_ref()
            .unwrap()
            .message_id,
        expected
    );
    let terminal = draw(&app, 70, 24);
    assert!(rendered(&terminal).contains("result-36"));
    for (width, height) in [(30, 8), (10, 3), (1, 1), (160, 50)] {
        draw(&app, width, height);
    }
}

#[tokio::test]
async fn mouse_positions_and_drag_selection_follow_rendered_unicode_cells() {
    let mut app = App::new(Settings::default(), vec![Session::new()]);
    app.editor.set_text("a你👩‍💻e\u{301}\tZ");
    let terminal = draw(&app, 100, 30);
    let chinese = locate(&terminal, "你");
    assert!(mouse(
        &mut app,
        Position::new(chinese.x + 1, chinese.y),
        MouseEventKind::Down(MouseButton::Left)
    ));
    assert_eq!(app.editor.cursor(), 1);
    let terminal = draw(&app, 100, 30);
    assert_eq!(terminal.backend().cursor_position(), chinese);
    assert!(mouse(
        &mut app,
        locate(&terminal, "e\u{301}"),
        MouseEventKind::Drag(MouseButton::Left)
    ));
    assert_eq!(app.editor.selected_text(), Some("你👩‍💻"));
    mouse(&mut app, chinese, MouseEventKind::Up(MouseButton::Left));
    app.handle_key_events(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.editor.text(), "aXe\u{301}\tZ");
    let terminal = draw(&app, 100, 30);
    let input = locate(&terminal, "aXe\u{301}");
    let cursor = app.editor.cursor();
    app.focus = Focus::Sidebar;
    app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
        .unwrap();
    draw(&app, 100, 30);
    assert!(!mouse(
        &mut app,
        input,
        MouseEventKind::Down(MouseButton::Left)
    ));
    assert!(!mouse(
        &mut app,
        input,
        MouseEventKind::Drag(MouseButton::Left)
    ));
    assert_eq!(app.editor.cursor(), cursor);
}

#[tokio::test]
async fn mouse_wheel_holds_history_position_as_new_text_arrives() {
    let settings = Settings::default();
    let mut message = Message::assistant_streaming(settings.default_selection());
    message.append_text(
        &(0..80)
            .map(|i| format!("row {i:03}\n\n"))
            .collect::<String>(),
    );
    let mut app = App::new(settings, vec![Session::new()]);
    app.sessions[0].messages.push(message);
    let terminal = draw(&app, 120, 40);
    assert!(mouse(
        &mut app,
        locate(&terminal, "row 079"),
        MouseEventKind::ScrollUp
    ));
    let terminal = draw(&app, 120, 40);
    assert!(!rendered(&terminal).contains("row 079"));
    assert!(rendered(&terminal).contains("row 075"));
    app.sessions[0].messages[0].append_text("\n\nnew output");
    let terminal = draw(&app, 120, 40);
    assert!(!rendered(&terminal).contains("new output"));
    assert!(rendered(&terminal).contains("row 075"));
}

#[tokio::test]
async fn sidebar_click_uses_the_scrolled_list_offset_and_opens_settings() {
    let sessions = (0..40)
        .map(|i| {
            let mut session = Session::new();
            session.title = Some(format!("session-{i:02}"));
            session
        })
        .collect();
    let mut app = App::new(Settings::default(), sessions);
    app.focus = Focus::Sidebar;
    app.sidebar_pos = 39;
    let terminal = draw(&app, 120, 40);
    click(&mut app, &terminal, "session-30");
    assert_eq!(app.current, 30);
    assert_eq!(app.focus, Focus::Input);
    let terminal = draw(&app, 120, 40);
    click(&mut app, &terminal, "设置");
    assert!(app.modal.is_some());
}
