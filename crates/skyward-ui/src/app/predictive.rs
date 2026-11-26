//! The Material Expressive edge swipe previews the previous page.

use super::*;
use crate::canvas::BackShot;

const BACK_COMMIT_MS: u64 = 300;

/// A finger's swipe in from a screen edge, with the pages Android 17's
/// predictive back has it show: the page under the finger shrinking away
/// over the page it goes back to, cornered and darkened as the system draws
/// them. Both pages are kept as they showed.
pub(super) struct BackSwipe {
    from_left: bool,
    /// The finger's travel across the screen, and its drift along it.
    travel: f32,
    drift: f32,
    /// Once the finger lifts: from what progress, when, and whether the
    /// swipe is taken.
    release: Option<(u64, f32, bool)>,
    front: Canvas,
    behind: Canvas,
    bg: Rgba,
    /// The corners the page leaves with.
    radius: f32,
}

impl BackSwipe {
    /// How far the swipe has come at `now`, from 0 at rest to 1 with the
    /// page gone, and whether it has stopped moving.
    pub(super) fn progress(&self, now: u64, width: f32) -> (f32, bool) {
        let Some((at, from, taken)) = self.release else {
            return ((self.travel / width).clamp(0.0, 1.0), false);
        };
        if taken {
            let t = (now.saturating_sub(at) as f32 / BACK_COMMIT_MS as f32).clamp(0.0, 1.0);
            (from + (1.0 - from) * motion::emphasized(t), t >= 1.0)
        } else {
            // Let go short of going back, it returns on a critical spring.
            let (x, done) = motion::compose_with(now.saturating_sub(at), from, 0.0, 1500.0, 1.0);
            (x.clamp(0.0, 1.0), done)
        }
    }
}


impl App {
    pub(super) fn keep_turn(&mut self, turn: Turn) {
        if turn.back || self.state.style != UiStyle::MaterialExpressive {
            self.backdrop = Some(turn.left);
        } else {
            self.unders.retain(|depth, _| *depth < turn.depth && turn.depth - *depth < 3);
            self.unders.insert(turn.depth, turn.left);
        }
    }

    /// Begin a swipe in from a screen edge: keep the page as it showed and
    /// the page under it, for drawing them as the finger has them. Nothing
    /// starts without both.
    pub(super) fn begin_swipe(&mut self, from_left: bool) -> bool {
        if self.state.style != UiStyle::MaterialExpressive || self.state.appearance.reduce_motion || self.swipe.is_some() {
            return false;
        }
        let Some(view) = self.shown else { return false };
        let depth = self.stack.len().saturating_sub(1);
        let Some(behind) = (depth > 0).then(|| self.unders.remove(&(depth - 1))).flatten() else { return false };
        let t = self.theme();
        let view = View { pressing: None, ripple: None, row: 0.0, ..view };
        let mut front = Canvas::new(self.w, self.h);
        self.paint_page(&mut front, &t, None, &view);
        let radius = if self.corners > 0.0 { self.corners } else { t.m.dp(40.0) };
        self.swipe = Some(BackSwipe { from_left, travel: 0.0, drift: 0.0, release: None, front, behind, bg: t.pal.bg, radius });
        self.dirty = true;
        true
    }

    /// The finger has come `travel` pixels in from the edge it started at,
    /// and drifted `drift` along the screen.
    pub(super) fn swipe_moved(&mut self, travel: f32, drift: f32) {
        if let Some(swipe) = self.swipe.as_mut() {
            swipe.travel = travel.max(0.0);
            swipe.drift = drift;
            self.dirty = true;
        }
    }

    /// The finger lifted: past the threshold the swipe goes back a page,
    /// and short of it the pages spring back as they were.
    pub(super) fn end_swipe(&mut self, now: u64, armed: bool) -> Vec<Effect> {
        let Some(swipe) = self.swipe.as_mut() else { return Vec::new() };
        let progress = (swipe.travel / self.w as f32).clamp(0.0, 1.0);
        swipe.release = Some((now, progress, false));
        self.dirty = true;
        if !armed {
            return Vec::new();
        }
        let before = self.stack.len();
        let fx = self.back(now);
        self.end_turn();
        if self.stack.len() < before {
            if let Some(swipe) = self.swipe.as_mut() {
                swipe.release = Some((now, progress, true));
            }
        }
        fx
    }

