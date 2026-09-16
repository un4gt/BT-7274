use ratatui::style::Color;

use super::{
    paint::{AMBER, BLACK, COLD, Canvas, DIM, RUST, STEEL, hash, mix, random},
    sprite::Chassis,
    timeline::{EYE_MS, IGNITION_MS, IMPACT_MS, Phase, Timeline, progress, smoothstep},
};

const CAMERA_SHAKE_MS: u64 = 330;

pub fn atmosphere(canvas: &mut Canvas, time: Timeline, ground: i32) {
    // Sparse depth cues turn into streaks as the drop accelerates.
    if time.ms < 220 {
        return;
    }
    let tick = time.ms as f32 / 1_000.0;
    for i in 0..28_u32 {
        let x = (random(i * 7 + 3) * canvas.width as f32) as i32;
        let y = ((random(i * 7 + 4) * ground as f32 + tick * 1.6) % ground.max(1) as f32) as i32;
        let color = mix(BLACK, DIM, 0.25 + random(i + 90) * 0.35);
        canvas.put(x, y, '.', color);
        if time.phase == Phase::Falling && i % 3 == 0 {
            canvas.put(x, y - 1, '|', mix(BLACK, STEEL, 0.22));
            canvas.put(x, y - 2, ':', mix(BLACK, DIM, 0.3));
        }
    }
}

pub fn ground(canvas: &mut Canvas, time: Timeline, ground: i32) {
    let fade = progress(time.ms, 360, 780);
    let center = canvas.width / 2;
    for x in 3..canvas.width - 3 {
        let distance = (x - center).unsigned_abs() as f32 / (canvas.width as f32 * 0.5);
        let color = mix(BLACK, DIM, (1.0 - distance * 0.8) * fade);
        canvas.put(
            x,
            ground,
            if hash(x as u32) % 7 < 2 { '.' } else { '_' },
            color,
        );
        if x % 9 == 0 {
            canvas.put(x, ground + 1, '.', mix(BLACK, DIM, fade * 0.5));
        }
    }
    if let Some(age) = time.impact_age() {
        let radius = (age * 110.0).min(19.0) as i32;
        for side in [-1, 1] {
            for x in 0..radius {
                if !hash(x as u32 + 91).is_multiple_of(3) {
                    canvas.put(
                        center + x * side,
                        ground + (x % 4 == 0) as i32,
                        if x % 3 == 0 { '/' } else { '_' },
                        mix(DIM, RUST, 0.24),
                    );
                }
            }
        }
    }
}

pub fn thrusters(canvas: &mut Canvas, time: Timeline, chassis: Chassis, ground: i32) {
    if !(IGNITION_MS..IMPACT_MS + 50).contains(&time.ms) {
        return;
    }
    let decay = 1.0 - progress(time.ms, IMPACT_MS, IMPACT_MS + 50);
    let tick = (time.ms / 24) as u32;
    for (side, (x, y)) in [(-1, chassis.left_nozzle), (1, chassis.right_nozzle)] {
        for depth in 0..12 {
            let width = 1 + depth / 3;
            for column in 0..width {
                let seed = tick.wrapping_mul(271) + (depth * 13 + column + 60 * (side + 1)) as u32;
                if random(seed) > decay * (1.0 - depth as f32 / 16.0) {
                    continue;
                }
                let px = x + side * (depth / 2 + column + 1);
                let py = y + depth + 1;
                if py > ground {
                    continue;
                }
                let ch = if depth < 2 || hash(seed).is_multiple_of(9) {
                    '*'
                } else if side < 0 {
                    '/'
                } else {
                    '\\'
                };
                let color = if depth < 3 {
                    AMBER
                } else {
                    mix(RUST, BLACK, depth as f32 / 15.0)
                };
                canvas.put(px, py, ch, color);
                canvas.tint_background(px, py, RUST, 0.13 * decay);
            }
        }
    }
    if chassis.bottom > ground - 5 {
        for i in 0..18_u32 {
            let side = if i % 2 == 0 { -1 } else { 1 };
            let spread = (random(i + tick * 33) * 24.0) as i32;
            canvas.put(
                canvas.width / 2 + side * (10 + spread),
                ground - (i % 2) as i32,
                '.',
                mix(BLACK, RUST, decay * 0.5),
            );
        }
    }
}

pub fn shockwave(canvas: &mut Canvas, time: Timeline, ground: i32) {
    let Some(age) = time.impact_age() else {
        return;
    };
    if age > 0.5 {
        return;
    }
    let center = canvas.width / 2;
    for ring in 0..2 {
        let t = age - ring as f32 * 0.065;
        if t < 0.0 {
            continue;
        }
        let radius = 4.0 + t * canvas.width as f32 * 2.8;
        let fade = (1.0 - t / 0.48).max(0.0);
        for side in [-1, 1] {
            let x = center + (radius as i32) * side;
            canvas.text(
                x - 3,
                ground,
                if side < 0 { "<====" } else { "====>" },
                mix(BLACK, AMBER, fade),
            );
            canvas.text(x - 2, ground - 1, "_.-", mix(BLACK, RUST, fade * 0.7));
            canvas.text(x - 1, ground + 1, "---", mix(BLACK, STEEL, fade * 0.7));
        }
    }
}

