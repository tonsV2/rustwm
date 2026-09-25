//! The screen fading to black and back.
//!
//! A black layer in front of everything, cursor included, darkens over the
//! last seconds before an idle timeout fires (swayidle's lock), so the lock
//! does not come as a surprise, and quickly when a lock is asked for. Once
//! locked, the screen stays black until a key or the pointer brings the
//! lock screen back, as in GNOME.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    output::Output,
    utils::Rectangle,
};
use tracing::info;

use crate::state::State;

/// How long the fade to black takes when a lock is asked for.
pub const LOCK_FADE: Duration = Duration::from_millis(500);
/// How long the fade back takes when the user is there again.
pub const WAKE_FADE: Duration = Duration::from_millis(300);

/// How black the screen is, and where that is heading.
#[derive(Debug)]
pub struct Fade {
    from: f32,
    to: f32,
    started: Instant,
    /// How long going from `from` to `to` takes.
    duration: Duration,
    id: Id,
    commit: CommitCounter,
    /// The opacity last drawn, so a change is reported to damage tracking.
    drawn: f32,
}

impl Default for Fade {
    fn default() -> Self {
        Self {
            from: 0.0,
            to: 0.0,
            started: Instant::now(),
            duration: Duration::ZERO,
            id: Id::new(),
            commit: CommitCounter::default(),
            drawn: 0.0,
        }
    }
}

impl Fade {
    /// How black the screen is at `now`, from 0 (not at all) to 1.
    pub fn alpha(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * t
    }

    /// Where the fade is heading, 0 or 1.
    pub fn target(&self) -> f32 {
        self.to
    }

    /// Whether the screen is all black and staying so.
    pub fn is_dark(&self, now: Instant) -> bool {
        self.to >= 1.0 && now.saturating_duration_since(self.started) >= self.duration
    }

    /// Starts fading from wherever the screen is now to `to`. `duration` is
    /// for a fade all the way; a shorter distance takes proportionally less.
    pub fn fade_to(&mut self, to: f32, duration: Duration) {
        let now = Instant::now();
        let from = self.alpha(now);
        if from == to && self.to == to {
            return;
        }
        self.from = from;
        self.to = to;
        self.started = now;
        self.duration = duration.mul_f32((to - from).abs());
    }

    /// Goes straight to `to`, without a fade.
    pub fn set(&mut self, to: f32) {
        self.from = to;
        self.to = to;
        self.started = Instant::now();
        self.duration = Duration::ZERO;
    }

    /// The black layer over `output`, or `None` while there is nothing to
    /// darken.
    pub fn element(&mut self, output: &Output) -> Option<SolidColorRenderElement> {
        let alpha = self.alpha(Instant::now());
        if alpha <= 0.0 {
            self.drawn = 0.0;
            return None;
        }
        if alpha != self.drawn {
            self.commit.increment();
            self.drawn = alpha;
        }
        let size = output.current_mode()?.size;
        Some(SolidColorRenderElement::new(
            self.id.clone(),
            Rectangle::from_size(size),
            self.commit,
            [0.0, 0.0, 0.0, alpha],
            Kind::Unspecified,
        ))
    }
}

impl State {
    /// Fades to black over the configured time, timed by [`State::schedule_idle_fade`] to end as the next idle timeout fires.
    pub fn start_idle_fade(&mut self) {
        let duration = Duration::from_millis(self.config.lock.fade_ms);
        if self.fade.target() < 1.0 {
            info!("fading to black before the idle timeout");
        }
        self.fade.fade_to(1.0, duration);
    }

    /// Input brings light: a fade to black turns around, and a locked
    /// screen shows the lock screen again. A lock being put up finishes its
    /// fade to black first.
    pub fn wake_screen(&mut self) {
        if self.is_locking() || self.fade.target() <= 0.0 {
            return;
        }
        info!("screen woken");
        self.fade.fade_to(0.0, WAKE_FADE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_linearly_and_settles() {
        let mut fade = Fade::default();
        assert_eq!(fade.alpha(Instant::now()), 0.0);
        fade.fade_to(1.0, Duration::from_secs(10));
        let start = fade.started;
        assert!((fade.alpha(start + Duration::from_secs(5)) - 0.5).abs() < 0.01);
        assert!(!fade.is_dark(start + Duration::from_secs(9)));
        assert!(fade.is_dark(start + Duration::from_secs(10)));
        assert_eq!(fade.alpha(start + Duration::from_secs(20)), 1.0);
    }

    #[test]
    fn a_shorter_distance_takes_less_time() {
        let mut fade = Fade::default();
        fade.set(0.5);
        fade.fade_to(1.0, Duration::from_secs(10));
        assert_eq!(fade.duration, Duration::from_secs(5));
        fade.set(1.0);
        fade.fade_to(1.0, Duration::from_secs(10));
        assert!(fade.is_dark(Instant::now()));
    }
}
