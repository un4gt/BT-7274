//! 循环待机动作与分层关节绘制。时间由宿主推进，静止段只预约下一次动作。

use std::{f32::consts::FRAC_PI_4, time::Duration};

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::{
    paint::{mix, rgb},
    sprite::{Chassis, Format},
    timeline::progress,
};

const INITIAL_WAIT_MS: u64 = 5_000;
const ACTION_MS: u64 = 2_400;
const REST_MS: u64 = 12_000;
const SLOT_MS: u64 = ACTION_MS + REST_MS;
const CYCLE_MS: u64 = SLOT_MS * 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleAction {
    Scan,
    Calibrate,
    ThumbsUp,
}

/// 固定顺序：巡视、握拳校准、竖拇指。初次等待 5 秒，动作之间停留 12 秒。
#[derive(Clone, Copy, Debug, Default)]
pub struct IdleAnimation {
    elapsed_ms: u64,
}

impl IdleAnimation {
    pub fn at(elapsed_ms: u64) -> Self {
        Self { elapsed_ms }
    }

    pub fn action(self) -> Option<IdleAction> {
        if self.elapsed_ms < INITIAL_WAIT_MS {
            return None;
        }
        let cycle = (self.elapsed_ms - INITIAL_WAIT_MS) % CYCLE_MS;
        (cycle % SLOT_MS < ACTION_MS).then_some(match cycle / SLOT_MS {
            0 => IdleAction::Scan,
            1 => IdleAction::Calibrate,
            _ => IdleAction::ThumbsUp,
        })
    }

    /// 动作中按 60 FPS 刷新，静止段直接等待下一次动作，不持续唤醒终端。
    pub fn next_frame_in(self) -> Duration {
        let delay = if self.elapsed_ms < INITIAL_WAIT_MS {
            INITIAL_WAIT_MS - self.elapsed_ms
        } else {
            let local = (self.elapsed_ms - INITIAL_WAIT_MS) % SLOT_MS;
            if local < ACTION_MS {
                16.min(ACTION_MS - local)
            } else {
                SLOT_MS - local
            }
        };
        Duration::from_millis(delay)
    }

    fn pose(self) -> Pose {
        let Some(action) = self.action() else {
            return Pose::default();
        };
        let ms = (self.elapsed_ms - INITIAL_WAIT_MS) % SLOT_MS;
        match action {
            IdleAction::Scan => Pose {
                look: -ease(ms, 0, 500) + 2.0 * ease(ms, 850, 1_550) - ease(ms, 1_850, ACTION_MS),
                ..Pose::default()
            },
            IdleAction::Calibrate | IdleAction::ThumbsUp => {
                let lift = envelope(ms, 100, 750, 1_850, ACTION_MS);
                let prepare = envelope(ms, 0, 120, 160, 350);
                Pose {
                    shoulder: 10.0 * lift - 3.0 * prepare,
                    elbow: 110.0 * lift,
                    hand: ease(ms, 350, 750) * (1.0 - ease(ms, 1_850, 2_200)),
                    grip: if action == IdleAction::Calibrate {
                        envelope(ms, 900, 1_180, 1_450, 1_700)
                    } else {
                        1.0
                    },
                    thumb: if action == IdleAction::ThumbsUp {
                        envelope(ms, 900, 1_200, 1_650, 1_850)
                    } else {
                        0.0
                    },
                    ..Pose::default()
                }
            }
        }
    }
}

#[derive(Default)]
struct Pose {
    look: f32,
    shoulder: f32,
    elbow: f32,
    hand: f32,
    grip: f32,
    thumb: f32,
}

fn ease(ms: u64, start: u64, end: u64) -> f32 {
    let t = progress(ms, start, end);
    (t * t * t * (t * (t * 6.0 - 15.0) + 10.0)).clamp(0.0, 1.0)
}

fn envelope(ms: u64, start: u64, raised: u64, release: u64, end: u64) -> f32 {
    ease(ms, start, raised) * (1.0 - ease(ms, release, end))
}

#[derive(Clone, Copy)]
struct Point(f32, f32);

/// 字符格高约为宽的两倍；在物理坐标中旋转，避免手臂被横向挤扁。
#[derive(Clone, Copy)]
struct Joint {
    pivot: Point,
    offset: Point,
    angle: f32,
}

