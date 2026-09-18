use ratatui::{
    buffer::{Buffer, Cell},
    layout::{Position, Rect},
    style::Color,
    widgets::Widget,
};

use crate::{
    Idle,
    paint::{AMBER, BLACK, mix, rgb},
    portrait::{Portrait, Sample},
    timeline::{ONLINE_MS, progress},
};

const HANDOFF_MS: u64 = ONLINE_MS;
const ENVIRONMENT_END_MS: u64 = HANDOFF_MS + 360;
const BACKGROUND_END_MS: u64 = HANDOFF_MS + 650;
const MOTION_END_MS: u64 = HANDOFF_MS + 850;
pub const STARTUP_DURATION_MS: u64 = HANDOFF_MS + 900;

/// 宿主提供最终机体区域及样式；过渡期间由本模块绘制同一个机体。
pub struct Handoff {
    pub area: Rect,
    pub idle: Option<Idle>,
}

/// 五次缓动让速度和加速度在两端归零，避免起步、停靠时突然变速。
fn ease(ms: u64, start: u64, end: u64) -> f32 {
    let t = progress(ms, start, end);
    (t * t * t * (t * (t * 6.0 - 15.0) + 10.0)).clamp(0.0, 1.0)
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// 机体持续可见并平滑移入聊天区域；面板在最终位置渐显，不按字符格推动边框。
/// 宿主应先画不含 idle 机体的 UI；完成帧由这里补上同一份 Idle 纹理。
pub fn render_startup(target: &mut Buffer, ms: u64, panels: &[Rect], handoff: Handoff) {
    if target.area.is_empty() {
        return;
    }
    if ms >= STARTUP_DURATION_MS {
        if let Some(idle) = handoff.idle {
            idle.render(handoff.area, target);
        }
        return;
    }
    let area = target.area;
    if ms < HANDOFF_MS {
        let scene = crate::render(area.width, area.height, ms);
        target.content.clone_from(&scene.content);
        return;
    }

    let ui = target.clone();
    let environment = crate::backdrop(area.width, area.height, ms);
    let background = ease(ms, HANDOFF_MS, BACKGROUND_END_MS);
    let environment_opacity = 1.0 - ease(ms, HANDOFF_MS, ENVIRONMENT_END_MS);
    for (index, cell) in target.content.iter_mut().enumerate() {
        *cell = environment.content[index].clone();
        let bg = mix(BLACK, ui.content[index].bg, background);
        let fg = mix(bg, cell.fg, environment_opacity);
        cell.set_bg(bg).set_fg(fg);
        if environment_opacity == 0.0 {
            cell.set_symbol(" ");
        }
    }
    reveal_panels(target, &ui, ms, panels, environment_opacity);

    let from = Portrait::intro(area.width, area.height);
    let to = handoff
        .idle
        .filter(|_| !handoff.area.is_empty())
        .map(|idle| idle.portrait(handoff.area.width, handoff.area.height));
    let t = ease(ms, HANDOFF_MS, MOTION_END_MS);
    let from_eye = (
        from.eye.0 + f32::from(area.x),
        from.eye.1 + f32::from(area.y),
    );
    let end_eye = to.as_ref().map_or(
        (
            f32::from(handoff.area.x) + f32::from(handoff.area.width) * 0.5,
            f32::from(handoff.area.y) + f32::from(handoff.area.height) * 0.3,
        ),
        |portrait| {
            (
                portrait.eye.0 + f32::from(handoff.area.x),
                portrait.eye.1 + f32::from(handoff.area.y),
            )
        },
    );
    let eye = (
        lerp(from_eye.0, end_eye.0, t),
        lerp(from_eye.1, end_eye.1, t),
    );
    let end_height = to.as_ref().map_or(
        (f32::from(handoff.area.height) * 0.8).clamp(1.0, from.height),
        |portrait| portrait.height,
    );
    let height = lerp(from.height, end_height, t);
    let morph = if to.is_some() {
        ease(ms, HANDOFF_MS + 100, MOTION_END_MS)
    } else {
        0.0
    };
    let opacity = if to.is_some() {
        1.0
    } else {
        1.0 - ease(ms, HANDOFF_MS + 150, MOTION_END_MS)
    };
    if opacity <= 0.0 {
        return;
    }

    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let dx = f32::from(x) - eye.0;
            let dy = f32::from(y) - eye.1;
            let source = from.sample(
                from.eye.0 + dx * from.height / height,
                from.eye.1 + dy * from.height / height,
            );
            let destination = to.as_ref().and_then(|portrait| {
                portrait.sample(
                    portrait.eye.0 + dx * portrait.height / height,
                    portrait.eye.1 + dy * portrait.height / height,
                )
            });
            blend_portraits(&mut target[(x, y)], source, destination, morph, opacity);
        }
    }
    // 传感器只绘制一次：对齐锚点后变化造型，不产生双眼、消失或位置跳跃。
    let eye_position = (eye.0.round() as i32, eye.1.round() as i32);
    if eye_position.0 >= 0
        && eye_position.1 >= 0
        && let Some(cell) = target.cell_mut((eye_position.0 as u16, eye_position.1 as u16))
    {
        let end_color = to
            .as_ref()
            .and_then(|portrait| {
                portrait
                    .buffer
                    .cell((portrait.eye.0 as u16, portrait.eye.1 as u16))
            })
            .map_or(AMBER, |cell| cell.fg);
        let fg = mix(cell.bg, mix(AMBER, end_color, morph), opacity);
        cell.set_symbol("◉").set_fg(fg);
    }
}

