//! A reusable Ratatui splash. Render `Intro::at(elapsed_ms)` until `DURATION_MS`.
//! Sprite, motion, particles, sensor illumination and camera distortion are layers.

mod effects;
mod idle;
mod paint;
mod portrait;
mod sprite;
mod timeline;
mod transition;

pub use idle::Idle;
pub use timeline::DURATION_MS;
pub use transition::{Handoff, STARTUP_DURATION_MS, render_startup};

use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

use paint::{AMBER, BLACK, COLD, Canvas, DIM, STEEL, mix};
use timeline::{APPEAR_MS, IGNITION_MS, IMPACT_MS, ONLINE_MS, Phase, Timeline, progress};

pub struct Intro {
    elapsed_ms: u64,
}

impl Intro {
    pub fn at(elapsed_ms: u64) -> Self {
        Self { elapsed_ms }
    }
}

impl Widget for Intro {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let scene = render(area.width, area.height, self.elapsed_ms);
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) {
                    *cell = scene[(x, y)].clone();
                }
            }
        }
    }
}

pub fn render(width: u16, height: u16, elapsed_ms: u64) -> Buffer {
    let mut canvas = Canvas::new(width, height);
    let time = Timeline::at(elapsed_ms);
    if time.phase == Phase::Blank || width == 0 || height == 0 {
        return canvas.buffer;
    }
    let (detailed, ground) = scene_metrics(width, height);
    effects::atmosphere(&mut canvas, time, ground);
    effects::ground(&mut canvas, time, ground);
    hud(&mut canvas, time, detailed);
    effects::dust(&mut canvas, time, ground, false);
    let mut chassis_layer = Canvas::new(width, height);
    let chassis = sprite::draw(&mut chassis_layer, time, ground, detailed);
    effects::thrusters(&mut canvas, time, chassis, ground);
    canvas.overlay(&chassis_layer);
    effects::shockwave(&mut canvas, time, ground);
    effects::dust(&mut canvas, time, ground, true);
    effects::eye(&mut canvas, time, chassis);
    captions(&mut canvas, time, ground, detailed);
    effects::impact_camera(&mut canvas, time);
    canvas.buffer
}

fn scene_metrics(width: u16, height: u16) -> (bool, i32) {
    let detailed = width >= 86 && height >= 36;
    let ground = (i32::from(height)
        - if detailed {
            7
        } else if height >= 22 {
            5
        } else {
            3
        })
    .max(0);
    (detailed, ground)
}

/// 交接期间单独淡出环境，机体由连续的摄像机变换绘制。
fn backdrop(width: u16, height: u16, ms: u64) -> Buffer {
    let mut canvas = Canvas::new(width, height);
    let time = Timeline::at(ms);
    let (detailed, ground) = scene_metrics(width, height);
    effects::atmosphere(&mut canvas, time, ground);
    effects::ground(&mut canvas, time, ground);
    hud(&mut canvas, time, detailed);
    captions(&mut canvas, time, ground, detailed);
    canvas.buffer
}

fn hud(canvas: &mut Canvas, time: Timeline, detailed: bool) {
    if canvas.height < 24 {
        return;
    }
    let fade = progress(time.ms, 240, IGNITION_MS);
    let color = mix(BLACK, DIM, fade);
    canvas.text(2, 1, "+", color);
    canvas.text(canvas.width - 3, 1, "+", color);
    canvas.text(5, 1, "M I L I T I A  /  S R S", color);
    let label = if time.phase == Phase::Online {
        "VANGUARD / LINK ESTABLISHED"
    } else {
        "VANGUARD / ORBITAL INSERTION"
    };
    if canvas.width >= 76 {
        canvas.text(canvas.width - label.len() as i32 - 5, 1, label, color);
    }
    if !detailed {
        return;
    }
    let (state, value) = match time.phase {
        Phase::Falling => ("DESCENT", "TERMINAL VELOCITY"),
        Phase::Braking => ("RETRO BURN", "BRACING FOR IMPACT"),
        Phase::Impact => ("CONTACT", "KINETIC OVERLOAD"),
        Phase::Rising => ("ACTUATORS", "STABILIZING"),
        Phase::Waking => ("NEURAL LINK", "SYNCHRONIZING"),
        Phase::Online => ("PROTOCOL 03", "PROTECT THE PILOT"),
        Phase::Blank => ("", ""),
    };
    canvas.text(
        5,
        canvas.height / 2 - 1,
        "// BT-7274",
        mix(BLACK, COLD, fade * 0.85),
    );
    canvas.text(5, canvas.height / 2 + 1, state, mix(BLACK, STEEL, fade));
    if canvas.width >= 110 {
        canvas.text(5, canvas.height / 2 + 2, value, color);
    }

    let rail = canvas.width - 7;
    for y in 6..canvas.height - 9 {
        canvas.put(rail, y, if y % 3 == 0 { '-' } else { ':' }, color);
    }
    let marker = if time.is_airborne() {
        6 + ((canvas.height - 16) as f32 * progress(time.ms, APPEAR_MS, IMPACT_MS)) as i32
    } else {
        canvas.height - 10
    };
    canvas.put(rail - 1, marker, '>', mix(BLACK, AMBER, fade * 0.8));
}

