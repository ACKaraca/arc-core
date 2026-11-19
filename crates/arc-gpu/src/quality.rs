//! Adaptive quality during navigation (CORE-GPU-021).
//!
//! While the camera moves, the expensive effects of a frame are lowered; once the camera has
//! been still for [`SETTLE_TIME`], the settings the caller asked for apply again. The logic is
//! pure: every call takes the time `now` as a [`Duration`] from an origin the caller chooses, so
//! the rules are tested by advancing time by hand and the system clock is never read here.
//!
//! The effects lowered are, in order: multisampling (four samples become one), silhouette edges
//! and the level-of-detail budget, which is multiplied by [`NAVIGATION_LOD_FACTOR`] so coarser
//! levels are chosen (a budget of 0 keeps the finest level). Nothing else in the settings
//! changes, so a frame drawn at rest is the same whether or not a navigation came before it.

use std::time::Duration;

use crate::render::RenderSettings;

/// The time the camera must stay still before full quality returns.
pub const SETTLE_TIME: Duration = Duration::from_millis(250);

/// The factor the level-of-detail budget is multiplied by during navigation.
pub const NAVIGATION_LOD_FACTOR: f32 = 4.0;

/// Tracks the camera's motion and chooses the quality of each frame.
///
/// The camera is moving at `now` when its last change is less than the settle time before it.
/// Times must not decrease between calls that record motion; an earlier time than the last
/// recorded one counts as that time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveQuality {
    settle: Duration,
    last_motion: Option<Duration>,
}

impl Default for AdaptiveQuality {
    fn default() -> Self {
        Self::new(SETTLE_TIME)
    }
}

impl AdaptiveQuality {
    /// A tracker that restores full quality `settle` after the last camera change.
    #[must_use]
    pub fn new(settle: Duration) -> Self {
        Self {
            settle,
            last_motion: None,
        }
    }

    /// Records a camera change at `now`.
    pub fn camera_moved(&mut self, now: Duration) {
        self.last_motion = Some(self.last_motion.map_or(now, |t| t.max(now)));
    }

    /// Whether the camera is moving at `now`: its last change is less than the settle time
    /// before `now`.
    #[must_use]
    pub fn is_navigating(&self, now: Duration) -> bool {
        self.settled_at().is_some_and(|end| now < end)
    }

    /// The time from which full quality applies again, when a camera change has happened.
    #[must_use]
    pub fn settled_at(&self) -> Option<Duration> {
        self.last_motion.map(|t| t.saturating_add(self.settle))
    }

    /// The settings a frame at `now` is drawn with: `settings` at rest, and while the camera
    /// moves the same settings with the expensive effects lowered.
    #[must_use]
    pub fn apply(&self, settings: &RenderSettings, now: Duration) -> RenderSettings {
        if !self.is_navigating(now) {
            return *settings;
        }
        let budget = settings.lod_budget_px;
        RenderSettings {
            sample_count: 1,
            silhouette_edges: false,
            // Capped at the largest finite value so a large budget stays valid.
            lod_budget_px: if budget > 0.0 {
                (budget * NAVIGATION_LOD_FACTOR).min(f32::MAX)
            } else {
                budget
            },
            ..*settings
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn quality_drops_when_navigation_starts() {
        let mut q = AdaptiveQuality::new(ms(250));
        assert!(!q.is_navigating(ms(0)));
        q.camera_moved(ms(1000));
        assert!(q.is_navigating(ms(1000)));
        let settings = RenderSettings {
            sample_count: 4,
            silhouette_edges: true,
            ..RenderSettings::DEFAULT
        };
        let lowered = q.apply(&settings, ms(1000));
        assert_eq!(lowered.sample_count, 1);
        assert!(!lowered.silhouette_edges);
        assert!(lowered.lod_budget_px > settings.lod_budget_px);
    }

    #[test]
    fn quality_stays_low_while_the_camera_keeps_moving() {
        let mut q = AdaptiveQuality::new(ms(250));
        for step in 0..20 {
            let now = ms(100 * step);
            q.camera_moved(now);
            // Every step is inside the settle time of the one before it.
            assert!(q.is_navigating(now + ms(249)), "step {step}");
        }
        assert!(q.is_navigating(ms(1900) + ms(200)));
    }

    #[test]
    fn full_quality_returns_after_the_settle_time() {
        let mut q = AdaptiveQuality::new(ms(250));
        q.camera_moved(ms(1000));
        q.camera_moved(ms(1400));
        assert!(q.is_navigating(ms(1649)));
        assert!(!q.is_navigating(ms(1650)));
        assert_eq!(q.settled_at(), Some(ms(1650)));
        let settings = RenderSettings::DEFAULT;
        assert_eq!(q.apply(&settings, ms(1650)), settings);
    }

    #[test]
    fn a_time_before_the_last_motion_keeps_the_later_one() {
        let mut q = AdaptiveQuality::new(ms(250));
        q.camera_moved(ms(1000));
        q.camera_moved(ms(900));
        assert_eq!(q.settled_at(), Some(ms(1250)));
    }

    #[test]
    fn a_zero_budget_stays_zero_and_a_huge_one_stays_finite() {
        let mut q = AdaptiveQuality::default();
        q.camera_moved(ms(0));
        let finest = RenderSettings {
            lod_budget_px: 0.0,
            ..RenderSettings::DEFAULT
        };
        assert_eq!(q.apply(&finest, ms(0)).lod_budget_px, 0.0);
        let huge = RenderSettings {
            lod_budget_px: f32::MAX,
            ..RenderSettings::DEFAULT
        };
        assert!(q.apply(&huge, ms(0)).lod_budget_px.is_finite());
    }
}
