//! 导出真实应用缓冲区，供离线动画预览；不加载用户配置或写入首次启动记录。

use ratatui::{Terminal, backend::TestBackend, style::Color};
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::App,
    config::{Settings, Theme},
    i18n::Lang,
    session::Session,
    startup::Startup,
};

#[tokio::test]
#[ignore = "exports animation frames to target/titan-preview for visual review"]
async fn export_titan_preview() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/titan-preview");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, width, height, theme, idle) in [
        ("desktop", 120, 42, Theme::Vanguard, true),
        ("compact", 80, 24, Theme::Vanguard, true),
        ("paper", 100, 32, Theme::Paper, true),
        ("no-idle", 100, 32, Theme::Carbon, false),
    ] {
        let mut app = App::new(
            Settings {
                theme,
                language: Lang::En,
                show_titan_when_idle: idle,
                ..Settings::default()
            },
            vec![Session::new()],
        );
        let now = Instant::now();
        app.startup = Some(Startup::for_test(now, true));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut frames = Vec::new();
        let duration = titan::STARTUP_DURATION_MS;
        let fps = 60;
        for index in 0..=(duration * fps).div_ceil(1_000) {
            let ms = (index * 1_000 / fps).min(duration);
            app.startup
                .as_mut()
                .unwrap()
                .advance(now + Duration::from_millis(ms));
            terminal.draw(|frame| super::draw(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            let mut runs = Vec::new();
            for y in 0..height {
                let mut x = 0;
                while x < width {
                    let first = &buffer[(x, y)];
                    let start = x;
                    let mut text = String::new();
                    while x < width {
                        let cell = &buffer[(x, y)];
                        if cell.fg != first.fg || cell.bg != first.bg {
                            break;
                        }
                        text.push_str(cell.symbol());
                        x += cell.symbol().width().max(1) as u16;
                    }
                    runs.push(json!([start, y, hex(first.fg), hex(first.bg), text]));
                }
            }
            frames.push(runs);
        }
        let data = json!({"width": width, "height": height, "fps": fps, "duration": duration, "frames": frames});
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
    }
    println!("Preview frames: {}", directory.display());
}

fn hex(color: Color) -> String {
    if let Color::Rgb(r, g, b) = color {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        "#070e10".into()
    }
}