fn captions(canvas: &mut Canvas, time: Timeline, ground: i32, detailed: bool) {
    if time.ms < ONLINE_MS {
        let (text, color) = match time.phase {
            Phase::Falling => ("// STAND BY FOR TITANFALL", DIM),
            Phase::Braking => ("// RETRO THRUST ENGAGED", STEEL),
            Phase::Impact => ("// IMPACT", AMBER),
            Phase::Rising => ("// CHASSIS STABILIZING", DIM),
            Phase::Waking => ("// ESTABLISHING NEURAL LINK", STEEL),
            _ => ("", DIM),
        };
        canvas.centered(
            ground + 2,
            text,
            mix(BLACK, color, progress(time.ms, 250, 600)),
        );
        return;
    }
    let fade = progress(time.ms, ONLINE_MS, ONLINE_MS + 210);
    // One decisive reveal after the sensor has reached full brightness.
    canvas.centered(ground + 2, "BT-7274 ONLINE", mix(STEEL, AMBER, fade));
    if detailed {
        canvas.centered(
            ground + 4,
            "PROTOCOL 03  /  PROTECT THE PILOT",
            mix(BLACK, STEEL, fade),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_all_phases_at_supported_and_extreme_sizes() {
        for (w, h) in [
            (0, 0),
            (1, 1),
            (40, 12),
            (52, 22),
            (80, 24),
            (100, 36),
            (120, 44),
        ] {
            for ms in (0..=DURATION_MS + 200).step_by(16) {
                let frame = render(w, h, ms);
                assert_eq!(frame.content.len(), usize::from(w) * usize::from(h));
                assert!(frame.content.iter().all(|cell| cell.symbol() != "@"));
            }
        }
    }

    #[test]
    fn starts_blank_and_finishes_with_a_lit_sensor_and_online_label() {
        let blank = render(110, 42, 0);
        assert!(blank.content.iter().all(|c| c.symbol() == " "));
        let done = render(110, 42, DURATION_MS);
        let text: String = done.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("BT-7274 ONLINE"));
        let eyes: Vec<_> = done.content.iter().filter(|c| c.symbol() == "◉").collect();
        assert_eq!(eyes.len(), 1);
        assert_eq!(eyes[0].fg, AMBER);
        assert_eq!(done, render(110, 42, u64::MAX));
    }

    #[test]
    fn silhouette_appears_at_100_ms_and_impact_does_not_ignite_the_eye() {
        let first = render(110, 42, 100);
        assert!(
            first
                .content
                .iter()
                .any(|c| c.symbol() != " " && c.fg != BLACK)
        );
        let impact = render(110, 42, IMPACT_MS);
        let sensor = impact.content.iter().find(|c| c.symbol() == "●").unwrap();
        assert_eq!(sensor.fg, ratatui::style::Color::Rgb(44, 19, 15));
    }

    #[test]
    fn renders_as_a_widget_inside_an_offset_viewport() {
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Intro::at(DURATION_MS), Rect::new(5, 3, 110, 42)))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), " ");
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|c| c.symbol() == "◉")
        );
    }
}
