//! Material 3 Expressive control motion, matched to SukiSU Nova:
//! cards and keys spring to 0.97 with a medium bounce, switches travel and
//! widen while held, slider handles narrow without bouncing, and a
//! segmented choice slides between its options.

use super::*;
use crate::theme::MotionCue;

#[derive(Clone, Copy)]
struct Travel {
    from: f32,
    to: f32,
    start: u64,
    damping: f32,
}

impl Travel {
    fn rest() -> Self {
        Self { from: 0.0, to: 0.0, start: 0, damping: 1.0 }
    }

    fn at(self, now: u64) -> (f32, bool) {
        motion::compose(now.saturating_sub(self.start), self.from, self.to, self.damping)
    }

    fn retarget(&mut self, to: f32, now: u64, damping: f32) {
        if (self.to - to).abs() < 0.001 && (self.damping - damping).abs() < 0.01 {
            return;
        }
        let from = self.at(now).0;
        *self = Self { from, to, start: now, damping };
    }

    fn jump(&mut self, to: f32, now: u64) {
        *self = Self { from: to, to, start: now, damping: 1.0 };
    }
}

#[derive(Clone, Copy)]
struct SwitchAnim {
    cy: f32,
    from: f32,
    to: f32,
    start: u64,
}

#[derive(Clone, Copy)]
struct CheckAnim {
    row: Rect,
    start: u64,
}

#[derive(Clone, Copy)]
struct SegmentAnim {
    top: f32,
    from: u8,
    to: u8,
    start: u64,
}

#[derive(Clone, Copy)]
pub(super) struct Expressive {
    now: u64,
    handle: Travel,
    key: Travel,
    icon: Travel,
    switch: Option<SwitchAnim>,
    check: Option<CheckAnim>,
    segment: Option<SegmentAnim>,
    /// The icon button whose spring is running, kept after the finger lifts.
    icon_action: Option<Action>,
}

impl Expressive {
    pub(super) fn new() -> Self {
        Self {
            now: 0,
            handle: Travel::rest(),
            key: Travel::rest(),
            icon: Travel::rest(),
            switch: None,
            check: None,
            segment: None,
            icon_action: None,
        }
    }

    /// Move the springs toward what is held. A page has to be drawn again
    /// while a control's own pixels are still changing.
    pub(super) fn advance(&mut self, now: u64, active: bool, key_down: bool, icon_down: bool, handle_down: bool) -> bool {
        self.now = now;
        if !active {
            self.handle.jump(0.0, now);
            self.key.jump(0.0, now);
            self.icon.jump(0.0, now);
            self.switch = None;
            self.check = None;
            self.segment = None;
            return false;
        }
        self.handle.retarget(if handle_down { 1.0 } else { 0.0 }, now, 1.0);
        self.key.retarget(if key_down { 1.0 } else { 0.0 }, now, 0.5);
        self.icon.retarget(if icon_down { 1.0 } else { 0.0 }, now, 0.5);
        if self.switch.is_some_and(|sw| motion::compose(now.saturating_sub(sw.start), sw.from, sw.to, 0.5).1) {
            self.switch = None;
        }
        if self.check.is_some_and(|check| motion::compose(now.saturating_sub(check.start), 0.6, 1.0, 0.5).1) {
            self.check = None;
        }
        if self.segment.is_some_and(|seg| motion::compose(now.saturating_sub(seg.start), 0.0, 1.0, 0.75).1) {
            self.segment = None;
        }
        self.content_live(now)
    }

    pub(super) fn content_live(&self, now: u64) -> bool {
        self.switch.is_some()
            || self.check.is_some()
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

    pub(super) fn cue(&self) -> MotionCue {
        let mut cue = MotionCue::default();
        let now = self.now;
        if let Some(sw) = self.switch {
            cue.switch_cy = sw.cy;
            cue.switch_pos = motion::compose(now.saturating_sub(sw.start), sw.from, sw.to, 0.5).0.clamp(0.0, 1.0);
        }
        cue.handle = self.handle.at(now).0.clamp(0.0, 1.0);
        if let Some(check) = self.check {
            cue.check = check.row;
            cue.check_pop = motion::compose(now.saturating_sub(check.start), 0.6, 1.0, 0.5).0;
        }
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
    pub(super) fn row_squeeze_live(&self, now: u64) -> bool {
        self.expressive_active() && self.row_press.is_some_and(|press| !press.squeeze_settled(now))
    }

    pub(super) fn expressive_active(&self) -> bool {
        self.state.style == UiStyle::MaterialExpressive && !self.state.appearance.reduce_motion
    }

    pub(super) fn sample_expressive(&mut self, now: u64) {
        let active = self.expressive_active();
        let key_down = self.state.held.is_some();
        let icon_down = self.pressing().is_some();
        let handle_down = matches!(self.drag, Drag::Slider(_, _) | Drag::Confirm { .. });
        let rebuild = self.expressive.advance(now, active, key_down, icon_down, handle_down);
        self.expressive.follow_icon(if icon_down { self.pressing() } else { None }, now);
        if rebuild {
            self.frame = None;
            self.row_art = None;
            self.shown = None;
            self.layer = None;
            self.stale.clear();
            self.dirty = true;
        } else if active && (self.expressive.icon_live(now) || self.row_squeeze_live(now)) {
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
            Action::Toggle(switch) => {
                let on = match switch {
                    Switch::Vibrate => self.state.vibrate,
                    Switch::FileTransfer => self.state.file_transfer,
                    Switch::Compress => self.state.compress,
                    Switch::Verify => self.state.verify,
                    Switch::Signature => self.state.check_signature,
                    Switch::KeepRecovery => self.state.keep_recovery,
                    Switch::ShowPassword => self.state.show_password,
                    Switch::ReduceMotion => self.state.appearance.reduce_motion,
                    Switch::Wlan => self.state.wlan.as_ref().is_some_and(|w| !matches!(w.power, Power::Off | Power::Failed(_))),
                    Switch::SystemTimeZone => self.state.follow_system_zone,
                };
                self.expressive.switch = Some(SwitchAnim {
                    cy: row.y + row.h / 2.0,
                    from: if on { 1.0 } else { 0.0 },
                    to: if on { 0.0 } else { 1.0 },
                    start: now,
                });
            }
            Action::Style(style) => {
                // The appearance card lists Material 3 Expressive, then Miuix.
                let from = u8::from(self.state.style != UiStyle::MaterialExpressive);
                let to = u8::from(*style != UiStyle::MaterialExpressive);
                self.note_segment(row, from, to, now);
            }
            Action::Theme { dark } => self.note_segment(row, u8::from(self.state.dark), u8::from(*dark), now),
            Action::ColorSource(_)
            | Action::WipeItem(_)
            | Action::ScreenOff(_)
            | Action::TimeZone(_)
            | Action::Language(_)
            | Action::RebootTarget(_)
            | Action::BackupPart(_)
            | Action::RestorePart(_)
            | Action::LogPart(_)
            | Action::FlashPartition(_)
            | Action::FlashSlots(_)
            | Action::PickSlot(_)
            | Action::BackupItem(_)
            | Action::RestoreItem(_)
            | Action::ExtractToggle(_)
            | Action::BackupIndividual => {
                self.expressive.check = Some(CheckAnim { row, start: now });
            }
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
