//! 导出真实应用缓冲区，供离线动画预览；不加载用户配置或写入首次启动记录。

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Color};
use serde_json::{Value, json};
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
        let mut render_times = Vec::new();
        let duration = titan::STARTUP_DURATION_MS;
        let fps = 60;
        for index in 0..=(duration * fps).div_ceil(1_000) {
            let ms = (index * 1_000 / fps).min(duration);
            app.startup
                .as_mut()
                .unwrap()
                .advance(now + Duration::from_millis(ms));
            let render_started = Instant::now();
            terminal.draw(|frame| super::draw(frame, &app)).unwrap();
            render_times.push(render_started.elapsed().as_micros());
            frames.push(encode_frame(terminal.backend().buffer()));
        }
        let data = json!({"width": width, "height": height, "fps": fps, "duration": duration, "frames": frames});
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
        render_times.sort_unstable();
        println!(
            "{name}: offscreen render p95 = {} us, max = {} us",
            render_times[render_times.len() * 95 / 100],
            render_times.last().unwrap(),
        );
    }
    println!("Preview frames: {}", directory.display());
}

#[tokio::test]
#[ignore = "exports typing, clearing and reversal frames for visual review"]
async fn export_idle_fade_preview() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/titan-preview");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, width, height, theme) in [
        ("idle-fade", 120, 42, Theme::Vanguard),
        ("idle-fade-compact", 80, 24, Theme::Vanguard),
        ("idle-fade-paper", 100, 32, Theme::Paper),
    ] {
        let mut app = App::new(
            Settings {
                theme,
                language: Lang::En,
                ..Settings::default()
            },
            vec![Session::new()],
        );
        let mut edits = [
            (450, "Hello"),
            (900, ""),
            (1_400, "x"),
            (1_483, ""),
            (1_533, "fast"),
            (1_750, ""),
        ]
        .into_iter()
        .peekable();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut frames = Vec::new();
        let now = Instant::now();
        let duration: u64 = 2_300;
        let fps = 60;
        for index in 0..=(duration * fps).div_ceil(1_000) {
            let ms = (index * 1_000 / fps).min(duration);
            while edits.peek().is_some_and(|(at, _)| *at <= ms) {
                app.editor.set_text(edits.next().unwrap().1);
            }
            terminal
                .draw(|frame| super::draw_at(frame, &app, now + Duration::from_millis(ms)))
                .unwrap();
            frames.push(encode_frame(terminal.backend().buffer()));
        }
        let data = json!({
            "width": width, "height": height, "fps": fps, "duration": duration, "frames": frames,
            "title": "输入淡出 → 清空淡入 → 快速反向",
            "steps": [[0, "IDLE"], [450, "TYPE"], [533, "FADE OUT"], [650, "HIDDEN"],
                [900, "CLEAR"], [1033, "FADE IN"], [1500, "REVERSE"], [duration, "IDLE"]],
        });
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
    }
    println!("Idle fade previews: {}", directory.display());
}

#[tokio::test]
#[ignore = "exports round-robin idle gestures; omits the long rests between actions"]
async fn export_idle_actions_preview() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/titan-preview");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, width, height, theme) in [
        ("idle-actions-detailed", 140, 52, Theme::Vanguard),
        ("idle-actions", 120, 42, Theme::Vanguard),
        ("idle-actions-compact", 80, 24, Theme::Vanguard),
        ("idle-actions-paper", 100, 32, Theme::Paper),
    ] {
        let app = App::new(
            Settings {
                theme,
                language: Lang::En,
                ..Settings::default()
            },
            vec![Session::new()],
        );
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let now = Instant::now();
        terminal
            .draw(|frame| super::draw_at(frame, &app, now))
            .unwrap();
        let duration: u64 = 7_800;
        let fps = 60;
        let mut frames = Vec::new();
        let mut render_times = Vec::new();
        for index in 0..=(duration * fps).div_ceil(1_000) {
            let ms = (index * 1_000 / fps).min(duration);
            let slot = (ms / 2_600).min(2);
            let elapsed = 4_800 + slot * 14_400 + (ms - slot * 2_600);
            let started = Instant::now();
            terminal
                .draw(|frame| super::draw_at(frame, &app, now + Duration::from_millis(elapsed)))
                .unwrap();
            render_times.push(started.elapsed().as_micros());
            frames.push(encode_frame(terminal.backend().buffer()));
        }
        let data = json!({
            "width": width, "height": height, "fps": fps, "duration": duration, "frames": frames,
            "title": "巡视 → 握拳校准 → 竖拇指（预览省略动作间等待）",
            "steps": [[0, "REST"], [900, "LOOK LEFT"], [1900, "LOOK RIGHT"],
                [3600, "RAISE ARM"], [4100, "CLENCH"], [4550, "RELEASE"],
                [6750, "THUMBS UP"], [duration, "REST"]],
        });
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
        render_times.sort_unstable();
        println!(
            "{name}: render p95 = {} us",
            render_times[render_times.len() * 95 / 100]
        );
    }
    println!("Idle action previews: {}", directory.display());
}

fn encode_frame(buffer: &Buffer) -> Vec<Value> {
    let mut runs = Vec::new();
    for y in buffer.area.y..buffer.area.bottom() {
        let mut x = buffer.area.x;
        while x < buffer.area.right() {
            let first = &buffer[(x, y)];
            let start = x;
            let mut text = String::new();
            while x < buffer.area.right() {
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
    runs
}

fn hex(color: Color) -> String {
    if let Color::Rgb(r, g, b) = color {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        "#070e10".into()
    }
}
