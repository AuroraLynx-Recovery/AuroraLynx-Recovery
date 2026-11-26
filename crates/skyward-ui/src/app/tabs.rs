//! The four Material Expressive destinations. The dock uses the same hit
//! regions and focus path as the existing file manager dock.

use super::*;
use crate::icons::{self, Icon};

pub(super) const TABS: [Screen; 4] = [Screen::Home, Screen::Regular, Screen::Advanced, Screen::Settings];

/// The dock stands under every page. Pages that already hold a strip at
/// the bottom (the file manager's copy bar, a folder picker) keep that
/// strip, stacked above the destinations.
pub(super) fn dockable(screen: Screen) -> bool {
    // Fastboot is the pre-recovery screen; its keys wrap its own choices.
    !matches!(screen, Screen::Fastboot)
}

/// Lay a page's own bottom strip over the destination dock, so both show.
pub(super) fn stack_docks(page: Dock, destinations: Dock) -> Dock {
    let w = page.canvas.w.max(destinations.canvas.w);
    let h = page.canvas.h + destinations.canvas.h;
    let mut canvas = Canvas::new(w, h);
    canvas.lay(&page.canvas, 0, 0, 1.0);
    canvas.lay(&destinations.canvas, 0, page.canvas.h as i32, 1.0);
    let mut hits = page.hits;
    let lift = page.canvas.h as f32;
    hits.extend(destinations.hits.into_iter().map(|hit| Hit {
        rect: Rect { y: hit.rect.y + lift, ..hit.rect },
        action: hit.action,
    }));
    Dock { canvas, hits }
}

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
    if t.style == UiStyle::Miuix {
        return miuix_dock(t, f, lang, selected, poses);
    }
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

/// The dock the Miuix style shows, after SukiSU Ultra: a floating capsule
/// 64 dp tall with 4 dp inside, every destination showing its icon and
/// label, and a 15% accent pill sliding under the chosen one.
fn miuix_dock(t: &Theme, f: &Fonts, lang: Lang, selected: Screen, poses: Option<[f32; 4]>) -> Dock {
    let m = t.m;
    let w = m.w as f32;
    let height = m.dp(64.0);
    let h = m.dp(92.0);
    let y = m.dp(16.0);
    let inset = m.dp(4.0);
    let cell = m.dp(76.0);
    let outer = (cell * 4.0 + inset * 2.0).min(w - m.dp(24.0));
    let x = (w - outer) / 2.0;
    let mut c = Canvas::new(m.w, h.ceil() as u32);
    // Lifted off the page, as floating bars are.
    let shadow = if t.dark { Rgba::rgb(0, 0, 0).with_alpha(51) } else { Rgba::rgb(0, 0, 0).with_alpha(26) };
    c.shadow_round_rect(x, y, outer, height, height / 2.0, m.dp(4.0), shadow);
    c.fill_round_rect(x, y, outer, height, height / 2.0, t.pal.card);
    let cw = (outer - inset * 2.0) / 4.0;
    let mut hits = Vec::with_capacity(TABS.len());
    let selected_index = TABS.iter().position(|&tab| tab == selected).unwrap_or(0);
    let index = poses.map_or(selected_index as f32, |weights| {
        weights.iter().enumerate().map(|(i, weight)| i as f32 * weight).sum()
    });
    c.fill_round_rect(x + inset + cw * index, y + inset, cw, height - inset * 2.0,
        (height - inset * 2.0) / 2.0, t.pal.accent.with_alpha(38));
    for (index, screen) in TABS.into_iter().enumerate() {
        let cx = x + inset + cw * (index as f32 + 0.5);
        let pill = Rect::new(cx - cw / 2.0, y + inset, cw, height - inset * 2.0);
        let ink = if screen == selected { t.pal.accent } else { t.pal.subtitle.with_alpha(102) };
        let icon = match screen {
            Screen::Home => Icon::Dashboard,
            Screen::Regular => Icon::PhoneDownload,
            Screen::Advanced => Icon::Wrench,
            _ => Icon::Gear,
        };
        icons::glyph(&mut c, icon, cx, pill.y + m.dp(17.0), m.dp(24.0), ink);
        c.text_center(&f.body, title(screen, lang), cx, pill.y + m.dp(40.0), m.dp(11.0), ink);
        hits.push(Hit { rect: pill, action: Action::SelectTab(screen) });
    }
    Dock { canvas: c, hits }
}

