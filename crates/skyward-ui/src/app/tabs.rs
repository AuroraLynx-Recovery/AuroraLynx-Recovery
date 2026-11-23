//! The four Material Expressive destinations. The dock uses the same hit
//! regions and focus path as the existing file manager dock.

use super::*;
use crate::icons::{self, Icon};

pub(super) const TABS: [Screen; 4] = [Screen::Home, Screen::Regular, Screen::Advanced, Screen::Settings];

pub(crate) fn title(screen: Screen, lang: Lang) -> &'static str {
    lang.pick(match screen {
        Screen::Home => texts::page::DEVICE_STATUS,
        Screen::Regular => texts::page::REGULAR_FLASHING,
        Screen::Advanced => texts::page::ADVANCED_FLASHING,
        Screen::Settings => texts::page::SETTINGS,
        _ => unreachable!("only primary destinations have tab titles"),
    })
}

pub(super) struct TabMotion {
    pub initial: [f32; 4],
    pub direction: i32,
    pub started: u64,
    pub previous: Canvas,
}

const TAB_PERIOD: f32 = 0.26;

impl TabMotion {
    pub(super) fn progress(&self, now: u64) -> f32 {
        motion::settle(now.saturating_sub(self.started), TAB_PERIOD)
    }

    pub(super) fn poses(&self, selected: Screen, now: u64) -> [f32; 4] {
        let progress = self.progress(now);
        std::array::from_fn(|index| self.initial[index] * (1.0 - progress) + if TABS[index] == selected { progress } else { 0.0 })
    }

    pub(super) fn paint(&self, out: &mut Canvas, incoming: &Canvas, now: u64) {
        out.clear(crate::color::Rgba::rgb(0, 0, 0));
        let shift = (out.w as f32 * (1.0 - self.progress(now))).round() as i32 * self.direction;
        out.lay(&self.previous, shift - out.w as i32 * self.direction, 0, 1.0);
        out.lay(incoming, shift, 0, 1.0);
    }
}

pub(super) fn dock(t: &Theme, f: &Fonts, lang: Lang, selected: Screen, poses: Option<[f32; 4]>) -> Dock {
    let m = t.m;
    let w = m.w as f32;
    let h = m.dp(88.0);
    let y = m.dp(12.0);
    let height = m.dp(64.0);
    let side = m.dp(24.0).min(((w - m.dp(208.0)) / 2.0).max(0.0));
    let outer = (w - side * 2.0).min(m.dp(480.0));
    let x = (w - outer) / 2.0;
    let inset = m.dp(8.0).min(outer * 0.03);
    let inner = outer - inset * 2.0;
    let minimum = m.dp(48.0).min(inner / 4.0);
    let font = m.dp(14.0);
    let tabs = std::array::from_fn::<_, 4, _>(|index| {
        let screen = TABS[index];
        let label = title(screen, lang);
        let (a, b) = f.strong.ink_span(label, font);
        let expanded = (m.dp(64.0) + (b - a).ceil().max(0.0)).min(inner - 3.0 * minimum);
        let change = poses.map_or(if selected == screen { 1.0 } else { 0.0 }, |poses| poses[index]);
        (screen, label, minimum + (expanded - minimum) * change, change)
    });
    let gap = ((inner - tabs.iter().map(|(_, _, width, _)| width).sum::<f32>()) / 3.0).max(0.0);
    let mut c = Canvas::new(m.w, h.ceil() as u32);
    c.fill_round_rect(x, y, outer, height, height / 2.0, t.pal.round);
    let mut left = x + inset;
    let cy = y + height / 2.0;
    let mut hits = Vec::with_capacity(TABS.len());
    for ((screen, label, width, change), icon) in tabs.into_iter().zip([Icon::Dashboard, Icon::PhoneDownload, Icon::Wrench, Icon::Gear]) {
        let r = Rect::new(left, cy - minimum / 2.0, width, minimum);
        let ink = crate::color::mix(t.pal.subtitle, t.pal.on_accent, change);
        if change > 0.0 {
            c.fill_round_rect(r.x, r.y, r.w, r.h, r.h / 2.0, t.pal.accent.with_alpha((change * 255.0).round() as u8));
        }
        let compact_center = left + width / 2.0;
        let glyph_x = if width > m.dp(60.0) {
            compact_center + (left + m.dp(26.0) - compact_center) * change
        } else {
            compact_center
        };
        let icon = if icon == Icon::Dashboard && change >= 0.5 { Icon::DashboardFilled } else { icon };
        icons::glyph(&mut c, icon, glyph_x, cy, m.dp(23.0).min(minimum), ink);
        if width >= m.dp(66.0) {
            let text_left = left + m.dp(45.0);
            let available = width - m.dp(55.0);
            c.text_center_in(&f.strong, label, text_left + available / 2.0, cy, font,
                ink.with_alpha((change * 255.0).round() as u8), available);
        }
        hits.push(Hit { rect: r, action: Action::SelectTab(screen) });
        left += width + gap;
    }
    Dock { canvas: c, hits }
}

