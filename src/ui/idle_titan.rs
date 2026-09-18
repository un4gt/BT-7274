//! 空对话机体的渐变与待机时钟；动作间只预约下次动作，遮挡和失焦时暂停。

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

const FADE_IN: Duration = Duration::from_millis(260);
const FADE_OUT: Duration = Duration::from_millis(180);
const FRAME_TIME: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy)]
struct Fade {
    position: f32,
    show: bool,
    last_frame: Option<Instant>,
}

#[derive(Debug, Clone, Copy, Default)]
struct MotionClock {
    elapsed: Duration,
    last_frame: Option<Instant>,
}

#[derive(Debug)]
pub(crate) struct IdleTitan {
    fade: Cell<Option<Fade>>,
    motion: Cell<MotionClock>,
    next_frame: Cell<Option<Instant>>,
    terminal_focused: bool,
}

impl Default for IdleTitan {
    fn default() -> Self {
        Self {
            fade: Cell::new(None),
            motion: Cell::new(MotionClock::default()),
            next_frame: Cell::new(None),
            terminal_focused: true,
        }
    }
}

impl IdleTitan {
    /// 首帧及启动动画交接直接使用最终亮度；消息或提示接管区域时立即清除机体。
    pub fn settle(&self, show: bool) {
        self.fade.set(Some(Fade {
            position: if show { 1.0 } else { 0.0 },
            show,
            last_frame: None,
        }));
        self.next_frame.set(None);
        self.motion.set(MotionClock::default());
    }

    pub fn update(&self, show: bool, visible: bool, now: Instant) {
        self.next_frame.set(None);
        let mut fade = self.fade.get().unwrap_or_else(|| {
            self.settle(show);
            self.fade.get().unwrap()
        });
        if fade.show != show {
            fade.show = show;
            // 从上一帧实际画出的亮度反向，不重置到全亮或全暗。
            fade.last_frame = None;
        }
        if visible && self.terminal_focused {
            if let Some(previous) = fade.last_frame {
                let elapsed = now.saturating_duration_since(previous).as_secs_f32();
                fade.position = if show {
                    (fade.position + elapsed / FADE_IN.as_secs_f32()).min(1.0)
                } else {
                    (fade.position - elapsed / FADE_OUT.as_secs_f32()).max(0.0)
                };
            }
            fade.last_frame = Some(now);
            if fade.position != if show { 1.0 } else { 0.0 } {
                self.next_frame.set(Some(now + FRAME_TIME));
            }
        } else {
            fade.last_frame = None;
        }
        self.fade.set(Some(fade));
        let mut motion = self.motion.get();
        if show && fade.position == 1.0 && visible && self.terminal_focused {
            if let Some(previous) = motion.last_frame {
                motion.elapsed += now.saturating_duration_since(previous);
            }
            motion.last_frame = Some(now);
            self.next_frame.set(Some(
                now + titan::IdleAnimation::at(
                    motion.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                )
                .next_frame_in(),
            ));
        } else {
            // 输入即冻结当前姿势，淡出途中不跳回站姿；完全隐藏后再重置轮播。
            motion.last_frame = None;
            if fade.position == 0.0 {
                motion.elapsed = Duration::ZERO;
            }
        }
        self.motion.set(motion);
    }

    pub fn opacity(&self) -> f32 {
        let t = self.fade.get().map_or(0.0, |fade| fade.position);
        // 两端减速；快速输入、清空时沿当前曲线来回，而不是重新开始一段动画。
        (t * t * t * (t * (t * 6.0 - 15.0) + 10.0)).clamp(0.0, 1.0)
    }

    pub fn next_frame(&self) -> Option<Instant> {
        self.next_frame.get()
    }

    pub fn animation(&self) -> titan::IdleAnimation {
        titan::IdleAnimation::at(
            self.motion
                .get()
                .elapsed
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        )
    }

