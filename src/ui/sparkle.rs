//! 输入框星空：与草稿共存、持续闪烁，仅在开关时淡入淡出。
//! 对齐 Codex rust-v0.154.0 的 chat_composer/sparkle.rs；
//! #44879 的输入即淡出、15 秒后结束策略不属于该版本。

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use unicode_width::UnicodeWidthStr;

use super::theme::Palette;
use crate::editor::EditorViewport;

const FRAME_TICK: Duration = Duration::from_millis(150);
const FADE_IN: Duration = Duration::from_secs(1);
const FADE_OUT: Duration = Duration::from_millis(75);
const FADE_FRAME_TICK: Duration = Duration::from_millis(25);
const DOTS: [&str; 8] = ["⠁", "⠂", "⠄", "⠈", "⠐", "⠠", "⡀", "⢀"];

#[derive(Debug, Clone, Copy)]
enum Phase {
    Waiting,
    Visible {
        since: Instant,
        elapsed: Duration,
    },
    Fading {
        started: Instant,
        elapsed: Duration,
        start_visibility: f32,
    },
    Finished,
}

struct SparkleFrame {
    elapsed: Duration,
    visibility: f32,
    next: Duration,
}

#[derive(Debug)]
pub(crate) struct Sparkle {
    enabled: bool,
    terminal_focused: bool,
    phase: Cell<Phase>,
    next_frame: Cell<Option<Instant>>,
}

impl Sparkle {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            terminal_focused: true,
            phase: Cell::new(if enabled {
                Phase::Waiting
            } else {
                Phase::Finished
            }),
            next_frame: Cell::new(None),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool, now: Instant) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if enabled {
            self.phase.set(Phase::Waiting);
        } else {
            self.fade_out(now);
        }
        self.next_frame.set(None);
    }

    pub fn set_terminal_focus(&mut self, focused: bool) {
        self.terminal_focused = focused;
        self.next_frame.set(None);
    }

    pub fn next_frame(&self) -> Option<Instant> {
        self.next_frame.get()
    }

    fn fade_out(&self, now: Instant) {
        match self.phase.get() {
            Phase::Waiting => self.phase.set(Phase::Finished),
            Phase::Visible { elapsed, .. } => self.phase.set(Phase::Fading {
                started: now,
                elapsed,
                // 从最后实际画出的亮度淡出，避免淡入期间关闭导致闪亮。
                start_visibility: fade_in_visibility(elapsed),
            }),
            Phase::Fading { .. } | Phase::Finished => {}
        }
    }

    fn frame(&self, now: Instant) -> Option<SparkleFrame> {
        match self.phase.get() {
            Phase::Waiting => {
                self.phase.set(Phase::Visible {
                    since: now,
                    elapsed: Duration::ZERO,
                });
                self.frame(now)
            }
            Phase::Visible { since, .. } => {
                let elapsed = now.saturating_duration_since(since);
                self.phase.set(Phase::Visible { since, elapsed });
                Some(SparkleFrame {
                    elapsed,
                    visibility: fade_in_visibility(elapsed),
                    next: if elapsed < FADE_IN {
                        FADE_FRAME_TICK.min(FADE_IN - elapsed)
                    } else {
                        FRAME_TICK
                    },
                })
            }
            Phase::Fading {
                started,
                elapsed,
                start_visibility,
            } => {
                let remaining = FADE_OUT.saturating_sub(now.saturating_duration_since(started));
                if remaining.is_zero() || start_visibility == 0.0 {
                    self.phase.set(Phase::Finished);
                    return None;
                }
                Some(SparkleFrame {
                    // 淡出时冻结星点位置和闪烁相位，不产生新星点。
                    elapsed,
                    visibility: start_visibility * remaining.as_secs_f32() / FADE_OUT.as_secs_f32(),
                    next: FADE_FRAME_TICK.min(remaining),
                })
            }
            Phase::Finished => None,
        }
    }

    pub fn render(
        &self,
        area: Rect,
        viewport: &EditorViewport,
        focused: bool,
        buf: &mut Buffer,
        palette: Palette,
        now: Instant,
    ) {
        self.next_frame.set(None);
        // 隐藏或失焦只暂停重绘，保留时间轴；恢复后直接继续，不受草稿影响。
        if !focused || !self.terminal_focused || area.is_empty() {
            return;
        }
        let Color::Rgb(red, green, blue) = palette.text else {
            return;
        };
        let Some(frame) = self.frame(now) else {
            return;
        };
        self.next_frame.set(Some(now + frame.next));
        let cursor = (
            area.x + (viewport.cursor_column as u16).min(area.width - 1),
            area.y + (viewport.cursor_row as u16).min(area.height - 1),
        );
        for y in area.y..area.bottom() {
            // 保护每行整个文本宽度，包括空格、选择区和宽字符的尾格。
            let text_width = viewport.rows.get(usize::from(y - area.y)).map_or(0, |row| {
                row.cells
                    .iter()
                    .map(|cell| cell.text.width())
                    .sum::<usize>()
            });
            for x in area.x..area.right() {
                if usize::from(x - area.x) < text_width || (x, y) == cursor {
                    continue;
                }
                let Some(cell) = buf.cell_mut((x, y)) else {
                    continue;
                };
                if cell.symbol() != " " || !cell.modifier.is_empty() || cell.bg != palette.panel {
                    continue;
                }
                let Color::Rgb(bg_red, bg_green, bg_blue) = cell.bg else {
                    continue;
                };
                let seed = star_hash(x - area.x, y - area.y);
                if !seed.is_multiple_of(5) {
                    continue;
                }
                let period = 4.0 + (seed % 31) as f32 / 10.0;
                let phase =
                    (frame.elapsed.as_secs_f32() / period + (seed % 997) as f32 / 997.0).fract();
                let brightness =
                    (phase * std::f32::consts::PI).sin().powi(12) * 0.55 * frame.visibility;
                if brightness < 0.04 {
                    continue;
                }
                let blend = |fg: u8, bg: u8| {
                    (f32::from(fg) * brightness + f32::from(bg) * (1.0 - brightness)) as u8
                };
                cell.set_symbol(DOTS[(seed / 161 % 8) as usize])
                    .set_fg(Color::Rgb(
                        blend(red, bg_red),
                        blend(green, bg_green),
                        blend(blue, bg_blue),
                    ));
            }
        }
    }
}

fn fade_in_visibility(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0)
}

fn star_hash(x: u16, y: u16) -> u64 {
    // 与 Codex rust-v0.154.0 相同的坐标分布；输入和重绘不重新播种。
    let mut value = u64::from(y) * 65537 + u64::from(x);
    value = (value ^ (value >> 16)).wrapping_mul(0x45d9f3b);
    value = (value ^ (value >> 16)).wrapping_mul(0x45d9f3b);
    value ^ (value >> 16)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::render_editor as render_for_test;