impl App {
    pub(super) fn select_tab(&mut self, screen: Screen, now: u64) {
        if self.state.style != UiStyle::MaterialExpressive || self.stack.len() != 1 {
            return;
        }
        let Some(from) = TABS.iter().position(|&tab| tab == self.screen()) else { return };
        let Some(to) = TABS.iter().position(|&tab| tab == screen) else { return };
        if from == to {
            return;
        }
        if !self.state.appearance.reduce_motion {
            let interrupted = self.tab_motion.take();
            let initial = interrupted.as_ref().map_or_else(
                || std::array::from_fn(|index| if from == index { 1.0 } else { 0.0 }),
                |motion| motion.poses(TABS[from], now),
            );
            let last_page = if let (Some(motion), Some((_, incoming))) = (interrupted, self.layer.as_ref()) {
                let mut composed = self.backdrop.take().unwrap_or_else(|| Canvas::new(self.w, self.h));
                motion.paint(&mut composed, incoming, now);
                self.backdrop = Some(motion.previous);
                Some(composed)
            } else {
                self.leave(false);
                self.turn.take().map(|turn| turn.left)
            };
            self.tab_motion = last_page.map(|previous| TabMotion {
                initial,
                direction: if to > from { 1 } else { -1 },
                started: now,
                previous,
            });
        }
        let (_, scroll, focus) = *self.top();
        self.tab_positions[from] = (scroll, focus);
        let (scroll, focus) = self.tab_positions[to];
        *self.top_mut() = (screen, scroll, focus);
        self.settle_page();
        self.refresh();
    }

    pub(super) fn finish_tab_motion(&mut self) {
        if let Some(motion) = self.tab_motion.take() {
            self.backdrop = Some(motion.previous);
            self.dirty = true;
        }
    }

    pub(super) fn set_style(&mut self, style: UiStyle) {
        if style == self.state.style {
            return;
        }
        self.finish_tab_motion();
        self.state.style = style;
        self.titles = None;
        self.chip = None;
        self.button_chip = None;
        self.search_words = None;
        self.tab_positions.fill((0.0, Focus::Unset));
        // Choosing a style keeps Settings in view. Material Settings becomes
        // a primary destination; the Miuix version returns to Home on Back.
        if matches!(self.screen(), Screen::Settings | Screen::Appearance) {
            self.stack.clear();
            if style == UiStyle::Miuix {
                self.stack.push((Screen::Home, 0.0, Focus::Unset));
            }
            self.stack.push((Screen::Settings, 0.0, Focus::Unset));
        }
        self.settle_page();
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[test]
    fn a_tab_spring_moves_then_settles_or_skips_when_reduced() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        let mut canvas = Canvas::new(W, H);
        app.state.style = UiStyle::MaterialExpressive;
        app.compose(&mut canvas, &f, &i, "12:00", 1);
        tap_at(&mut app, &f, &i, Action::SelectTab(Screen::Regular), 50);
        assert_eq!(app.tab_motion.as_ref().map(|motion| motion.initial), Some([1.0, 0.0, 0.0, 0.0]));
        app.compose(&mut canvas, &f, &i, "12:00", 130);
        let expected = app.tab_motion.as_ref().unwrap().poses(Screen::Regular, 130);
        tap_at(&mut app, &f, &i, Action::SelectTab(Screen::Advanced), 130);
        assert_eq!(app.tab_motion.as_ref().unwrap().initial, expected, "rapid switching keeps the current indicator widths");
        app.compose(&mut canvas, &f, &i, "12:00", 800);
        assert!(app.tab_motion.is_none());
        app.state.appearance.reduce_motion = true;
        tap_at(&mut app, &f, &i, Action::SelectTab(Screen::Home), 900);
        assert_eq!(app.screen(), Screen::Home);
        assert!(app.tab_motion.is_none());
    }

    #[test]
    fn material_tabs_reuse_the_existing_screens_and_dock_hits() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        assert!(app.frame(&f, &i).dock.is_none());
        app.state.style = UiStyle::MaterialExpressive;
        app.refresh();
        for screen in [Screen::Regular, Screen::Advanced, Screen::Settings, Screen::Home] {
            tap(&mut app, &f, &i, Action::SelectTab(screen));
            assert_eq!(app.screen(), screen);
            assert_eq!(app.stack.len(), 1);
        }
        tap(&mut app, &f, &i, Action::SelectTab(Screen::Regular));
        tap(&mut app, &f, &i, Action::Open(Screen::Install));
        assert_eq!(app.screen(), Screen::Install);
        assert!(app.frame(&f, &i).dock.is_none());
        tap_back(&mut app, &f, &i, 1000);
        assert_eq!(app.screen(), Screen::Regular);
    }
}
