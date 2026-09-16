//! 机体纹理及传感器锚点。几何运动与造型细节分离，缩放时以锚点对齐。

use ratatui::{
    buffer::{Buffer, Cell},
    style::Color,
};

use crate::{
    effects,
    paint::{BLACK, Canvas, DIM, rgb},
    sprite::{self, Chassis},
    timeline::{DURATION_MS, Timeline},
};

pub(crate) struct Portrait {
    pub buffer: Buffer,
    pub eye: (f32, f32),
    pub height: f32,
    pub background: Color,
    mask: Vec<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct Sample<'a> {
    pub cell: &'a Cell,
    pub coverage: f32,
    pub ink: f32,
    /// 背景只采样相对于底色的光照差，避免随字符带入深色或浅色矩形。
    pub shade: [f32; 3],
}

impl Portrait {
    pub fn from_canvas(canvas: Canvas, chassis: Chassis) -> Self {
        let mask: Vec<_> = canvas
            .buffer
            .content
            .iter()
            .map(|cell| cell.symbol() != " " || cell.fg != DIM || cell.bg != BLACK)
            .collect();
        let mut top = canvas.height;
        let mut bottom = 0;
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                if mask[(y * canvas.width + x) as usize] {
                    top = top.min(y);
                    bottom = bottom.max(y);
                }
            }
        }
        Self {
            buffer: canvas.buffer,
            eye: (chassis.eye.0 as f32, chassis.eye.1 as f32),
            height: (bottom - top + 1).max(1) as f32,
            background: BLACK,
            mask,
        }
    }

    pub fn intro(width: u16, height: u16) -> Self {
        let mut canvas = Canvas::new(width, height);
        let (detailed, ground) = crate::scene_metrics(width, height);
        let time = Timeline::at(DURATION_MS);
        let chassis = sprite::draw(&mut canvas, time, ground, detailed);
        effects::eye(&mut canvas, time, chassis);
        Self::from_canvas(canvas, chassis)
    }

    /// 双线性覆盖率让边缘在相邻字符格之间过渡，避免只按整格舍入移动。
    pub fn sample(&self, x: f32, y: f32) -> Option<Sample<'_>> {
        let left = x.floor() as i32;
        let top = y.floor() as i32;
        let fx = x - left as f32;
        let fy = y - top as f32;
        let mut coverage = 0.0_f32;
        let mut ink = 0.0_f32;
        let mut shade = [0.0_f32; 3];
        let (r, g, b) = rgb(self.background);
        let base = [r, g, b];
        let mut strongest = 0.0_f32;
        let mut selected = None;
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
                let (sx, sy) = (left + dx, top + dy);
                if sx < 0
                    || sy < 0
                    || sx >= i32::from(self.buffer.area.width)
                    || sy >= i32::from(self.buffer.area.height)
                {
                    continue;
                }
                let index = sy as usize * usize::from(self.buffer.area.width) + sx as usize;
                if !self.mask[index] {
                    continue;
                }
                let weight = wx * wy;
                coverage += weight;
                let cell = &self.buffer.content[index];
                if cell.symbol() != " " && cell.symbol() != "◉" {
                    ink += weight;
                }
                let (r, g, b) = rgb(cell.bg);
                for (channel, (tint, base)) in shade.iter_mut().zip([r, g, b].into_iter().zip(base))
                {
                    *channel += (f32::from(tint) - f32::from(base)) * weight;
                }
                let score = weight * if cell.symbol() == " " { 0.01 } else { 1.0 };
                if score > strongest {
                    strongest = score;
                    selected = Some(cell);
                }
            }
        }
        selected.map(|cell| Sample {
            cell,
            coverage: coverage.min(1.0),
            ink: ink.min(1.0),
            shade,
        })
    }
}
