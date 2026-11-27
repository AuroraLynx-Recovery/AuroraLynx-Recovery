//! Material 3 Expressive control motion, matched to SukiSU Nova:
//! cards and keys spring to 0.97 with a medium bounce, switches travel and
//! widen while held, slider handles narrow without bouncing, and a
//! segmented choice slides between its options.

use super::*;
use crate::theme::{ControlId, MotionCue};

#[derive(Clone, Copy)]
struct Travel {
    from: f32,
    to: f32,
    start: u64,
    damping: f32,
    stiffness: f32,
}

impl Travel {
    fn rest() -> Self {
        Self { from: 0.0, to: 0.0, start: 0, damping: 1.0, stiffness: 400.0 }
    }

    fn at(self, now: u64) -> (f32, bool) {
        motion::compose_with(now.saturating_sub(self.start), self.from, self.to, self.stiffness, self.damping)
    }

    fn retarget(&mut self, to: f32, now: u64, damping: f32) {
        if (self.to - to).abs() < 0.001 && (self.damping - damping).abs() < 0.01 {
            return;
        }
        let from = self.at(now).0;
        *self = Self { from, to, start: now, damping, ..*self };
    }

    /// Aim at `to` on a spring of `stiffness` and `damping`, as Material's
    /// fast spatial springs move a pressed switch.
    fn spring_to(&mut self, to: f32, now: u64, stiffness: f32, damping: f32) {
        if (self.to - to).abs() < 0.001 && self.stiffness == stiffness && self.damping == damping {
            return;
        }
        let from = self.at(now).0;
        *self = Self { from, to, start: now, damping, stiffness };
    }

    fn jump(&mut self, to: f32, now: u64) {
        *self = Self { from: to, to, start: now, damping: 1.0, stiffness: 400.0 };
    }
}

#[derive(Clone, Copy)]
struct ControlAnim {
    key: ControlId,
    from: f32,
    to: f32,
    start: u64,
}

#[derive(Clone, Copy)]
struct SegmentAnim {
    top: f32,
    from: u8,
    to: u8,
    start: u64,
}

pub(super) struct Expressive {
    now: u64,
    active: bool,
    handle: Travel,
    key: Travel,
    icon: Travel,
    press: Travel,
    press_cy: f32,
    controls: Vec<ControlAnim>,
    settled_controls: Vec<(ControlId, bool)>,
    control_screen: Option<Screen>,
    segment: Option<SegmentAnim>,
    /// The icon button whose spring is running, kept after the finger lifts.
    icon_action: Option<Action>,
}

impl Expressive {
    pub(super) fn new() -> Self {
        Self {
            now: 0,
            active: false,
            handle: Travel::rest(),
            key: Travel::rest(),
            icon: Travel::rest(),
            press: Travel::rest(),
            press_cy: 0.0,
            controls: Vec::new(),
            settled_controls: Vec::new(),
            control_screen: None,
            segment: None,
            icon_action: None,
        }
    }

    /// Move the springs toward what is held. A page has to be drawn again
    /// while a control's own pixels are still changing.
    pub(super) fn advance(&mut self, now: u64, active: bool, key_down: bool, icon_down: bool, handle_down: bool, switch_down: Option<f32>) -> bool {
        self.now = now;
        self.active = active;
        if let Some(cy) = switch_down {
            self.press_cy = cy;
        }
        if !active {
            self.handle.jump(0.0, now);
            self.key.jump(0.0, now);
            self.icon.jump(0.0, now);
            self.press.jump(0.0, now);
            self.controls.clear();
            self.segment = None;
            return false;
        }
        // The handle narrows in about 100 ms, as the expressive slider's does.
        self.handle.spring_to(if handle_down { 1.0 } else { 0.0 }, now, 1600.0, 1.0);
        self.key.retarget(if key_down { 1.0 } else { 0.0 }, now, 0.5);
        self.icon.retarget(if icon_down { 1.0 } else { 0.0 }, now, 0.5);
        // A switch's thumb grows on Material's fast spatial spring.
        self.press.spring_to(if switch_down.is_some() { 1.0 } else { 0.0 }, now, 800.0, 0.6);
        let had_controls = !self.controls.is_empty();
        self.controls.retain(|sw| !motion::compose_with(now.saturating_sub(sw.start), sw.from, sw.to, 800.0, 0.6).1);
        if self.segment.is_some_and(|seg| motion::compose(now.saturating_sub(seg.start), 0.0, 1.0, 0.75).1) {
            self.segment = None;
        }
        had_controls || self.content_live(now)
    }

    pub(super) fn content_live(&self, now: u64) -> bool {
        !self.press.at(now).1
            || !self.controls.is_empty()
            || self.segment.is_some()
            || !self.handle.at(now).1
            || !self.key.at(now).1
    }

    pub(super) fn icon_live(&self, now: u64) -> bool {
        !self.icon.at(now).1
    }

    pub(super) fn icon_action(&self) -> Option<Action> {
        self.icon_action
    }

    pub(super) fn follow_icon(&mut self, action: Option<Action>, now: u64) {
        if action.is_some() {
            self.icon_action = action;
        } else if self.icon.at(now).1 {
            self.icon_action = None;
        }
    }