pub fn dust(canvas: &mut Canvas, time: Timeline, ground: i32, foreground: bool) {
    let Some(age) = time.impact_age() else {
        return;
    };
    if age > 1.1 {
        return;
    }
    for i in 0..140_u32 {
        if (i % 3 == 0) != foreground {
            continue;
        }
        let side = if i % 2 == 0 { -1.0 } else { 1.0 };
        let life = 0.35 + random(i * 11 + 1) * 0.75;
        if age > life {
            continue;
        }
        let t = age + 0.025;
        let start = 3.0 + random(i * 11 + 2) * 14.0;
        let speed = 15.0 + random(i * 11 + 3) * 55.0;
        let vertical_speed = 2.0 + random(i * 11 + 4) * 15.0;
        let x = canvas.width as f32 / 2.0 + side * (start + speed * t);
        let y = ground as f32 - vertical_speed * t + 13.0 * t * t;
        if y > ground as f32 + 1.0 {
            continue;
        }
        let glyphs = if foreground {
            &['*', '.', '`', ':', ','][..]
        } else {
            &['.', ':', ';', '~', '\'', '`'][..]
        };
        let glyph = glyphs[(hash(i + 7) as usize) % glyphs.len()];
        let fade = (1.0 - age / life).powf(0.65);
        canvas.put(
            x.round() as i32,
            y.round() as i32,
            glyph,
            mix(BLACK, if i % 11 == 0 { AMBER } else { RUST }, fade * 0.9),
        );
    }
}

pub fn eye_state(ms: u64) -> (char, Color, f32) {
    let age = ms.saturating_sub(EYE_MS);
    if ms < EYE_MS {
        ('●', Color::Rgb(44, 19, 15), 0.0)
    } else if age < 120 {
        (
            '●',
            mix(
                Color::Rgb(44, 19, 15),
                Color::Rgb(157, 75, 32),
                progress(age, 0, 100),
            ),
            0.12,
        )
    } else if age < 280 {
        (
            '◉',
            mix(
                Color::Rgb(157, 75, 32),
                Color::Rgb(255, 158, 100),
                progress(age, 120, 240),
            ),
            0.42,
        )
    } else {
        (
            '◉',
            mix(Color::Rgb(255, 158, 100), AMBER, progress(age, 280, 380)),
            0.75,
        )
    }
}

pub fn eye(canvas: &mut Canvas, time: Timeline, chassis: Chassis) {
    if time.is_airborne() && chassis.scale < 0.7 {
        return;
    }
    let (glyph, color, glow) = eye_state(time.ms);
    let (x, y) = chassis.eye;
    // A separate, elliptical light pass illuminates the surrounding sensor recess.
    for dy in -2..=2_i32 {
        for dx in -5..=5_i32 {
            let distance = (dx as f32 / 4.5).powi(2) + (dy as f32 / 1.6).powi(2);
            let falloff = (-distance * 1.7).exp() * glow;
            canvas.tint_background(x + dx, y + dy, RUST, falloff * 0.45);
        }
    }
    if glow > 0.4 {
        canvas.put(x - 2, y, '[', mix(STEEL, AMBER, glow * 0.55));
        canvas.put(x + 2, y, ']', mix(STEEL, AMBER, glow * 0.55));
    }
    canvas.put(x, y, glyph, color);
}

