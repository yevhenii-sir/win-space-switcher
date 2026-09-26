use std::time::{Duration, Instant};

/// Opacity animation between two levels with smoothstep easing.
#[derive(Clone, Copy, Debug)]
pub struct Fade {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

impl Fade {
    /// `full_duration` is for a complete 0↔1 fade; shorter distances (e.g. reversing half-way) take
    /// proportionally less time, so the speed stays the same.
    pub fn new(from: f32, to: f32, full_duration: Duration, now: Instant) -> Self {
        let duration = full_duration.mul_f32((to - from).abs().min(1.0));
        Self { from, to, started: now, duration }
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn opacity_at(&self, now: Instant) -> f32 {
        let t = self.progress_at(now);
        self.from + (self.to - self.from) * t * t * (3.0 - 2.0 * t)
    }

    pub fn is_finished_at(&self, now: Instant) -> bool {
        self.progress_at(now) >= 1.0
    }

    fn progress_at(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.started);
        (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Duration = Duration::from_millis(200);

    #[test]
    fn runs_from_start_to_target() {
        let start = Instant::now();
        let fade = Fade::new(0.0, 1.0, FULL, start);
        assert_eq!(fade.opacity_at(start), 0.0);
        assert!((fade.opacity_at(start + FULL / 2) - 0.5).abs() < 1e-6);
        assert_eq!(fade.opacity_at(start + FULL), 1.0);
        assert!(!fade.is_finished_at(start + FULL / 2));
        assert!(fade.is_finished_at(start + FULL));
    }

    #[test]
    fn partial_distance_takes_proportional_time() {
        let start = Instant::now();
        let fade = Fade::new(0.5, 0.0, FULL, start);
        assert!(fade.is_finished_at(start + FULL / 2));
        assert_eq!(fade.opacity_at(start + FULL / 2), 0.0);
    }

    #[test]
    fn zero_duration_finishes_immediately() {
        let start = Instant::now();
        let fade = Fade::new(1.0, 0.0, Duration::ZERO, start);
        assert!(fade.is_finished_at(start));
        assert_eq!(fade.opacity_at(start), 0.0);
    }
}