impl App {
    pub(super) fn select_tab(&mut self, screen: Screen, now: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(to) = TABS.iter().position(|&tab| tab == screen) else { return fx };
        // The dock stands under every page: a page opened over a
        // destination gives way first, without a slide of its own.
        let mut steps = 0;
        while self.stack.len() > 1 && steps < 8 {
            let before = self.stack.len();
            self.shown = None;
            fx.extend(self.back(now));
            if self.stack.len() == before {
                return fx;
            }
            steps += 1;
        }
        let Some(from) = TABS.iter().position(|&tab| tab == self.screen()) else {
            // The recovery started on a page of its own: the dock opens the
            // destination over it.
            self.open(screen);
            return fx;
        };
        if from == to {
            return fx;
        }
        let (_, scroll, focus) = *self.top();
        self.tab_positions[from] = (scroll, focus);
        let (scroll, focus) = self.tab_positions[to];
        // The page left is kept as it showed, to slide away from.
        let resting = std::array::from_fn(|index| if from == index { 1.0 } else { 0.0 });
        let (initial, previous) = if self.state.appearance.reduce_motion {
            (resting, None)
        } else if self.tab_motion.is_some() && self.layer.is_some() {
            let interrupted = self.tab_motion.take().expect("a tab motion was running");
            let initial = interrupted.poses(TABS[from], now);
            let mut composed = self.backdrop.take().unwrap_or_else(|| Canvas::new(self.w, self.h));
            if let Some((_, incoming)) = self.layer.as_ref() {
                interrupted.paint(&mut composed, incoming, now);
            }
            self.backdrop = Some(interrupted.previous);
            (initial, Some(composed))
        } else {
            self.leave(false);
            (resting, self.turn.take().map(|turn| turn.left))
        };
        *self.top_mut() = (screen, scroll, focus);
        self.tab_motion = previous.map(|previous| TabMotion {
            initial,
            direction: if to > from { 1 } else { -1 },
            started: now,
            previous,
        });
        self.settle_page();
        self.refresh();
        fx
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
        // Choosing a style keeps Settings in view. Both styles treat it as
        // a primary destination.
        if matches!(self.screen(), Screen::Settings | Screen::Appearance) {
            self.stack.clear();
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
        assert!(app.frame(&f, &i).dock.is_some(), "the dock shows in either style");
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
        assert!(app.frame(&f, &i).dock.is_some(), "the destinations stay under every page");
        tap_back(&mut app, &f, &i, 1000);
        assert_eq!(app.screen(), Screen::Regular);
    }

    #[test]
    fn typing_pages_replace_the_destination_dock_in_both_styles() {
        let (f, i) = (fonts(), info());
        for style in [UiStyle::Miuix, UiStyle::MaterialExpressive] {
            let mut app = App::new(Screen::Home, W, H, PPD);
            app.state.style = style;
            let mut out = Canvas::new(W, H);
            app.compose(&mut out, &f, &i, "12:00", 0);
            tap_button(&mut app, &f, &i, 1000);
            assert_eq!(app.screen(), Screen::Search);
            let frame = app.frame(&f, &i);
            assert!(frame.dock.is_none());
            let top = frame.canvas.keyboard_top.expect("typing page marks a keyboard");
            assert_eq!(top, (H as f32 - widgets::keyboard_height(&app.theme())).floor() as u32);
            app.compose(&mut out, &f, &i, "12:00", 1010);
            app.compose(&mut out, &f, &i, "12:00", 1110);
            assert!(app.bottom_level > 0.0 && app.bottom_level < 1.0);
            app.compose(&mut out, &f, &i, "12:00", 2000);
            assert_eq!(app.bottom_level, 1.0);
            app.act(Action::CloseSearch, 2100);
            app.compose(&mut out, &f, &i, "12:00", 2110);
            app.compose(&mut out, &f, &i, "12:00", 2210);
            assert!(app.bottom_level > 0.0 && app.bottom_level < 1.0);
            app.compose(&mut out, &f, &i, "12:00", 3000);
            assert_eq!(app.bottom_level, 0.0);
            assert!(app.bottom_keyboard.is_none());
        }
    }
}