impl Joint {
    fn map(self, point: Point) -> Point {
        let (sin, cos) = self.angle.sin_cos();
        let x = (point.0 - self.pivot.0) / 2.0;
        let y = point.1 - self.pivot.1;
        Point(
            self.pivot.0 + self.offset.0 + (x * cos - y * sin) * 2.0,
            self.pivot.1 + self.offset.1 + x * sin + y * cos,
        )
    }

    fn unmap(self, point: Point) -> Point {
        let (sin, cos) = self.angle.sin_cos();
        let x = (point.0 - self.pivot.0 - self.offset.0) / 2.0;
        let y = point.1 - self.pivot.1 - self.offset.1;
        Point(
            self.pivot.0 + (x * cos + y * sin) * 2.0,
            self.pivot.1 - x * sin + y * cos,
        )
    }
}

struct Parts {
    head: Rect,
    upper: Rect,
    forearm: Rect,
    hand: Rect,
    shoulder: Point,
    elbow: Point,
    wrist: Point,
}

impl Parts {
    fn for_chassis(chassis: Chassis) -> Self {
        let (head, upper, forearm, hand, shoulder, elbow, wrist) = match chassis.format {
            Format::Detailed => (
                Rect::new(20, 7, 10, 3),
                Rect::new(3, 6, 8, 6),
                Rect::new(3, 12, 6, 3),
                Rect::new(3, 15, 5, 1),
                Point(8.0, 6.0),
                Point(7.0, 11.5),
                Point(5.0, 15.0),
            ),
            Format::Compact => (
                Rect::new(13, 3, 7, 3),
                Rect::new(1, 3, 6, 4),
                Rect::new(1, 7, 5, 2),
                Rect::new(1, 9, 5, 1),
                Point(4.0, 3.0),
                Point(4.0, 6.5),
                Point(3.0, 9.0),
            ),
            Format::Mini => (
                Rect::new(8, 2, 5, 1),
                Rect::new(2, 1, 5, 2),
                Rect::new(2, 3, 5, 1),
                Rect::new(1, 4, 5, 1),
                Point(4.0, 1.0),
                Point(4.0, 3.0),
                Point(3.0, 4.0),
            ),
        };
        let map = |Point(x, y): Point| {
            Point(
                chassis.origin.0 as f32 + x * chassis.scale,
                chassis.origin.1 as f32 + y * chassis.scale,
            )
        };
        let rect = |area: Rect| {
            let start = map(Point(f32::from(area.x), f32::from(area.y)));
            let end = map(Point(f32::from(area.right()), f32::from(area.bottom())));
            Rect::new(
                start.0.round().max(0.0) as u16,
                start.1.round().max(0.0) as u16,
                (end.0.round() - start.0.round()).max(0.0) as u16,
                (end.1.round() - start.1.round()).max(0.0) as u16,
            )
        };
        Self {
            head: rect(head),
            upper: rect(upper),
            forearm: rect(forearm),
            hand: rect(hand),
            shoulder: map(shoulder),
            elbow: map(elbow),
            wrist: map(wrist),
        }
    }
}

pub(crate) fn animate(
    buffer: &mut Buffer,
    chassis: Chassis,
    animation: IdleAnimation,
    background: Color,
    armor: Color,
    sensor: Color,
) {
    let pose = animation.pose();
    if animation.action().is_none() || chassis.scale < 0.7 {
        return;
    }
    let parts = Parts::for_chassis(chassis);
    if pose.look != 0.0 {
        let source = buffer.clone();
        clear(buffer, parts.head, background);
        let shift = pose.look * 1.5 * chassis.scale;
        blit(
            buffer,
            &source,
            parts.head,
            Joint {
                pivot: Point(chassis.eye.0 as f32, chassis.eye.1 as f32),
                offset: Point(shift, 0.0),
                angle: 0.0,
            },
            1.0,
            background,
        );
        let x = (chassis.eye.0 as f32 + shift).round().max(0.0) as u16;
        if let Some(cell) = buffer.cell_mut((x, chassis.eye.1.max(0) as u16)) {
            cell.set_symbol("◉").set_fg(sensor);
        }
    }
    if pose.shoulder == 0.0 && pose.elbow == 0.0 {
        return;
    }
    let source = buffer.clone();
    for area in [parts.upper, parts.forearm, parts.hand] {
        clear(buffer, area, background);
    }
    let shoulder = Joint {
        pivot: parts.shoulder,
        offset: Point(0.0, 0.0),
        angle: pose.shoulder.to_radians(),
    };
    let elbow = shoulder.map(parts.elbow);
    let forearm = Joint {
        pivot: parts.elbow,
        offset: Point(elbow.0 - parts.elbow.0, elbow.1 - parts.elbow.1),
        angle: pose.elbow.to_radians(),
    };
    blit(buffer, &source, parts.upper, shoulder, 1.0, background);
    blit(buffer, &source, parts.forearm, forearm, 1.0, background);
    blit(
        buffer,
        &source,
        parts.hand,
        forearm,
        1.0 - pose.hand,
        background,
    );
    if pose.hand > 0.0 {
        let wrist = forearm.map(parts.wrist);
        draw_hand(buffer, chassis.format, wrist, &pose, background, armor);
    }
}

