/// A critically damped spring coming to rest at zero: the motion a page
/// makes springing back from past its end, and the easing of bars and
/// titles appearing. `period` is its response, the time one swing would
/// take without damping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    start: u64,
    from: f32,
    velocity: f32,
    omega: f32,
}

impl Spring {
    /// A spring released at `start` from `from` moving at `velocity` units
    /// per second.
    pub fn new(start: u64, from: f32, velocity: f32, period: f32) -> Spring {
        Spring { start, from, velocity, omega: std::f32::consts::TAU / period }
    }

    /// Where the spring is at `now`, and whether it has settled: back
    /// within half a unit of rest after any swing out.
    pub fn at(&self, now: u64) -> (f32, bool) {
        let t = now.saturating_sub(self.start) as f32 / 1000.0;
        let (x0, v0, w) = (self.from, self.velocity, self.omega);
        let x = (x0 + (v0 + w * x0) * t) * (-w * t).exp();
        // A spring set moving away from rest turns once, when its velocity
        // (v0 − w (v0 + w x0) t) e^(−w t) reaches zero.
        let turn = if v0 + w * x0 != 0.0 { v0 / (w * (v0 + w * x0)) } else { 0.0 };
        (x, t >= turn && x.abs() < 0.5)
    }
}

/// How far 0 → 1 a spring of `period` released from rest has come after
/// `ms` milliseconds.
/// Where a spring of Compose stiffness 400 sits between `from` and `to`
/// after `ms`. `damping` is the ratio: 1 is critical, 0.5 is medium bouncy,
/// the same pair SukiSU uses for cards, switches and slider handles.
pub fn compose(ms: u64, from: f32, to: f32, damping: f32) -> (f32, bool) {
    compose_with(ms, from, to, 400.0, damping)
}

/// Where a spring of Compose `stiffness` (of a mass of one) sits between
/// `from` and `to` after `ms`, `damping` being the ratio: 1 is critical, 0.5
/// medium bouncy. The back gesture lets its progress go back on one of
/// stiffness 1500.
pub fn compose_with(ms: u64, from: f32, to: f32, stiffness: f32, damping: f32) -> (f32, bool) {
    let omega = stiffness.max(1.0).sqrt();
    let t = ms as f32 / 1000.0;
    let u0 = from - to;
    let (u, velocity) = if damping >= 0.999 {
        (u0 * (1.0 + omega * t) * (-omega * t).exp(), -u0 * omega * omega * t * (-omega * t).exp())
    } else {
        let z = damping.clamp(0.05, 0.999);
        let wd = omega * (1.0 - z * z).sqrt();
        let decay = (-z * omega * t).exp();
        (decay * (u0 * (wd * t).cos() + (z * omega * u0) / wd * (wd * t).sin()), -decay * u0 * omega * omega / wd * (wd * t).sin())
    };
    let settled = (u.abs() < 0.004 * (1.0 + (to - from).abs()) && velocity.abs() < 0.04 * (1.0 + (to - from).abs())) || t > 1.4;
    (to + u, settled)
}

/// A cubic Bézier from (0, 0) to (1, 1) with control points (`x1`, `y1`)
/// and (`x2`, `y2`), read at `x`: how far along it a value `x` of the way
/// is.
pub fn bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let across = |t: f32| 3.0 * (1.0 - t) * (1.0 - t) * t * x1 + 3.0 * (1.0 - t) * t * t * x2 + t * t * t;
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut t = x;
    for _ in 0..16 {
        t = (lo + hi) / 2.0;
        if across(t) < x {
            lo = t;
        } else {
            hi = t;
        }
    }
    3.0 * (1.0 - t) * (1.0 - t) * t * y1 + 3.0 * (1.0 - t) * t * t * y2 + t * t * t
}

/// The Android back gesture's curve, (0.1, 0.1, 0, 1): the page leaves at
/// the finger's pace and slows as it goes.
pub fn back_gesture(x: f32) -> f32 {
    bezier(x, 0.1, 0.1, 0.0, 1.0)
}

