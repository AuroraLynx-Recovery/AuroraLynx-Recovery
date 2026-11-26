//! Archived: the Android 17 predictive back gesture.
//!
//! Kept out of the build when the recovery went back to Skyward's own slide
//! for a swipe in from a screen edge. The active code is the `Drag::Edge`
//! cue in `app/input.rs`, which springs back into the edge or calls
//! [`App::back`], whose slide is [`Turn`] and [`App::slid`].
//!
//! To bring it back: move these pieces into their old homes (the state into
//! `app/mod.rs`, the methods into `impl App`, the canvas pieces into
//! `canvas.rs`), restore the calls in `Touch::Move`/`Touch::Up`, and draw
//! `swipe_shots` at the top of `compose_page`.
//!
//! It needs `motion::bezier`/`back_gesture`/`emphasized`, which stay in
//! `motion.rs`.

use super::*;
use crate::canvas::BackShot;

/// A finger's swipe in from a screen edge, with the pages Android 17's
/// predictive back has it show: the page under the finger shrinking away
/// over the page it goes back to, cornered and darkened as the system draws
/// them. Both pages are kept as they showed.
struct BackSwipe {
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
    fn progress(&self, now: u64, width: f32) -> (f32, bool) {
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


    pub(super) fn keep_turn(&mut self, turn: Turn) {
        if turn.back {
            self.backdrop = Some(turn.left);
        } else {
            self.unders.insert(turn.depth, turn.left);
        }
    }

    /// Begin a swipe in from a screen edge: keep the page as it showed and
    /// the page under it, for drawing them as the finger has them. Nothing
    /// starts without both.
    pub(super) fn begin_swipe(&mut self, from_left: bool) -> bool {
        if self.swipe.is_some() {
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
        self.frame = None;
        self.dirty = true;
    }

    /// The two pages a swipe shows at `now`, as Android 17's predictive back
    /// has them: the page under the finger scaled back over the page it goes
    /// back to, rounded and darkened, and both taking the finger's drift.
    pub(super) fn swipe_shots(&self, now: u64, m: &crate::theme::Metrics, dark: bool) -> Option<(crate::canvas::BackShot, crate::canvas::BackShot, f32)> {
        let swipe = self.swipe.as_ref()?;
        let (width, height) = (self.w as f32, self.h as f32);
        let (raw, _) = swipe.progress(now, width);
        let p = motion::back_gesture(raw);
        let dir = if swipe.from_left { 1.0 } else { -1.0 };
        let drift = (swipe.drift / (height / 2.0)).clamp(-1.0, 1.0);
        let eased = 1.0 - (1.0 - drift.abs()).powi(3);
        let ty = eased * (height / 20.0 - m.dp(8.0)) * drift.signum();
        let mut front = crate::canvas::BackShot {
            scale: 1.0 - 0.15 * p,
            dx: dir * p * ((1.0 - 0.85) * 0.5 * width - m.dp(8.0)),
            dy: ty,
            radius: swipe.radius,
            alpha: 1.0,
        };
        let mut behind = crate::canvas::BackShot {
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


/// Where one surface of a back gesture stands: scaled about the middle of
/// the screen, moved by (`dx`, `dy`), its corners rounded by `radius`, and
/// faded by `alpha`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackShot {
    pub scale: f32,
    pub dx: f32,
    pub dy: f32,
    pub radius: f32,
    pub alpha: f32,
}


    pub fn back(&mut self, front: &Canvas, behind: &Canvas, front_at: BackShot, behind_at: BackShot, scrim: f32, bg: Rgba) {
        assert!((front.w, front.h) == (self.w, self.h) && (behind.w, behind.h) == (self.w, self.h), "the pages are one size");
        assert!(self.whole() && front.whole() && behind.whole(), "pages go back whole");
        let (w, h) = (self.w as i32, self.h as i32);
        let wu = w as usize;
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let keep = ((1.0 - scrim.clamp(0.0, 1.0)) * 256.0).round() as u32;
        let bg_px = u32::from_le_bytes([bg.r, bg.g, bg.b, 255]);
        let (bs, bdx, bdy) = (behind_at.scale.max(0.02), behind_at.dx, behind_at.dy);
        let (fs, fdx, fdy) = (front_at.scale.max(0.02), front_at.dx, front_at.dy);
        let alpha = front_at.alpha.clamp(0.0, 1.0);
        let a8 = (alpha * 256.0).round() as u32;
        // Where the front reaches: nothing outside it is sampled.
        let (fw, fh) = (w as f32 * fs, h as f32 * fs);
        let (fcx, fcy) = (cx + fdx, cy + fdy);
        let (fx0, fx1) = ((fcx - fw / 2.0).floor().max(0.0) as i32, (fcx + fw / 2.0).ceil().min(w as f32) as i32);
        let (fy0, fy1) = ((fcy - fh / 2.0).floor().max(0.0) as i32, (fcy + fh / 2.0).ceil().min(h as f32) as i32);
        let corners = [front_at.radius * fs; 4];
        let (left, top) = (fcx - fw / 2.0, fcy - fh / 2.0);
        // Each surface maps a screen pixel to `pixel / scale + base`: walked
        // a pixel at a time, no division in the loop. The front's columns
        // and rows are read between, in 256ths, once.
        let (sb, sf) = (1.0 / bs, 1.0 / fs);
        let behind_base_x = cx - cx / bs - bdx;
        let behind_base_y = cy - cy / bs - bdy;
        let front_base_x = cx - cx / fs - fdx;
        let front_base_y = cy - cy / fs - fdy;
        let mut cols = Vec::with_capacity(wu);
        for x in 0..wu {
            let fx = ((x as f32 + 0.5) * sf + front_base_x).clamp(0.0, (w - 1) as f32);
            let x0 = fx.floor() as usize;
            cols.push((x0, (x0 + 1).min(wu - 1), ((fx - x0 as f32) * 256.0).round() as u32));
        }
        let front_px = front.data.as_chunks::<4>().0;
        let behind_px = behind.data.as_chunks::<4>().0;
        let row = wu * 4;
        for y in 0..h {
            let py = y as f32 + 0.5;
            let sy = py * sb + behind_base_y;
            let fy = py * sf + front_base_y;
            let rows_front = a8 > 0 && y >= fy0 && y < fy1;
            let behind_row = sy >= 0.0 && sy < h as f32;
            let iy = (sy as usize).min(h as usize - 1) * wu;
            let (fy0i, fy1i, ty) = if rows_front {
                let at = fy.clamp(0.0, (h - 1) as f32);
                let y0 = at.floor() as usize;
                (y0, (y0 + 1).min(h as usize - 1), ((at - y0 as f32) * 256.0).round() as u32)
            } else {
                (0, 0, 0)
            };
            let (r0, r1) = (fy0i * wu, fy1i * wu);
            let mut sx = 0.5 * sb + behind_base_x;
            for x in 0..wu {
                let mut cover = 0.0;
                if rows_front && x as i32 >= fx0 && (x as i32) < fx1 {
                    cover = back_cover(x as f32 + 0.5, py, left, top, fw, fh, corners[0]);
                }
                let pixel = if cover * alpha >= 0.998 {
                    // The front hides the page behind here, scrim and all.
                    let (x0, x1, tx) = cols[x];
                    let top = lerp_px(u32::from_le_bytes(front_px[r0 + x0]), u32::from_le_bytes(front_px[r0 + x1]), tx);
                    let bottom = lerp_px(u32::from_le_bytes(front_px[r1 + x0]), u32::from_le_bytes(front_px[r1 + x1]), tx);
                    lerp_px(top, bottom, ty)
                } else {
                    let mut pixel = if behind_row && sx >= 0.0 && sx < w as f32 {
                        u32::from_le_bytes(behind_px[(iy + sx as usize).min(behind_px.len() - 1)])
                    } else {
                        bg_px
                    };
                    if keep < 256 {
                        pixel = darken_px(pixel, keep);
                    }
                    if cover > 0.002 {
                        let (x0, x1, tx) = cols[x];
                        let top = lerp_px(u32::from_le_bytes(front_px[r0 + x0]), u32::from_le_bytes(front_px[r0 + x1]), tx);
                        let bottom = lerp_px(u32::from_le_bytes(front_px[r1 + x0]), u32::from_le_bytes(front_px[r1 + x1]), tx);
                        pixel = lerp_px(pixel, lerp_px(top, bottom, ty), (a8 as f32 * cover).round() as u32);
                    }
                    pixel
                };
                let i = y as usize * row + x * 4;
                self.data[i..i + 4].copy_from_slice(&pixel.to_le_bytes());
                sx += sb;
            }
        }
    }


/// How much of the pixel (`px`, `py`) the front surface of a back gesture
/// covers, its corners rounded by `r`: whole away from its corners, and
/// read off the corners' curve near them.
#[inline]
fn back_cover(px: f32, py: f32, left: f32, top: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if px >= left + r && px <= left + w - r {
        return ((py - top).min(top + h - py) + 0.5).clamp(0.0, 1.0);
    }
    if py >= top + r && py <= top + h - r {
        return ((px - left).min(left + w - px) + 0.5).clamp(0.0, 1.0);
    }
    let cx = left + w / 2.0;
    let cy = top + h / 2.0;
    let qx = (px - cx).abs() - (w / 2.0 - r);
    let qy = (py - cy).abs() - (h / 2.0 - r);
    let d = if qx > 0.0 && qy > 0.0 { qx.hypot(qy) - r } else { qx.max(qy) - r };
    (0.5 - d).clamp(0.0, 1.0)
}

/// The opaque pixel `b` mixed into `a` by `t` 256ths: their red and blue
/// taken in one multiply, then their green.
#[inline]
fn lerp_px(a: u32, b: u32, t: u32) -> u32 {
    let t = t.min(256);
    let rb = (((b & 0x00ff_00ff) * t + (a & 0x00ff_00ff) * (256 - t) + 0x0080_0080) >> 8) & 0x00ff_00ff;
    let g = (((((b >> 8) & 0xff) * t + ((a >> 8) & 0xff) * (256 - t)) >> 8) + 0) & 0xff;
    rb | (g << 8) | 0xff00_0000
}

/// The packed pixel `p` keeping `keep` 256ths of its light.
#[inline]
fn darken_px(p: u32, keep: u32) -> u32 {
    let rb = (((p & 0x00ff_00ff) * keep) >> 8) & 0x00ff_00ff;
    let g = (((p >> 8) & 0xff) * keep >> 8) & 0xff;
    rb | (g << 8) | 0xff00_0000
}