    /// Observe the actual shared switch and check renderers after a page updates.
    /// This covers controls whose values arrive asynchronously as well as taps.
    pub(super) fn observe_controls(&mut self, screen: Screen, controls: &[(ControlId, bool)], animate: bool) -> bool {
        let mut changed = false;
        if self.control_screen != Some(screen) || !animate {
            self.controls.clear();
        } else {
            for &(key, on) in controls {
                let Some((_, old)) = self.settled_controls.iter().find(|(id, _)| *id == key) else { continue };
                if *old != on {
                    let from = if let Some(sw) = self.controls.iter().find(|sw| sw.key == key) {
                        motion::compose_with(self.now.saturating_sub(sw.start), sw.from, sw.to, 800.0, 0.6).0.clamp(0.0, 1.0)
                    } else if *old { 1.0 } else { 0.0 };
                    self.controls.retain(|sw| sw.key != key);
                    self.controls.push(ControlAnim { key, from, to: if on { 1.0 } else { 0.0 }, start: self.now });
                    changed = true;
                }
            }
        }
        self.control_screen = Some(screen);
        self.settled_controls = controls.to_vec();
        changed
    }

    pub(super) fn cue(&self) -> MotionCue {
        let mut cue = MotionCue::default();
        let now = self.now;
        cue.controls = self.controls.iter().map(|control| {
            let pos = motion::compose_with(now.saturating_sub(control.start), control.from, control.to, 800.0, 0.6).0;
            (control.key, pos.clamp(0.0, 1.0))
        }).collect();
        cue.moving_keys = self.active;
        cue.switch_press_cy = self.press_cy;
        cue.switch_press = self.press.at(now).0.clamp(0.0, 1.0);
        cue.handle = self.handle.at(now).0.clamp(0.0, 1.0);
        if let Some(seg) = self.segment {
            cue.segment_on = true;
            cue.segment_top = seg.top;
            cue.segment_from = seg.from;
            cue.segment_to = seg.to;
            cue.segment = motion::compose(now.saturating_sub(seg.start), 0.0, 1.0, 0.75).0.clamp(0.0, 1.0);
        }
        cue.key = self.key.at(now).0;
        cue.icon = self.icon.at(now).0;
        cue
    }
}

impl App {
    pub(super) fn expressive_active(&self) -> bool {
        !self.state.appearance.reduce_motion
    }

    pub(super) fn sample_expressive(&mut self, now: u64) {
        let active = self.expressive_active();
        let key_down = self.state.held.is_some();
        let icon_down = self.pressing().is_some();
        let handle_down = matches!(self.drag, Drag::Slider(_, _) | Drag::Confirm { .. });
        let switch_down = match self.drag {
            Drag::Press { action: Some(_), .. } => self.row_press.and_then(|press| {
                self.frame.as_ref()?.canvas.controls.iter().find_map(|(key, _)| {
                    let ControlId::Switch(x, y) = *key else { return None };
                    press.rect.contains(x as f32 - 1.0, y as f32).then_some(y as f32)
                })
            }),
            _ => None,
        };
        let rebuild = self.expressive.advance(now, active, key_down, icon_down, handle_down, switch_down);
        self.expressive.follow_icon(if icon_down { self.pressing() } else { None }, now);
        if rebuild {
            self.frame = None;
            self.row_art = None;
            self.shown = None;
            self.layer = None;
            self.stale.clear();
            self.dirty = true;
        } else if active && (self.expressive.icon_live(now) || self.row_press.is_some_and(|press| press.changing(now))) {
            self.dirty = true;
        }
    }

    /// Remember a control that has to travel, before the action changes it.
    pub(super) fn note_control_motion(&mut self, action: &Action, now: u64) {
        if !self.expressive_active() {
            return;
        }
        let Some(row) = self.row_press.map(|press| press.rect) else { return };
        match action {
            Action::Style(style) => {
                // The appearance card lists Material 3 Expressive, then Miuix.
                let from = u8::from(self.state.style != UiStyle::MaterialExpressive);
                let to = u8::from(*style != UiStyle::MaterialExpressive);
                self.note_segment(row, from, to, now);
            }
            Action::Theme { dark } => self.note_segment(row, u8::from(self.state.dark), u8::from(*dark), now),
            _ => {}
        }
    }

    fn note_segment(&mut self, row: Rect, from: u8, to: u8, now: u64) {
        if from == to {
            return;
        }
        self.expressive.segment = Some(SegmentAnim { top: row.y - self.theme().m.dp(39.0), from, to, start: now });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_switches_animate_without_a_matching_action() {
        let (first, second) = (ControlId::Switch(640, 300), ControlId::Check(640, 480));
        let mut motion = Expressive::new();
        assert!(!motion.observe_controls(Screen::Permissions, &[(first, false), (second, true)], true));
        assert!(motion.observe_controls(Screen::Permissions, &[(first, true), (second, false)], true));
        assert_eq!(motion.cue().controls[0], (first, 0.0));
        assert_eq!(motion.cue().controls[1], (second, 1.0));
        motion.advance(55, true, false, false, false, None);
        let cue = motion.cue();
        assert!(cue.controls[0].1 > 0.0);
        assert!(cue.controls[1].1 < 1.0);
        motion.observe_controls(Screen::Mount, &[(first, false)], true);
        assert!(motion.cue().controls.is_empty());
    }
}

#[cfg(test)]
mod spring_tests {
    use super::*;
    #[test]
    fn unchanged_target_allows_the_spring_to_finish() {
        let mut spring = Travel::rest();
        spring.spring_to(1.0, 0, 800.0, 0.6);
        for now in (16..2000).step_by(16) {
            spring.spring_to(1.0, now, 800.0, 0.6);
        }
        assert_eq!(spring.start, 0);
        assert!(spring.at(2000).1);
    }
}