/// Camera translation and scanline displacement are applied to the entire frame.
pub fn impact_camera(canvas: &mut Canvas, time: Timeline) {
    let Some(age) = time.impact_age() else {
        return;
    };
    let age_ms = time.ms.saturating_sub(IMPACT_MS);
    if age_ms >= CAMERA_SHAKE_MS {
        return;
    }
    // Hold each kick for about two frames so the recoil reads even on slower terminals.
    let tick = (age_ms / 30) as usize;
    let kicks = [
        (10, 4),
        (-9, -3),
        (8, 3),
        (-7, -3),
        (6, 2),
        (-5, 2),
        (4, -1),
        (-3, 1),
        (2, -1),
        (-1, 0),
        (0, 0),
    ];
    let (dx, dy) = kicks[tick.min(kicks.len() - 1)];
    let dx = (dx as f32 * (canvas.width as f32 / 110.0).min(1.0)).round() as i32;
    let dy = (dy as f32 * (canvas.height as f32 / 42.0).min(1.0)).round() as i32;
    let source = canvas.buffer.clone();
    let flash = (1.0 - age / 0.070).max(0.0) + (1.0 - (age - 0.080).abs() / 0.025).max(0.0) * 0.28;
    let decay = 1.0 - smoothstep(age_ms as f32 / CAMERA_SHAKE_MS as f32);
    for y in 0..canvas.height {
        let band = hash(y as u32 / 3 + tick as u32 * 31);
        let tear = if age < 0.180 && band.is_multiple_of(4) {
            let direction = if band & 1_024 == 0 { -1.0 } else { 1.0 };
            (direction * (8 + band % 9) as f32 * decay).round() as i32
        } else {
            0
        };
        let ripple = ((y as f32 * 0.8 + age * 110.0).sin() * 3.0 * decay).round() as i32;
        for x in 0..canvas.width {
            let source_x = x - dx - tear - ripple;
            let source_y = y - dy;
            let cell = &mut canvas.buffer[(x as u16, y as u16)];
            if source_x >= 0 && source_x < canvas.width && source_y >= 0 && source_y < canvas.height
            {
                *cell = source[(source_x as u16, source_y as u16)].clone();
            } else {
                cell.reset();
                cell.set_bg(BLACK).set_fg(DIM);
            }
            cell.set_bg(mix(cell.bg, AMBER, flash * 0.32));
            if !matches!(cell.symbol(), "●" | "◉") {
                cell.set_fg(mix(cell.fg, AMBER, flash * 0.80));
            }
            // Brief displaced cyan traces make the impact read as a damaged feed.
            if age < 0.140 && cell.symbol() == " " {
                let ghost_x = x + dx;
                if ghost_x >= 0 && ghost_x < canvas.width {
                    let ghost = &source[(ghost_x as u16, y as u16)];
                    if !matches!(ghost.symbol(), " " | "●" | "◉") && band.is_multiple_of(3) {
                        cell.set_symbol(ghost.symbol())
                            .set_fg(mix(BLACK, COLD, 0.65 * decay));
                    }
                }
            }
        }
    }
    if age < 0.120 {
        for stripe in 0..3 {
            let y = (canvas.height / 4 + stripe * 5 + tick as i32 * 2) % canvas.height;
            for x in 0..canvas.width {
                if hash(x as u32 + tick as u32 * 7).is_multiple_of(3)
                    && !matches!(canvas.buffer[(x as u16, y as u16)].symbol(), "●" | "◉")
                {
                    canvas.put(
                        x,
                        y,
                        ['_', '/', ':', '-', '='][x as usize % 5],
                        mix(BLACK, AMBER, 0.75 * decay),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_has_its_own_exact_color_stages() {
        assert_eq!(eye_state(IMPACT_MS).0, '●');
        assert_eq!(eye_state(EYE_MS + 100).1, Color::Rgb(157, 75, 32));
        assert_eq!(eye_state(EYE_MS + 240).0, '◉');
        assert_eq!(eye_state(EYE_MS + 240).1, Color::Rgb(255, 158, 100));
        assert_eq!(eye_state(EYE_MS + 380).1, Color::Rgb(255, 184, 108));
    }

    #[test]
    fn camera_has_a_large_recoil_then_damps_to_a_stable_frame() {
        // 24 行终端将大屏的 4 行首震缩至 2 行，保留往复方向和衰减。
        for (age, expected_y) in [(0, 12), (30, 8), (210, 11)] {
            let mut canvas = Canvas::new(80, 24);
            canvas.text(24, 10, "HUD", STEEL);
            impact_camera(&mut canvas, Timeline::at(IMPACT_MS + age));
            assert!((0..80).any(|x| canvas.buffer[(x, expected_y)].symbol() == "H"));
        }
        let mut settled = Canvas::new(80, 24);
        settled.text(4, 2, "HUD", STEEL);
        let before = settled.buffer.clone();
        impact_camera(&mut settled, Timeline::at(IMPACT_MS + CAMERA_SHAKE_MS));
        assert_eq!(before, settled.buffer);
    }

    #[test]
    fn thrusters_stay_lit_through_the_extended_burn_and_shut_off_after_contact() {
        let chassis = Chassis {
            eye: (40, 8),
            left_nozzle: (25, 8),
            right_nozzle: (55, 8),
            bottom: 18,
            scale: 1.0,
        };
        for ms in [IGNITION_MS, 950, 1_200, IMPACT_MS - 1] {
            let mut canvas = Canvas::new(80, 24);
            thrusters(&mut canvas, Timeline::at(ms), chassis, 20);
            for range in [0..40_u16, 40..80] {
                assert!(range.into_iter().any(|x| {
                    (0..24).any(|y| matches!(canvas.buffer[(x, y)].symbol(), "/" | "\\" | "*"))
                }));
            }
        }
        let mut canvas = Canvas::new(80, 24);
        thrusters(&mut canvas, Timeline::at(IMPACT_MS + 50), chassis, 20);
        assert!(
            canvas
                .buffer
                .content
                .iter()
                .all(|cell| cell.symbol() == " ")
        );
    }
}
