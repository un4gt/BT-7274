//! All motion is sampled from elapsed time, never accumulated per frame.

pub const APPEAR_MS: u64 = 100;
pub const IGNITION_MS: u64 = 650;
pub const IMPACT_MS: u64 = IGNITION_MS + 700;
pub const RISE_MS: u64 = IMPACT_MS + 200;
pub const STAND_MS: u64 = RISE_MS + 700;
pub const EYE_MS: u64 = STAND_MS + 50;
pub const ONLINE_MS: u64 = EYE_MS + 450;
pub const DURATION_MS: u64 = ONLINE_MS + 750;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Blank,
    Falling,
    Braking,
    Impact,
    Rising,
    Waking,
    Online,
}

#[derive(Clone, Copy, Debug)]
pub struct Timeline {
    pub ms: u64,
    pub phase: Phase,
    pub rise: f32,
}

impl Timeline {
    pub fn at(ms: u64) -> Self {
        let ms = ms.min(DURATION_MS);
        let phase = match ms {
            0..APPEAR_MS => Phase::Blank,
            APPEAR_MS..IGNITION_MS => Phase::Falling,
            IGNITION_MS..IMPACT_MS => Phase::Braking,
            IMPACT_MS..RISE_MS => Phase::Impact,
            RISE_MS..STAND_MS => Phase::Rising,
            STAND_MS..ONLINE_MS => Phase::Waking,
            _ => Phase::Online,
        };
        Self {
            ms,
            phase,
            rise: smoothstep(progress(ms, RISE_MS, STAND_MS)),
        }
    }

    pub fn impact_age(self) -> Option<f32> {
        (self.ms >= IMPACT_MS).then(|| (self.ms - IMPACT_MS) as f32 / 1_000.0)
    }

    pub fn is_airborne(self) -> bool {
        matches!(self.phase, Phase::Falling | Phase::Braking)
    }
}

pub fn progress(ms: u64, start: u64, end: u64) -> f32 {
    (ms.saturating_sub(start) as f32 / (end - start) as f32).clamp(0.0, 1.0)
}

pub fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_has_a_full_200_ms_pose_hold() {
        for ms in IMPACT_MS..RISE_MS {
            let t = Timeline::at(ms);
            assert_eq!(t.phase, Phase::Impact);
            assert_eq!(t.rise, 0.0);
        }
        assert!(Timeline::at(RISE_MS + 50).rise > 0.0);
        assert_eq!(Timeline::at(STAND_MS).rise, 1.0);
    }

    #[test]
    fn timeline_boundaries_and_final_frame_are_stable() {
        for (ms, phase) in [
            (0, Phase::Blank),
            (100, Phase::Falling),
            (650, Phase::Braking),
            (1_350, Phase::Impact),
            (1_550, Phase::Rising),
            (2_250, Phase::Waking),
            (2_750, Phase::Online),
        ] {
            assert_eq!(Timeline::at(ms).phase, phase);
        }
        assert_eq!(Timeline::at(u64::MAX).ms, DURATION_MS);
        assert_eq!(DURATION_MS, 3_500);
        assert_eq!(IMPACT_MS - IGNITION_MS, 700);
    }
}