fn clear(buffer: &mut Buffer, area: Rect, background: Color) {
    let area = area.intersection(buffer.area);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            buffer[(x, y)]
                .set_symbol(" ")
                .set_fg(background)
                .set_bg(background);
        }
    }
}

/// 双线性覆盖率将小于一格的运动转换成亮度变化，不按整格跳动关节。
fn blit(
    target: &mut Buffer,
    source: &Buffer,
    area: Rect,
    joint: Joint,
    opacity: f32,
    background: Color,
) {
    if opacity <= 0.0 || area.is_empty() {
        return;
    }
    let corners = [
        Point(f32::from(area.x) - 1.0, f32::from(area.y) - 1.0),
        Point(f32::from(area.right()), f32::from(area.y) - 1.0),
        Point(f32::from(area.x) - 1.0, f32::from(area.bottom())),
        Point(f32::from(area.right()), f32::from(area.bottom())),
    ]
    .map(|p| joint.map(p));
    let left = corners
        .iter()
        .map(|p| p.0.floor() as i32)
        .min()
        .unwrap()
        .max(i32::from(target.area.x));
    let right = corners
        .iter()
        .map(|p| p.0.ceil() as i32)
        .max()
        .unwrap()
        .min(i32::from(target.area.right()) - 1);
    let top = corners
        .iter()
        .map(|p| p.1.floor() as i32)
        .min()
        .unwrap()
        .max(i32::from(target.area.y));
    let bottom = corners
        .iter()
        .map(|p| p.1.ceil() as i32)
        .max()
        .unwrap()
        .min(i32::from(target.area.bottom()) - 1);
    let base = rgb(background);
    for y in top..=bottom {
        for x in left..=right {
            let p = joint.unmap(Point(x as f32, y as f32));
            let sx = p.0.floor() as i32;
            let sy = p.1.floor() as i32;
            let fx = p.0 - sx as f32;
            let fy = p.1 - sy as f32;
            let mut ink = 0.0;
            let mut strongest = 0.0;
            let mut selected = None;
            let mut shade = [0.0_f32; 3];
            for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
                    let (px, py) = (sx + dx, sy + dy);
                    if px < i32::from(area.x)
                        || py < i32::from(area.y)
                        || px >= i32::from(area.right())
                        || py >= i32::from(area.bottom())
                    {
                        continue;
                    }
                    let Some(cell) = source.cell((px as u16, py as u16)) else {
                        continue;
                    };
                    let weight = wx * wy;
                    let tint = rgb(cell.bg);
                    for (channel, (value, base)) in shade.iter_mut().zip(
                        [tint.0, tint.1, tint.2]
                            .into_iter()
                            .zip([base.0, base.1, base.2]),
                    ) {
                        *channel += (f32::from(value) - f32::from(base)) * weight;
                    }
                    if cell.symbol() != " " && cell.symbol() != "◉" {
                        ink += weight;
                        if weight > strongest {
                            strongest = weight;
                            selected = Some(cell);
                        }
                    }
                }
            }
            let cell = &mut target[(x as u16, y as u16)];
            let (r, g, b) = rgb(cell.bg);
            let tint = |value: u8, delta: f32| {
                (f32::from(value) + delta * opacity).clamp(0.0, 255.0) as u8
            };
            cell.set_bg(Color::Rgb(
                tint(r, shade[0]),
                tint(g, shade[1]),
                tint(b, shade[2]),
            ));
            if let Some(source) = selected
                && ink * opacity > 0.02
            {
                let foreground = mix(cell.bg, source.fg, ink * opacity);
                cell.set_symbol(rotate_glyph(source.symbol(), joint.angle))
                    .set_fg(foreground);
            }
        }
    }
}