fn reveal_panels(
    target: &mut Buffer,
    ui: &Buffer,
    ms: u64,
    panels: &[Rect],
    environment_opacity: f32,
) {
    for y in ui.area.y..ui.area.bottom() {
        for x in ui.area.x..ui.area.right() {
            let index = panels
                .iter()
                .position(|panel| panel.contains(Position::new(x, y)));
            // 面板只错开一帧显现。终端只能整格移动长边框，持续位移会造成阶梯与拖尾感。
            let delay = index.unwrap_or(0).min(4) as u64 * 16;
            let start = HANDOFF_MS + delay;
            let cell = &mut target[(x, y)];
            let source = &ui[(x, y)];
            let opacity = if is_border(source.symbol()) {
                ease(ms, start, start + 420)
            } else {
                ease(ms, start + 60, start + 560)
            };
            if opacity == 0.0 {
                continue;
            }
            // 背景已统一渐变；再次按文字透明度叠加会在浅色主题产生亮框。
            let bg = cell.bg;
            if opacity == 1.0 {
                *cell = source.clone();
                cell.set_bg(bg);
            } else if source.symbol() != " "
                && (cell.symbol() == " " || opacity >= environment_opacity)
            {
                *cell = source.clone();
                cell.set_bg(bg).set_fg(mix(bg, source.fg, opacity));
            }
        }
    }
}