    pub fn set_terminal_focus(&mut self, focused: bool) {
        self.terminal_focused = focused;
        if let Some(mut fade) = self.fade.get() {
            fade.last_frame = None;
            self.fade.set(Some(fade));
        }
        self.next_frame.set(None);
        let mut motion = self.motion.get();
        motion.last_frame = None;
        self.motion.set(motion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_typing_finishes_fade_and_clearing_reverses_from_current_opacity() {
        let fade = IdleTitan::default();
        let start = Instant::now();
        fade.update(true, true, start);
        assert_eq!(fade.opacity(), 1.0);
        assert_eq!(fade.next_frame(), Some(start + Duration::from_secs(5)));
        fade.update(false, true, start);
        assert_eq!(fade.opacity(), 1.0);
        fade.update(false, true, start + Duration::from_millis(90));
        assert!((fade.opacity() - 0.5).abs() < 0.001);
        let middle = fade.opacity();
        fade.update(true, true, start + Duration::from_millis(90));
        assert_eq!(fade.opacity(), middle);
        fade.update(true, true, start + Duration::from_millis(150));
        assert!(fade.opacity() > middle);
        fade.update(true, true, start + Duration::from_millis(230));
        assert_eq!(fade.opacity(), 1.0);
        assert_eq!(
            fade.next_frame(),
            Some(start + Duration::from_millis(5_230))
        );

        let start = start + Duration::from_secs(1);
        fade.update(false, true, start);
        for ms in (0..=200).step_by(10) {
            fade.update(false, true, start + Duration::from_millis(ms));
        }
        assert_eq!(fade.opacity(), 0.0);
        assert!(fade.next_frame().is_none());
        fade.update(true, true, start + Duration::from_secs(1));
        assert_eq!(fade.opacity(), 0.0);
        fade.update(true, true, start + Duration::from_secs(1) + FADE_IN);
        assert_eq!(fade.opacity(), 1.0);
        assert_eq!(fade.animation().action(), None);
        assert!(fade.next_frame().unwrap() > start + Duration::from_secs(5));
    }

    #[test]
    fn hidden_or_unfocused_views_pause_without_leaving_frame_wakeups() {
        let mut fade = IdleTitan::default();
        let start = Instant::now();
        fade.settle(true);
        fade.update(false, true, start);
        fade.update(false, true, start + Duration::from_millis(60));
        let middle = fade.opacity();
        fade.update(false, false, start + Duration::from_millis(60));
        assert!(fade.next_frame().is_none());
        fade.update(false, true, start + Duration::from_secs(30));
        assert_eq!(fade.opacity(), middle);
        assert!(fade.next_frame().is_some());
        fade.set_terminal_focus(false);
        fade.update(false, true, start + Duration::from_secs(60));
        assert_eq!(fade.opacity(), middle);
        assert!(fade.next_frame().is_none());
        fade.set_terminal_focus(true);
        fade.update(false, true, start + Duration::from_secs(90));
        assert_eq!(fade.opacity(), middle);
        fade.update(false, true, start + Duration::from_secs(90) + FADE_OUT);
        assert_eq!(fade.opacity(), 0.0);
        assert!(fade.next_frame().is_none());
    }

    #[test]
    fn motion_pauses_for_overlays_focus_and_fades_and_restarts_after_hiding() {
        let mut idle = IdleTitan::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        idle.update(true, true, at(0));
        idle.update(true, true, at(5_600));
        assert_eq!(idle.animation().action(), Some(titan::IdleAction::Scan));
        assert_eq!(idle.next_frame(), Some(at(5_616)));
        let elapsed = idle.motion.get().elapsed;
        idle.update(true, false, at(5_600));
        assert!(idle.next_frame().is_none());
        idle.update(true, true, at(30_000));
        assert_eq!(idle.motion.get().elapsed, elapsed);
        idle.set_terminal_focus(false);
        idle.update(true, true, at(60_000));
        assert!(idle.next_frame().is_none());
        idle.set_terminal_focus(true);
        idle.update(true, true, at(90_000));
        assert_eq!(idle.motion.get().elapsed, elapsed);

        idle.update(false, true, at(90_000));
        idle.update(false, true, at(90_090));
        assert_eq!(idle.motion.get().elapsed, elapsed);
        idle.update(true, true, at(90_090));
        idle.update(true, true, at(90_250));
        assert_eq!(idle.motion.get().elapsed, elapsed);
        idle.update(true, true, at(90_350));
        assert_eq!(
            idle.motion.get().elapsed,
            elapsed + Duration::from_millis(100)
        );

        idle.update(false, true, at(90_350));
        idle.update(false, true, at(90_550));
        assert!(idle.next_frame().is_none());
        assert_eq!(idle.animation().action(), None);
        idle.update(true, true, at(90_550));
        idle.update(true, true, at(90_850));
        assert_eq!(idle.next_frame(), Some(at(95_850)));
        idle.settle(false);
        assert!(idle.next_frame().is_none());
    }
}