fn rotate_glyph(glyph: &str, angle: f32) -> &str {
    let direction = match glyph {
        "_" | "-" => 0.0,
        "\\" => 1.0,
        "|" => 2.0,
        "/" => 3.0,
        _ => return glyph,
    };
    if angle == 0.0 {
        return glyph;
    }
    match ((direction + angle / FRAC_PI_4).round() as i32).rem_euclid(4) {
        0 => "_",
        1 => "\\",
        2 => "|",
        _ => "/",
    }
}

fn draw_hand(
    target: &mut Buffer,
    format: Format,
    wrist: Point,
    pose: &Pose,
    background: Color,
    armor: Color,
) {
    let (open, fist, thumb): (&[&str], &[&str], &[&str]) = match format {
        Format::Detailed => (
            &[" | | | ", " | | | ", "[=====]", " \\___/ "],
            &[" .---. ", "[|||||]", "[#####]", " \\___/ "],
            &[
                "  __   ", " |::|  ", " |::|_ ", "[||_|_]", "[#####]", " \\___/ ",
            ],
        ),
        Format::Compact => (
            &["| | |", "[===]", "[___]"],
            &[".---.", "[|||]", "[___]"],
            &[" _   ", "|:|  ", "|:|_ ", "[###]", "[___]"],
        ),
        Format::Mini => (&["|||", "[_]"], &["[|]", "[_]"], &[" | ", " |]", "[_]"]),
    };
    // 手指先收拢，再展开拇指；用同一腕部锚点交叠，收手时回到原画。
    for (rows, weight) in [
        (open, (1.0 - pose.grip) * (1.0 - pose.thumb)),
        (fist, pose.grip * (1.0 - pose.thumb)),
        (thumb, pose.thumb),
    ] {
        if weight <= 0.0 {
            continue;
        }
        let width = rows.iter().map(|row| row.len()).max().unwrap() as u16;
        let mut patch = Buffer::empty(Rect::new(0, 0, width, rows.len() as u16));
        for (y, row) in rows.iter().enumerate() {
            for (x, glyph) in row.char_indices() {
                patch[(x as u16, y as u16)]
                    .set_char(glyph)
                    .set_fg(armor)
                    .set_bg(background);
            }
        }
        let pivot = Point(f32::from(width - 1) / 2.0, (rows.len() - 1) as f32);
        blit(
            target,
            &patch,
            patch.area,
            Joint {
                pivot,
                offset: Point(wrist.0 - pivot.0, wrist.1 - pivot.1),
                angle: 0.0,
            },
            pose.hand * weight,
            background,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_has_quiet_gaps_and_wraps_to_scan() {
        assert_eq!(IdleAnimation::at(0).action(), None);
        assert_eq!(IdleAnimation::at(0).next_frame_in(), Duration::from_secs(5));
        for (slot, action) in [
            IdleAction::Scan,
            IdleAction::Calibrate,
            IdleAction::ThumbsUp,
            IdleAction::Scan,
        ]
        .into_iter()
        .enumerate()
        {
            let start = INITIAL_WAIT_MS + SLOT_MS * slot as u64;
            assert_eq!(IdleAnimation::at(start - 1).action(), None);
            assert_eq!(IdleAnimation::at(start).action(), Some(action));
            assert_eq!(
                IdleAnimation::at(start).next_frame_in(),
                Duration::from_millis(16)
            );
            assert_eq!(IdleAnimation::at(start + ACTION_MS).action(), None);
            assert_eq!(
                IdleAnimation::at(start + ACTION_MS).next_frame_in(),
                Duration::from_millis(REST_MS)
            );
        }
        assert!(!IdleAnimation::at(u64::MAX).next_frame_in().is_zero());
    }
}