    /// Stop a swipe still moving after the finger lifted, taking its pages
    /// away whether it was taken or let go of.
    pub(super) fn finish_swipe(&mut self) {
        let Some(swipe) = self.swipe.take() else { return };
        let top = self.stack.len().saturating_sub(1);
        if swipe.release.is_some_and(|(_, _, taken)| taken) {
            // Pages deeper than the one gone back to are stale now.
            self.unders.retain(|depth, _| *depth <= top);
        } else {
            self.unders.insert(top.saturating_sub(1), swipe.behind);
        }
        self.dirty = true;
    }

    pub(super) fn paint_swipe(&self, out: &mut Canvas, t: &Theme, now: u64) -> bool {
        let Some(swipe) = &self.swipe else { return false };
        let Some((front, behind, scrim)) = self.swipe_shots(now, &t.m, t.dark) else { return false };
        out.back(&swipe.front, &swipe.behind, front, behind, scrim, swipe.bg);
        true
    }

    /// The two pages a swipe shows at `now`, as Android 17's predictive back
    /// has them: the page under the finger scaled back over the page it goes
    /// back to, rounded and darkened, and both taking the finger's drift.
    pub(super) fn swipe_shots(&self, now: u64, m: &crate::theme::Metrics, dark: bool) -> Option<(BackShot, BackShot, f32)> {
        let swipe = self.swipe.as_ref()?;
        let (width, height) = (self.w as f32, self.h as f32);
        let (raw, _) = swipe.progress(now, width);
        let p = motion::back_gesture(raw);
        let dir = if swipe.from_left { 1.0 } else { -1.0 };
        let drift = (swipe.drift / (height / 2.0)).clamp(-1.0, 1.0);
        let eased = 1.0 - (1.0 - drift.abs()).powi(3);
        let ty = eased * (height / 20.0 - m.dp(8.0)) * drift.signum();
        let mut front = BackShot {
            scale: 1.0 - 0.15 * p,
            dx: dir * p * ((1.0 - 0.85) * 0.5 * width - m.dp(8.0)),
            dy: ty,
            radius: swipe.radius,
            alpha: 1.0,
        };
        let mut behind = BackShot {
            scale: 0.95 - 0.10 * p,
            dx: -dir * (1.0 - p) * m.dp(96.0),
            dy: ty,
            radius: 0.0,
            alpha: 1.0,
        };
        let mut scrim = (if dark { 0.8 } else { 0.2 }) * p;
        if let Some((at, from, true)) = swipe.release {
            // Taken, the page swells back as it slides off and fades within
            // the first fifth, while the page behind comes to fill the screen.
            let t = (now.saturating_sub(at) as f32 / BACK_COMMIT_MS as f32).clamp(0.0, 1.0);
            let c = motion::emphasized(t);
            let p0 = motion::back_gesture(from);
            let s0 = 1.0 - 0.15 * p0;
            let dx0 = dir * p0 * ((1.0 - 0.85) * 0.5 * width - m.dp(8.0));
            front.scale = s0 + (1.0 - s0) * c;
            front.dx = dx0 + (dir * (width * 0.5 + m.dp(96.0)) - dx0) * c;
            front.alpha = (1.0 - 5.0 * t).max(0.0);
            let behind0 = 0.95 - 0.10 * p0;
            behind.scale = behind0 + (1.0 - behind0) * c;
            behind.dx = (-dir * (1.0 - p0) * m.dp(96.0)) * (1.0 - c);
            scrim = (if dark { 0.8 } else { 0.2 }) * p0 * (1.0 - t);
        }
        Some((front, behind, scrim))
    }
}
