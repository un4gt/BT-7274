use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

use crate::{
    effects,
    paint::{BLACK, Canvas, mix, rgb},
    portrait::Portrait,
    sprite,
    timeline::{DURATION_MS, Timeline},
};

/// 已上线的 BT：只画机体，使用宿主主题，不需要持续刷新。
#[derive(Clone, Copy)]
pub struct Idle {
    background: Color,
    armor: Color,
    sensor: Color,
}

impl Idle {
    pub fn new(background: Color, armor: Color, sensor: Color) -> Self {
        Self {
            background,
            armor,
            sensor,
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
        portrait
    }
}

impl Widget for Idle {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let portrait = self.portrait(area.width, area.height);
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) else {
                    continue;
                };
                *cell = portrait.buffer[(x, y)].clone();
            }
        }
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
}
