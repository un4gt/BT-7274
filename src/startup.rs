//! 首次开场记录独立于用户设置；只有完整呈现后才记录完成。

use color_eyre::eyre::{ContextCompat, Result};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const COMPLETED: &[u8] = b"bt-7274-intro-completed\n";
const FRAME_TIME: Duration = Duration::from_millis(16);

#[derive(Debug)]
pub(crate) struct Startup {
    marker: PathBuf,
    first_run: bool,
    elapsed: Duration,
    last_update: Instant,
    focused: bool,
}

impl Startup {
    pub fn load(show_on_startup: bool, now: Instant) -> Result<Option<Self>> {
        let marker = dirs::data_dir()
            .context("无法定位系统数据目录")?
            .join(env!("CARGO_PKG_NAME"))
            .join("intro-completed");
        Ok(Self::at_path(marker, show_on_startup, now))
    }

    fn at_path(marker: PathBuf, show_on_startup: bool, now: Instant) -> Option<Self> {
        let completed = match std::fs::read(&marker) {
            Ok(bytes) => bytes == COMPLETED,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("无法读取开场记录，本次播放完整动画");
                }
                false
            }
        };
        (!completed || show_on_startup).then_some(Self {
            marker,
            first_run: !completed,
            elapsed: Duration::ZERO,
            last_update: now,
            focused: true,
        })
    }

    pub fn advance(&mut self, now: Instant) {
        if self.focused {
            self.elapsed = (self.elapsed + now.saturating_duration_since(self.last_update))
                .min(Duration::from_millis(titan::STARTUP_DURATION_MS));
        }
        self.last_update = now;
    }

    pub fn set_focus(&mut self, focused: bool, now: Instant) {
        self.advance(now);
        self.focused = focused;
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed.as_millis() as u64
    }

    pub fn finished(&self) -> bool {
        self.elapsed_ms() >= titan::STARTUP_DURATION_MS
    }

    pub fn can_skip(&self) -> bool {
        !self.first_run
    }

    pub fn next_frame(&self) -> Option<Instant> {
        self.focused.then_some(self.last_update + FRAME_TIME)
    }

    /// 调用方必须在成功绘制完成帧之后调用；中断播放不会消耗首次体验。
    pub fn record_completion(&self) -> Result<()> {
        if self.first_run && self.finished() {
            crate::storage::atomic_write_private(&self.marker, COMPLETED)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_ignores_preference_until_a_full_playback_is_recorded() {
        let root = std::env::temp_dir().join(format!("bt-intro-{}", uuid::Uuid::new_v4()));
        let path = root.join("intro-completed");
        let now = Instant::now();
        let mut startup = Startup::at_path(path.clone(), false, now).unwrap();
        assert!(!startup.can_skip());
        startup.advance(now + Duration::from_millis(2_000));
        startup.record_completion().unwrap();
        assert!(!path.exists());
        assert!(Startup::at_path(path.clone(), false, now).is_some());
        startup.advance(now + Duration::from_millis(titan::STARTUP_DURATION_MS));
        startup.record_completion().unwrap();
        assert!(Startup::at_path(path.clone(), false, now).is_none());
        assert!(
            Startup::at_path(path.clone(), true, now)
                .unwrap()
                .can_skip()
        );
        std::fs::write(&path, b"partial").unwrap();
        assert!(!Startup::at_path(path, false, now).unwrap().can_skip());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn losing_focus_pauses_playback_and_frame_wakeups() {
        let now = Instant::now();
        let mut startup = Startup {
            marker: PathBuf::new(),
            first_run: true,
            elapsed: Duration::ZERO,
            last_update: now,
            focused: true,
        };
        startup.set_focus(false, now + Duration::from_millis(700));
        assert!(startup.next_frame().is_none());
        startup.advance(now + Duration::from_secs(20));
        assert_eq!(startup.elapsed_ms(), 700);
        startup.set_focus(true, now + Duration::from_secs(30));
        startup.advance(now + Duration::from_millis(30_500));
        assert_eq!(startup.elapsed_ms(), 1_200);
        assert!(startup.next_frame().is_some());
    }

    impl Startup {
        pub(crate) fn for_test(now: Instant, first_run: bool) -> Self {
            Self {
                marker: PathBuf::new(),
                first_run,
                elapsed: Duration::ZERO,
                last_update: now,
                focused: true,
            }
        }
    }
}