/// Material 3's emphasized decelerate, (0.05, 0.7, 0.1, 1): what a back
/// gesture runs on once it is taken.
pub fn emphasized(x: f32) -> f32 {
    bezier(x, 0.05, 0.7, 0.1, 1.0)
}

pub fn settle(ms: u64, period: f32) -> f32 {
    let (x, done) = Spring::new(0, 1.0, 0.0, period).at(ms);
    if done && x < 0.002 {
        1.0
    } else {
        1.0 - x
    }
}

/// How far a page shows past its end while a finger drags it `pull`
/// pixels past that end, on a screen `range` pixels tall: the page follows
/// less and less, and never more than a third of the screen.
pub fn rubber(pull: f32, range: f32) -> f32 {
    let x = (pull.abs() / range).min(1.0);
    (x - x * x + x * x * x / 3.0) * range * pull.signum()
}

/// The drag past the end that shows a page `offset` pixels past it: the
/// inverse of [`rubber`].
pub fn unrubber(offset: f32, range: f32) -> f32 {
    let d = offset.abs().min(range / 3.0);
    (range - range.powf(2.0 / 3.0) * (range - 3.0 * d).cbrt()) * offset.signum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_released_spring_comes_back_without_overshooting() {
        let s = Spring::new(0, 120.0, 0.0, 0.4);
        let mut last = 120.0;
        for ms in (0..1200).step_by(8) {
            let (x, _) = s.at(ms);
            assert!(x <= last && x >= 0.0, "{ms}: {x}");
            last = x;
        }
        assert!(s.at(1200).1, "settled");
        assert!(!s.at(100).1);
    }

    #[test]
    fn a_spring_thrown_out_swings_once_and_returns() {
        // A page hitting its end at 3000 px/s.
        let s = Spring::new(0, 0.0, 3000.0, 0.4);
        let w = std::f32::consts::TAU / 0.4;
        let peak_ms = (1000.0 / w) as u64;
        let (peak, _) = s.at(peak_ms);
        assert!((peak - 3000.0 / (w * std::f32::consts::E)).abs() < 1.0, "{peak}");
        assert!(!s.at(2).1, "it has only just left");
        assert!(s.at(1500).1);
        assert!(s.at(1500).0 >= 0.0, "it never crosses back");
    }

    #[test]
    fn a_medium_bouncy_spring_overshoots_then_settles() {
        assert!(!compose(120, 0.0, 1.0, 0.5).1, "crossing the target at speed does not finish the spring");
        let mut peak = 0.0_f32;
        for ms in (0..400).step_by(8) {
            peak = peak.max(compose(ms, 0.0, 1.0, 0.5).0);
        }
        assert!(peak > 1.08, "a card press springs past its target, {peak}");
        let (end, done) = compose(1400, 0.0, 1.0, 0.5);
        assert!(done && (end - 1.0).abs() < 0.01, "{end}");
        let (handle, _) = compose(0, 4.0, 2.0, 1.0);
        assert!((handle - 4.0).abs() < 0.01);
        let (narrow, settled) = compose(800, 4.0, 2.0, 1.0);
        assert!(settled && (narrow - 2.0).abs() < 0.05, "{narrow}");
    }

    #[test]
    fn settling_runs_from_zero_to_one() {
        assert_eq!(settle(0, 0.3), 0.0);
        assert!(settle(150, 0.3) > 0.7);
        assert_eq!(settle(2000, 0.3), 1.0);
    }

    #[test]
    fn rubber_banding_resists_more_the_further_it_goes() {
        let range = 3200.0;
        assert!((rubber(10.0, range) - 10.0).abs() < 0.1, "it starts at the finger's pace");
        assert!(rubber(1600.0, range) < 1600.0 * 0.7);
        assert!((rubber(10_000.0, range) - range / 3.0).abs() < 0.5, "a third of the screen at most");
        assert!(rubber(-400.0, range) < 0.0, "pulling down goes the other way");
        for pull in [0.0, 50.0, 400.0, -900.0, 2500.0] {
            assert!((unrubber(rubber(pull, range), range) - pull).abs() < 0.5, "{pull}");
        }
    }
}