fn blend_portraits(
    cell: &mut Cell,
    from: Option<Sample<'_>>,
    to: Option<Sample<'_>>,
    morph: f32,
    opacity: f32,
) {
    let from_weight = from.map_or(0.0, |sample| sample.coverage * (1.0 - morph));
    let to_weight = to.map_or(0.0, |sample| sample.coverage * morph);
    let weight = from_weight + to_weight;
    if weight <= 0.0 {
        return;
    }
    let from_ink = from.map_or(0.0, |sample| sample.ink * (1.0 - morph));
    let to_ink = to.map_or(0.0, |sample| sample.ink * morph);
    let source = if from_ink > to_ink || (from_ink == to_ink && from_weight >= to_weight) {
        from.unwrap().cell
    } else {
        to.unwrap().cell
    };
    let ink = from_ink + to_ink;
    let fg = match (from, to) {
        (Some(a), Some(b)) => mix(
            a.cell.fg,
            b.cell.fg,
            if ink > 0.0 {
                to_ink / ink
            } else {
                to_weight / weight
            },
        ),
        _ => source.fg,
    };
    let alpha = weight.min(1.0) * opacity;
    // 双线性采样不复制传感器，由公共运动锚点单独绘制。
    let symbol = if source.symbol() == "◉" {
        " "
    } else {
        source.symbol()
    };
    let from_shade = from.map_or([0.0; 3], |sample| sample.shade);
    let to_shade = to.map_or([0.0; 3], |sample| sample.shade);
    let (r, g, b) = rgb(cell.bg);
    let [r, g, b] = std::array::from_fn(|i| {
        (f32::from([r, g, b][i]) + lerp(from_shade[i], to_shade[i], morph) * opacity)
            .round()
            .clamp(0.0, 255.0) as u8
    });
    cell.set_bg(Color::Rgb(r, g, b));
    if alpha >= 0.5 || cell.symbol() == " " {
        // 内部留白参与遮挡，但不能增加相邻字符亮度，否则会出现不透明的重影。
        let fg = mix(
            cell.bg,
            fg,
            if symbol == " " { alpha } else { ink * opacity },
        );
        cell.set_symbol(symbol).set_fg(fg);
    } else {
        cell.set_fg(mix(cell.fg, fg, alpha));
    }
}

