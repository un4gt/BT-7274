//! 顶层布局与渲染分发。
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │        Header：当前会话标题           │
//! ├────────────┬────────────────────────┤
//! │ 历史会话 90% │  聊天消息区             │
//! │ ⚙ 设置  10% ├────────────────────────┤
//! │            │  输入框                 │
//! ├────────────┴────────────────────────┤
//! │ Footer：模型 | 上下文 | tok/s | 状态   │
//! └─────────────────────────────────────┘
//! ```

pub(crate) mod activity;
mod art;
mod chat;
mod code;
mod conversation;
mod error;
mod footer;
mod header;
pub(crate) mod markdown;
mod markdown_view;
pub(crate) mod mouse;
mod picker;
mod settings;
mod sidebar;
pub(crate) mod sparkle;
mod theme;
mod think_view;
mod tool_view;
pub(crate) mod transcript;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    widgets::Block,
};

use crate::app::App;

#[cfg(test)]
pub(crate) use chat::input_is_focused;

/// 所有可见后台活动共用同一组动画帧，避免不同面板的加载反馈漂移。
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn spinner_frame(ticks: u64) -> &'static str {
    SPINNER[ticks as usize % SPINNER.len()]
}

/// 渲染整帧。
pub fn draw(frame: &mut Frame, app: &App) {
    app.mouse.borrow_mut().clear();
    let palette = theme::palette(app.settings.theme);
    frame.render_widget(Block::new().style(palette.base()), frame.area());

    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    let (sidebar_area, content_area) = if body_area.width >= 64 {
        let sidebar_width = if body_area.width >= 110 { 28 } else { 24 };
        let [sidebar, _, content] = Layout::horizontal([
            Constraint::Length(sidebar_width),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body_area);
        (sidebar, content)
    } else {
        let sidebar_height = body_area.height.saturating_div(3).clamp(3, 8);
        let [sidebar, _, content] = Layout::vertical([
            Constraint::Length(sidebar_height),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body_area);
        (sidebar, content)
    };

    header::render(app, header_area, frame.buffer_mut(), palette);
    sidebar::render(app, sidebar_area, frame.buffer_mut(), palette);
    chat::render(frame, app, content_area, palette);
    footer::render(app, footer_area, frame.buffer_mut(), palette);

    if let Some(modal) = &app.modal {
        app.mouse.borrow_mut().block_background();
        settings::render(frame, modal, app.ticks);
    }
    if app.picker.is_some() {
        app.mouse.borrow_mut().block_background();
        picker::render(frame, app);
    }
    if app.conversation_overlay.is_some() {
        app.mouse.borrow_mut().block_background();
        conversation::render(frame, app);
    }
    if app.code_overlay.is_some() {
        app.mouse.borrow_mut().block_background();
        code::render(frame, app);
    }
    if app.activity_overlay.is_some() {
        app.mouse.borrow_mut().block_background();
        activity::render(frame, app);
    }
    if app.error_detail.is_some() {
        app.mouse.borrow_mut().block_background();
        error::render(frame, app);
    }
}

/// 固定上限、保留终端边距的居中弹窗区域。
pub(crate) fn popup_rect(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    let width = preferred_width.min(area.width.saturating_sub(2));
    let height = preferred_height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(crate) fn dim_background(frame: &mut Frame, palette: theme::Palette) {
    let area = frame.area();
    frame.buffer_mut().set_style(
        area,
        Style::default()
            .fg(palette.border)
            .bg(palette.background)
            .add_modifier(Modifier::DIM),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{
            ConversationOverlay, Focus, LineEdit, ModelsSection, RecoveryState, SettingsCategory,
            SettingsPane,
        },
        config::{ApiKind, McpServerConfig, Provider, Settings, Theme},
        runtime::{
            context::prepare_compaction,
            conversation::{Message, MessageStatus, Session},
        },
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    fn draw_sizes(app: &App) {
        for (width, height) in [(1, 1), (10, 3), (30, 8), (80, 24), (160, 50)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, app)).unwrap();
        }
    }

    fn render_text(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn assert_input_cursor(app: &App, width: u16, before: &str, under: &str) {
        let mut terminal = Terminal::new(TestBackend::new(width, 34)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let backend = terminal.backend();
        assert!(backend.cursor_visible());
        let cursor = backend.cursor_position();
        // 从实际渲染的单元格检查插入点，不重复组件的标签宽度计算。
        assert!(before.is_ascii());
        let start = cursor.x.checked_sub(before.len() as u16).unwrap();
        let rendered_before = (start..cursor.x)
            .map(|x| backend.buffer()[(x, cursor.y)].symbol())
            .collect::<String>();
        assert_eq!(
            rendered_before, before,
            "cursor at {cursor:?}, width {width}"
        );
        assert_eq!(backend.buffer()[cursor].symbol(), under);
    }

    #[tokio::test]
    async fn proxy_input_cursor_tracks_typing_navigation_and_horizontal_scroll() {
        for language in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let settings = Settings {
                language,
                ..Settings::default()
            };
            let mut app = App::new(settings, vec![Session::new()]);
            app.focus = Focus::Sidebar;
            app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
                .unwrap();
            let modal = app.modal.as_mut().unwrap();
            modal.category = SettingsCategory::Network;
            modal.pane = SettingsPane::Content;
            modal.network_pos = 1;
            app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap();
            for character in "http://127".chars() {
                app.handle_key_events(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                    .unwrap();
            }
            for width in [60, 110] {
                assert_input_cursor(&app, width, "http://127", " ");
            }
            app.handle_key_events(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
                .unwrap();
            assert_input_cursor(&app, 110, "http://12", "7");
            app.handle_key_events(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
                .unwrap();
            assert_input_cursor(&app, 110, "", "h");
            app.handle_key_events(KeyEvent::new(KeyCode::End, KeyModifiers::NONE))
                .unwrap();
            assert_input_cursor(&app, 110, "http://127", " ");
            for text in [
                "你好http://127".to_owned(),
                format!("{}http://127", "x".repeat(160)),
            ] {
                app.modal.as_mut().unwrap().proxy_edit = LineEdit::from_text(&text);
                for width in [60, 110] {
                    assert_input_cursor(&app, width, "http://127", " ");
                }
            }
        }
    }

    #[tokio::test]
    async fn localized_single_line_editors_place_the_cursor_after_the_text() {
        for language in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let settings = Settings {
                language,
                ..Settings::default()
            };
            let mut app = App::new(settings, vec![Session::new()]);
            let value = "http://127";
            app.handle_key_events(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT))
                .unwrap();
            let picker = app.picker.as_mut().unwrap();
            picker.editing = true;
            picker.buffer = value.chars().collect();
            picker.cursor = picker.buffer.len();
            assert_input_cursor(&app, 80, value, " ");

            app.picker = None;
            app.conversation_overlay = Some(ConversationOverlay::Rename {
                session_id: app.open_session().id.clone(),
                edit: LineEdit::from_text(value),
                error: None,
            });
            assert_input_cursor(&app, 80, value, " ");
            app.conversation_overlay = Some(ConversationOverlay::Search {
                edit: LineEdit::from_text(value),
                selected: 0,
            });
            assert_input_cursor(&app, 80, value, " ");

            app.conversation_overlay = None;
            app.focus = Focus::Sidebar;
            app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
                .unwrap();
            app.handle_key_events(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
                .unwrap();
            app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
                .unwrap();
            let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
            wizard.step = 2;
            wizard.field = 0;
            wizard.name = LineEdit::from_text(value);
            assert_input_cursor(&app, 80, value, " ");

            let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
            wizard.step = 3;
            wizard.manual_active = true;
            wizard.manual = LineEdit::from_text(value);
            assert_input_cursor(&app, 80, value, " ");

            let modal = app.modal.as_mut().unwrap();
            modal.wizard = None;
            modal.manual_active = true;
            modal.manual = LineEdit::from_text(value);
            assert_input_cursor(&app, 110, value, " ");
        }
    }

    #[tokio::test]
    async fn mcp_input_cursor_has_room_after_a_scrolled_value() {
        let mut app = App::new(Settings::default(), vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let modal = app.modal.as_mut().unwrap();
        modal.category = SettingsCategory::Mcp;
        modal.pane = SettingsPane::Content;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        let wizard = app.modal.as_mut().unwrap().mcp_wizard.as_mut().unwrap();
        wizard.field = 0;
        wizard.name = LineEdit::from_text(&format!("{}http://127", "x".repeat(160)));
        for width in [60, 110] {
            assert_input_cursor(&app, width, "http://127", " ");
        }
    }

    #[tokio::test]
    async fn all_overlays_render_at_small_terminal_sizes() {
        let mut app = App::new(Settings::default(), vec![Session::new()]);
        draw_sizes(&app);

        app.handle_key_events(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT))
            .unwrap();
        app.picker.as_mut().unwrap().editing = true;
        draw_sizes(&app);
        app.picker = None;

        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        draw_sizes(&app);

        app.handle_key_events(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
        wizard.step = 2;
        wizard.key.buffer = "sk-secret-value".chars().collect();
        wizard.key.cursor = wizard.key.buffer.len();
        wizard.headers.buffer = r#"{"Authorization":"Bearer header-secret"}"#.chars().collect();
        wizard.headers.cursor = wizard.headers.buffer.len();
        wizard.field = 3;
        let rendered = render_text(&app, 100, 30);
        assert!(!rendered.contains("sk-secret-value"));
        assert!(!rendered.contains("header-secret"));
        assert!(rendered.contains('•'));

        let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
        wizard.step = 3;
        wizard.manual_active = true;
        draw_sizes(&app);
    }

    #[tokio::test]
    async fn model_picker_renders_models_from_every_provider() {
        let mut settings = Settings::default();
        settings.providers[0].id = "provider-gemini".to_owned();
        settings.providers[0].name = "Gemini".to_owned();
        settings.providers[0].api_kind = ApiKind::GeminiGenerateContent;
        settings.providers[0].models = vec!["gemini-3.1-pro-preview".to_owned()];
        settings.model = "gemini-3.1-pro-preview".to_owned();

        let mut cerebras = Provider::new(ApiKind::ChatCompletions);
        cerebras.id = "cerebras".to_owned();
        cerebras.name = "Cerebras".to_owned();
        cerebras.models = vec![
            "gemma-4-31b".to_owned(),
            "qwen-3.8-27b".to_owned(),
            "gpt-oss-120b".to_owned(),
        ];
        settings.providers.push(cerebras);

        let mut app = App::new(settings, vec![Session::new()]);
        app.handle_key_events(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT))
            .unwrap();
        let rendered = render_text(&app, 80, 24);

        for expected in [
            "Gemini [gem] (1)",
            "gemini-3.1-pro-preview",
            "Cerebras [chat] (3)",
            "gemma-4-31b",
            "qwen-3.8-27b",
            "gpt-oss-120b",
        ] {
            assert!(
                rendered.contains(expected),
                "missing picker row: {expected}"
            );
        }
    }

    #[tokio::test]
    async fn settings_lists_keep_the_last_selection_visible() {
        let mut settings = Settings::default();
        for index in 1..10 {
            let mut provider = Provider::new(ApiKind::ChatCompletions);
            provider.name = format!("provider-{index}");
            provider.base_url = Some(format!("https://provider-{index}.example/v1"));
            provider.models = (0..20)
                .map(|model| format!("model-{index}-{model}"))
                .collect();
            settings.providers.push(provider);
        }
        let mut app = App::new(settings, vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let modal = app.modal.as_mut().unwrap();
        modal.provider_pos = 9;
        modal.model_pos = 19;
        modal.section = ModelsSection::ModelsList;

        let rendered = render_text(&app, 100, 30);
        assert!(rendered.contains("provider-9"));
        assert!(rendered.contains("model-9-19"));
    }

    #[tokio::test]
    async fn every_theme_renders_at_desktop_and_narrow_sizes() {
        for theme in Theme::ALL {
            let theme_name = match theme {
                Theme::Vanguard => "vanguard",
                Theme::Carbon => "carbon",
                Theme::Paper => "paper",
            };
            let settings: Settings = toml::from_str(&format!("theme = \"{theme_name}\"")).unwrap();
            let app = App::new(settings, vec![Session::new()]);
            draw_sizes(&app);
        }
    }

    #[tokio::test]
    async fn settings_modal_uses_a_scrim_and_spacious_visual_rhythm() {
        let settings = Settings {
            language: crate::i18n::Lang::En,
            theme: Theme::Carbon,
            ..Settings::default()
        };
        let mut app = App::new(settings, vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let modal = app.modal.as_mut().unwrap();
        modal.category = SettingsCategory::Appearance;
        modal.pane = SettingsPane::Category;

        let width = 120;
        let height = 40;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let palette = theme::palette(Theme::Carbon);
        let backdrop = &buffer[(0, 5)];
        assert_eq!(backdrop.bg, palette.background);
        assert!(backdrop.modifier.contains(ratatui::style::Modifier::DIM));

        let rows = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let row_of = |needle: &str| {
            rows.iter()
                .position(|row| row.contains(needle))
                .unwrap_or_else(|| panic!("missing rendered label: {needle}"))
        };

        assert_eq!(row_of("Context") - row_of("Models"), 2);
        assert_eq!(row_of("Theme") - row_of("Language"), 3);
        assert_eq!(row_of("Titan Art") - row_of("Theme"), 3);

        let appearance_row = rows
            .iter()
            .rposition(|row| row.contains("Appearance"))
            .expect("missing Appearance category") as u16;
        assert!(
            (0..width).any(|x| buffer[(x, appearance_row)].bg == palette.selection),
            "selected category should paint its own row"
        );
        for y in appearance_row + 1..height {
            assert!(
                (0..width).all(|x| buffer[(x, y)].bg != palette.selection),
                "category highlight leaked below its row at y={y}"
            );
        }
        assert!(
            (0..width).all(|x| buffer[(x, height - 1)].bg == palette.background),
            "settings content painted outside the popup"
        );
    }

    #[tokio::test]
    async fn keyboard_settings_are_editable_responsive_and_own_the_cursor() {
        let settings = Settings {
            language: crate::i18n::Lang::En,
            ..Settings::default()
        };
        let mut app = App::new(settings, vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        {
            let modal = app.modal.as_mut().unwrap();
            modal.category = SettingsCategory::Keyboard;
            modal.pane = SettingsPane::Content;
            modal.keybinding_pos = 1;
        }

        let rendered = render_text(&app, 110, 34);
        assert!(rendered.contains("Insert newline"));
        assert!(rendered.contains("ctrl+o"));
        assert!(rendered.contains('❯'));
        draw_sizes(&app);

        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        app.modal.as_mut().unwrap().keybinding_edit = Some(LineEdit::from_text("alt+enter"));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(terminal.backend().cursor_visible());
        let cursor = terminal.backend().cursor_position();
        assert!(cursor.x < 80 && cursor.y < 24);

        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(
            app.modal
                .as_ref()
                .unwrap()
                .draft
                .keybindings
                .newline
                .to_string(),
            "alt+enter"
        );
    }

    #[tokio::test]
    async fn fenced_code_viewer_scrolls_and_suppresses_the_chat_cursor() {
        let settings = Settings {
            language: crate::i18n::Lang::En,
            ..Settings::default()
        };
        let mut session = Session::new();
        let mut message = Message::assistant_streaming(settings.default_selection());
        message.status = MessageStatus::Completed;
        message.append_text("```rust\nlet very_long_identifier = \"你好世界 and more text\";\n```");
        session.messages.push(message);
        let mut app = App::new(settings, vec![session]);
        app.editor.set_text("draft");

        app.handle_key_events(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE))
            .unwrap();
        assert!(app.code_overlay.is_some());
        assert!(!chat::input_is_focused(&app));
        app.handle_key_events(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        assert!(app.code_overlay.as_ref().unwrap().horizontal_scroll > 0);
        assert!(render_text(&app, 80, 24).contains("Code 1/1"));
        draw_sizes(&app);

        app.handle_key_events(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(app.code_overlay.is_none());
        assert!(chat::input_is_focused(&app));
    }

    #[tokio::test]
    async fn mcp_settings_and_wizard_are_narrow_safe_redacted_and_single_cursor() {
        let mut settings = Settings::default();
        settings.mcp_servers.push(McpServerConfig {
            name: "docs".to_owned(),
            url: "https://example.com/mcp".to_owned(),
            bearer_token_env_var: None,
            http_headers: Default::default(),
            env_http_headers: Default::default(),
            enabled: false,
            startup_timeout_sec: 30,
        });
        let mut app = App::new(settings, vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        {
            let modal = app.modal.as_mut().unwrap();
            modal.category = SettingsCategory::Mcp;
            modal.pane = SettingsPane::Content;
        }
        draw_sizes(&app);

        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        {
            let wizard = app.modal.as_mut().unwrap().mcp_wizard.as_mut().unwrap();
            wizard.http_headers = LineEdit::from_text(r#"{"x-api-key":"mcp-secret-value"}"#);
            wizard.field = 2;
        }
        draw_sizes(&app);

        let rendered = render_text(&app, 100, 30);
        assert!(!rendered.contains("mcp-secret-value"));
        assert!(!rendered.contains('▌'));
        assert!(rendered.contains('•'));

        let backend = TestBackend::new(30, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(terminal.backend().cursor_visible());
        let cursor = terminal.backend().cursor_position();
        assert!(cursor.x < 30 && cursor.y < 8);
    }

    #[tokio::test]
    async fn proxy_password_is_redacted_until_the_url_is_edited() {
        let settings: Settings = toml::from_str(
            r#"
[proxy]
mode = "http"
url = "http://proxy-user:secret-password@127.0.0.1:7890"
"#,
        )
        .unwrap();
        let mut app = App::new(settings, vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let modal = app.modal.as_mut().unwrap();
        modal.category = SettingsCategory::Network;
        modal.pane = SettingsPane::Content;

        let rendered = render_text(&app, 110, 34);

        assert!(!rendered.contains("secret-password"));
        assert!(rendered.contains("••••"));
    }

    #[tokio::test]
    async fn editing_fields_do_not_paint_a_second_cursor() {
        let mut app = App::new(Settings::default(), vec![Session::new()]);
        assert!(chat::input_is_focused(&app));
        let selection = app.settings.default_selection();
        app.sessions[0]
            .messages
            .push(Message::assistant_streaming(selection));
        app.settings.language = crate::i18n::Lang::En;
        let streaming = render_text(&app, 100, 30);
        assert!(!streaming.contains('▌'));
        assert!(streaming.contains("Generating"));
        app.sessions[0].messages[0].status = MessageStatus::Cancelled;
        let cancelled = render_text(&app, 100, 30);
        assert!(cancelled.contains("Cancelled"));
        assert!(cancelled.contains("cancelled before response text arrived"));
        app.sessions[0].messages.clear();
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let modal = app.modal.as_mut().unwrap();
        modal.category = SettingsCategory::Network;
        modal.pane = SettingsPane::Content;
        modal.network_pos = 1;
        modal.proxy_editing = true;
        assert!(!render_text(&app, 110, 34).contains('▌'));

        app.modal = None;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT))
            .unwrap();
        app.picker.as_mut().unwrap().editing = true;
        assert!(!render_text(&app, 100, 30).contains('▌'));

        app.picker = None;
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
        wizard.step = 2;
        assert!(!render_text(&app, 100, 30).contains('▌'));

        let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
        wizard.step = 3;
        wizard.manual_active = true;
        assert!(!render_text(&app, 100, 30).contains('▌'));
    }

    #[tokio::test]
    async fn classified_error_details_are_keyboard_accessible_and_small_safe() {
        let mut settings = Settings::default();
        settings.providers[0].api_key = Some(crate::secret::SecretValue::from("test-key"));
        let model = settings.model.clone();
        settings.providers[0].model_settings.insert(
            model,
            crate::config::ModelSettings {
                capabilities: crate::config::ModelCapabilities {
                    context_window: Some(1),
                    ..crate::config::ModelCapabilities::default()
                },
                ..crate::config::ModelSettings::default()
            },
        );
        let mut app = App::new(settings, vec![Session::new()]);
        app.editor.set_text("too much context");
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(app.notice().unwrap().error.is_some());

        app.handle_key_events(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE))
            .unwrap();
        assert!(app.error_detail.is_some());
        assert!(render_text(&app, 100, 30).contains("Esc/F2"));
        draw_sizes(&app);

        app.handle_key_events(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(app.error_detail.is_none());
    }

    #[tokio::test]
    async fn conversation_overlays_are_keyboard_focused_and_small_terminal_safe() {
        let settings = Settings {
            language: crate::i18n::Lang::En,
            ..Settings::default()
        };
        let mut app = App::new(settings, vec![Session::new()]);
        app.conversation_overlay = Some(ConversationOverlay::Rename {
            session_id: app.open_session().id.clone(),
            edit: LineEdit::from_text("renamed session"),
            error: None,
        });
        draw_sizes(&app);
        assert!(render_text(&app, 100, 30).contains("Rename Session"));
        assert!(!render_text(&app, 100, 30).contains('▌'));

        app.conversation_overlay = Some(ConversationOverlay::Search {
            edit: LineEdit::from_text("session"),
            selected: 0,
        });
        draw_sizes(&app);
        assert!(render_text(&app, 100, 30).contains("Search Sessions"));

        let selection = app.settings.default_selection();
        let mut partial = Message::assistant_streaming(selection);
        partial.append_text("persisted partial reply");
        app.sessions[0].messages.push(partial);
        app.conversation_overlay = Some(ConversationOverlay::Recovery(RecoveryState {
            session_id: app.open_session().id.clone(),
            message_index: 0,
            selected: 1,
            error: None,
        }));
        draw_sizes(&app);
        let recovery = render_text(&app, 100, 30);
        assert!(recovery.contains("Unfinished Generation Found"));
        assert!(recovery.contains("Keep as cancelled"));

        app.sessions[0].messages.clear();
        for index in 0..8 {
            if index % 2 == 0 {
                app.sessions[0]
                    .messages
                    .push(Message::user(format!("task {index}"), None));
            } else {
                let mut assistant = Message::assistant_streaming(app.settings.default_selection());
                assistant.append_text(&(format!("answer {index}")));
                assistant.status = MessageStatus::Completed;
                app.sessions[0].messages.push(assistant);
            }
        }
        let model = app
            .settings
            .provider()
            .settings_for_model(&app.settings.model);
        let plan =
            prepare_compaction(app.open_session(), &model, &app.settings.context, false).unwrap();
        app.conversation_overlay = Some(ConversationOverlay::Compact {
            session_id: app.open_session().id.clone(),
            plan,
            scroll: 0,
        });
        draw_sizes(&app);
        assert!(render_text(&app, 100, 30).contains("Compaction Preview"));
    }

    #[tokio::test]
    async fn context_settings_show_state_and_non_color_focus_marker() {
        let settings = Settings {
            language: crate::i18n::Lang::En,
            ..Settings::default()
        };
        let mut app = App::new(settings, vec![Session::new()]);
        app.focus = Focus::Sidebar;
        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let modal = app.modal.as_mut().unwrap();
        modal.category = SettingsCategory::Context;
        modal.pane = SettingsPane::Content;
        modal.context_pos = 1;

        let rendered = render_text(&app, 110, 34);
        assert!(rendered.contains("Auto compact"));
        assert!(rendered.contains("Threshold"));
        assert!(rendered.contains("85%"));
        assert!(rendered.contains('❯'));
        draw_sizes(&app);
    }
}
