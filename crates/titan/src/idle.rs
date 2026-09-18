use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

use crate::{
    effects,
    idle_motion::{self, IdleAnimation},
    paint::{BLACK, Canvas, mix, rgb},
    portrait::Portrait,
    sprite,
    timeline::{DURATION_MS, Timeline},
};

/// 已上线的 BT：默认静止，宿主可提供动画时钟播放待机动作。
#[derive(Clone, Copy)]
pub struct Idle {
    background: Color,
    armor: Color,
    sensor: Color,
    animation: IdleAnimation,
}

impl Idle {
    pub fn new(background: Color, armor: Color, sensor: Color) -> Self {
        Self {
            background,
            armor,
            sensor,
            animation: IdleAnimation::default(),
        }
    }

    pub fn animate(mut self, animation: IdleAnimation) -> Self {
        self.animation = animation;
        self
    }

    /// 在宿主提供的空白区域渐显机体；零透明度保持原缓冲区不变。
    pub fn render_with_opacity(self, area: Rect, buffer: &mut Buffer, opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        if area.is_empty() || opacity == 0.0 {
            return;
        }
        let portrait = self.portrait(area.width, area.height);
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) else {
                    continue;
                };
                let source = &portrait.buffer[(x, y)];
                let bg = mix(cell.bg, source.bg, opacity);
                *cell = source.clone();
                cell.set_bg(bg).set_fg(mix(bg, source.fg, opacity));
            }
        }
    }

    pub(crate) fn portrait(self, width: u16, height: u16) -> Portrait {
        let mut canvas = Canvas::new(width, height);
        let detailed = width >= 64 && height >= 32;
        let sprite_height = if detailed { 27 } else { 17 };
        let ground = ((canvas.height + sprite_height) / 2).min(canvas.height - 1);
        let time = Timeline::at(DURATION_MS);
        let chassis = sprite::draw(&mut canvas, time, ground, detailed);
        effects::eye(&mut canvas, time, chassis);
        let mut portrait = Portrait::from_canvas(canvas, chassis);
        for cell in &mut portrait.buffer.content {
            let (r, g, b) = rgb(cell.fg);
            let light = f32::from(r.max(g).max(b)) / 189.0;
            cell.set_fg(if cell.symbol() == "◉" {
                self.sensor
            } else {
                mix(self.background, self.armor, light)
            });
            cell.set_bg(if cell.bg == BLACK {
                self.background
            } else {
                mix(self.background, self.armor, 0.12)
            });
        }
        portrait.background = self.background;
        idle_motion::animate(
            &mut portrait.buffer,
            chassis,
            self.animation,
            self.background,
            self.armor,
            self.sensor,
        );
        portrait
    }
}

impl Widget for Idle {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        self.render_with_opacity(area, buffer, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_art_respects_theme_bounds_and_is_stable() {
        for area in [
            Rect::new(3, 2, 1, 1),
            Rect::new(3, 2, 45, 10),
            Rect::new(3, 2, 90, 36),
        ] {
            let background = Color::Rgb(243, 247, 246);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 42));
            Idle::new(background, Color::Rgb(90, 110, 105), Color::Red).render(area, &mut buffer);
            assert_eq!(buffer[(0, 0)].symbol(), " ");
            assert_eq!(buffer[(0, 0)].bg, Color::Reset);
            if area.width > 1 {
                assert_eq!(buffer[(area.x, area.y)].bg, background);
            }
            let first = buffer.clone();
            Idle::new(background, Color::Rgb(90, 110, 105), Color::Red).render(area, &mut buffer);
            assert_eq!(first, buffer);
            if area.height > 10 {
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "◉" && cell.fg == Color::Red)
                );
            }
        }
    }

    #[test]
    fn opacity_fades_sensor_and_armor_without_changing_their_positions() {
        let area = Rect::new(3, 2, 80, 28);
        for background in [Color::Rgb(12, 24, 27), Color::Rgb(255, 255, 255)] {
            let idle = Idle::new(
                background,
                Color::Rgb(103, 137, 133),
                Color::Rgb(251, 146, 60),
            );
            let mut blank = Buffer::empty(Rect::new(0, 0, 90, 34));
            blank.set_style(area, ratatui::style::Style::default().bg(background));
            let mut hidden = blank.clone();
            idle.render_with_opacity(area, &mut hidden, 0.0);
            assert_eq!(hidden, blank);
            let mut full = blank.clone();
            idle.render(area, &mut full);
            let mut middle = blank.clone();
            idle.render_with_opacity(area, &mut middle, 0.5);
            assert_eq!(middle[(0, 0)], blank[(0, 0)]);
            for (partial, opaque) in middle.content.iter().zip(&full.content) {
                assert_eq!(partial.symbol(), opaque.symbol());
                if opaque.symbol() != " " {
                    assert_ne!(partial.fg, opaque.fg);
                    assert_ne!(partial.fg, background);
                }
            }
        }
    }

    #[test]
    fn gestures_keep_feet_planted_one_sensor_and_return_to_the_rest_pose() {
        for (width, height) in [
            (0, 0),
            (1, 1),
            (8, 4),
            (18, 8),
            (32, 12),
            (49, 9),
            (80, 28),
            (100, 38),
        ] {
            let area = Rect::new(3, 2, width, height);
            for background in [Color::Rgb(12, 24, 27), Color::Rgb(255, 255, 255)] {
                let idle = Idle::new(
                    background,
                    Color::Rgb(103, 137, 133),
                    Color::Rgb(251, 146, 60),
                );
                let render = |ms| {
                    let mut buffer = Buffer::empty(Rect::new(0, 0, width + 6, height + 4));
                    idle.animate(IdleAnimation::at(ms))
                        .render(area, &mut buffer);
                    buffer
                };
                let rest = render(0);
                let last_row = (area.y..area.bottom())
                    .rev()
                    .find(|y| (area.x..area.right()).any(|x| rest[(x, *y)].symbol() != " "));
                for start in [5_000, 19_400, 33_800] {
                    assert_eq!(render(start), rest);
                    assert_eq!(render(start + 2_400), rest);
                    if width > 32 {
                        assert!(
                            render(start + 1_000) != rest,
                            "unchanged pose at {start} ms, {width}x{height}"
                        );
                    }
                    for ms in (start..=start + 2_400).step_by(16) {
                        let frame = render(ms);
                        assert_eq!(frame[(0, 0)], rest[(0, 0)]);
                        if width > 1 {
                            assert_eq!(
                                frame.content.iter().filter(|c| c.symbol() == "◉").count(),
                                1
                            );
                        }
                        if let Some(y) = last_row {
                            for x in area.x..area.right() {
                                assert_eq!(frame[(x, y)], rest[(x, y)], "foot moved at {ms} ms");
                            }
                        }
                    }
                }
            }
        }
    }
}