fn is_border(symbol: &str) -> bool {
    matches!(
        symbol,
        "─" | "│"
            | "╭"
            | "╮"
            | "╰"
            | "╯"
            | "┌"
            | "┐"
            | "└"
            | "┘"
            | "├"
            | "┤"
            | "┬"
            | "┴"
            | "┼"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        style::{Color, Style},
        widgets::{Block, BorderType},
    };

    fn chat(area: Rect) -> Buffer {
        let mut buffer = Buffer::empty(area);
        Block::bordered()
            .border_type(BorderType::Rounded)
            .style(
                Style::default()
                    .fg(Color::Rgb(50, 210, 190))
                    .bg(Color::Rgb(12, 24, 27)),
            )
            .render(area, &mut buffer);
        buffer
    }

    #[test]
    fn sensor_remains_lit_and_moves_at_most_one_cell_per_frame() {
        for (width, height, destination) in [
            (120, 42, Rect::new(30, 4, 87, 27)),
            (80, 24, Rect::new(26, 4, 51, 9)),
        ] {
            let area = Rect::new(0, 0, width, height);
            let idle = Idle::new(Color::Rgb(12, 24, 27), Color::Rgb(103, 137, 133), AMBER);
            let mut previous: Option<(i32, i32)> = None;
            for ms in (HANDOFF_MS..=STARTUP_DURATION_MS).step_by(16) {
                let mut frame = chat(area);
                render_startup(
                    &mut frame,
                    ms,
                    &[area],
                    Handoff {
                        area: destination,
                        idle: Some(idle),
                    },
                );
                let sensors: Vec<_> = frame
                    .content
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| cell.symbol() == "◉")
                    .collect();
                assert_eq!(sensors.len(), 1, "sensor count at {ms}ms, {width}x{height}");
                let (index, cell) = sensors[0];
                assert_eq!(cell.fg, AMBER, "sensor dimmed at {ms}ms");
                let position = (
                    (index % usize::from(width)) as i32,
                    (index / usize::from(width)) as i32,
                );
                if let Some(previous) = previous {
                    assert!(
                        (position.0 - previous.0).abs() <= 1
                            && (position.1 - previous.1).abs() <= 1,
                        "sensor jumped at {ms}ms"
                    );
                }
                previous = Some(position);
            }
        }
    }

    #[test]
    fn handoff_starts_on_the_existing_scene_and_finishes_on_the_actual_ui() {
        let area = Rect::new(2, 3, 80, 24);
        let destination = Rect::new(27, 7, 51, 9);
        let idle = Idle::new(Color::Rgb(12, 24, 27), Color::Rgb(103, 137, 133), AMBER);
        let mut first = chat(area);
        render_startup(
            &mut first,
            HANDOFF_MS,
            &[area],
            Handoff {
                area: destination,
                idle: Some(idle),
            },
        );
        assert_eq!(first.content, crate::render(80, 24, HANDOFF_MS).content);
        for idle in [None, Some(idle)] {
            let mut expected = chat(area);
            if let Some(idle) = idle {
                idle.render(destination, &mut expected);
            }
            for ms in [STARTUP_DURATION_MS, u64::MAX] {
                let mut frame = chat(area);
                render_startup(
                    &mut frame,
                    ms,
                    &[area],
                    Handoff {
                        area: destination,
                        idle,
                    },
                );
                assert_eq!(frame, expected);
            }
        }
    }

    #[test]
    fn panels_fade_in_place_and_finish_within_650_ms() {
        let area = Rect::new(3, 2, 100, 36);
        let panels = [
            Rect::new(3, 2, 100, 3),
            Rect::new(3, 5, 24, 30),
            Rect::new(28, 5, 75, 22),
            Rect::new(28, 27, 75, 8),
            Rect::new(3, 35, 100, 3),
        ];
        for (bg, fg) in [
            (Color::Rgb(12, 24, 27), Color::Rgb(224, 239, 237)),
            (Color::Rgb(255, 255, 255), Color::Rgb(25, 38, 37)),
        ] {
            let style = Style::default().bg(bg).fg(fg);
            let mut blank = Buffer::empty(area);
            blank.set_style(area, Style::default().bg(bg).fg(bg));
            let mut ui = blank.clone();
            for panel in panels {
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .title(" panel ")
                    .style(style)
                    .render(panel, &mut ui);
            }
            let (r, g, b) = rgb(bg);
            let contrast = |color| {
                let (fr, fg, fb) = rgb(color);
                u16::from(fr.abs_diff(r)) + u16::from(fg.abs_diff(g)) + u16::from(fb.abs_diff(b))
            };
            let mut previous = vec![0; ui.content.len()];
            for ms in (HANDOFF_MS..HANDOFF_MS + 650).step_by(16) {
                let mut frame = blank.clone();
                reveal_panels(&mut frame, &ui, ms, &panels, 0.0);
                for (index, (cell, expected)) in frame.content.iter().zip(&ui.content).enumerate() {
                    if cell.symbol() != " " {
                        assert_eq!(cell.symbol(), expected.symbol(), "shifted panel at {ms}ms");
                        let current = contrast(cell.fg);
                        assert!(current >= previous[index], "panel faded back at {ms}ms");
                        previous[index] = current;
                    }
                }
                if ms >= HANDOFF_MS + 150 {
                    for panel in panels {
                        let corner = &frame[(panel.x, panel.y)];
                        assert_eq!(corner.symbol(), "╭");
                        assert!(contrast(corner.fg) > 0, "panel delayed at {ms}ms");
                    }
                }
            }
            let mut settled = blank.clone();
            reveal_panels(&mut settled, &ui, HANDOFF_MS + 650, &panels, 0.0);
            assert_eq!(settled, ui);
        }
    }

    #[test]
    fn light_theme_has_no_bright_background_boxes_around_revealing_borders() {
        let area = Rect::new(0, 0, 80, 24);
        let mut ui = Buffer::empty(area);
        Block::bordered()
            .style(Style::default().bg(Color::White).fg(Color::Black))
            .render(area, &mut ui);
        // 使用实际 RGB 底色，以覆盖浅色主题从深色开场转到白色 UI 的过程。
        for cell in &mut ui.content {
            cell.set_bg(Color::Rgb(255, 255, 255));
        }
        for ms in (HANDOFF_MS..=STARTUP_DURATION_MS).step_by(16) {
            let mut frame = ui.clone();
            render_startup(&mut frame, ms, &[area], Handoff { area, idle: None });
            assert_eq!(
                frame[(79, 12)].bg,
                frame[(77, 12)].bg,
                "bright border at {ms}ms"
            );
        }
    }
}
