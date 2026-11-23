// Copyright (C) 2024 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0
//
// Vector paths from frameworks/base, android-17.0.0_r1:
// packages/SystemUI/res/drawable/battery_unified_frame{,_bg}.xml
// packages/SystemUI/res/drawable/battery_unified_attr_charging.xml
// packages/SystemUI/res/values/strings.xml (battery_unified_frame_path_string).
// BatteryFillDrawable and BatteryLayersDrawable drive the viewport, fill,
// layer order and attribution bounds. Text uses the recovery's bundled font.

use crate::canvas::Canvas;
use crate::color::Rgba;
use crate::font::Fonts;
use crate::icons;
use crate::theme::Theme;
use std::sync::LazyLock;
use tiny_skia::{BlendMode, FillRule, Paint, Pixmap, PixmapPaint, Stroke, Transform};

pub(crate) const WIDTH_DP: f32 = 24.0;
const HEIGHT_DP: f32 = 14.0;
const BODY: &str = "M2.75,3C2.75,1.757 3.757,0.75 5,0.75H20C21.795,0.75 23.25,2.205 23.25,4V10C23.25,11.795 21.795,13.25 20,13.25H5C3.757,13.25 2.75,12.243 2.75,11V3Z";
const TERMINAL: &str = "M0,4C0,3.448 0.448,3 1,3H1.5V11H1C0.448,11 0,10.552 0,10V4Z";
const CHARGING: &str = "M4,20L5,13H0L9,0H11L10,8H16L6,20H4Z";

static BODY_PATH: LazyLock<tiny_skia::Path> = LazyLock::new(|| icons::path(BODY));
static TERMINAL_PATH: LazyLock<tiny_skia::Path> = LazyLock::new(|| icons::path(TERMINAL));
static CHARGING_PATH: LazyLock<tiny_skia::Path> = LazyLock::new(|| icons::path(CHARGING));

pub(crate) fn draw(c: &mut Canvas, t: &Theme, f: &Fonts, level: u8, charging: bool, right: f32, cy: f32) {
    let dp = t.m.px_per_dp;
    let w = (WIDTH_DP * dp).ceil().max(1.0) as u32;
    let h = (HEIGHT_DP * dp).ceil().max(1.0) as u32;
    let mut icon = Pixmap::new(w, h).expect("the battery has a pixel extent");
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    let (fg, track, normal, low, active) = if t.dark {
        ((255, 255, 255), (0, 0, 0, 46), (95, 99, 104), (197, 34, 31), (24, 128, 56))
    } else {
        ((0, 0, 0), (255, 255, 255, 56), (154, 160, 166), (234, 67, 53), (52, 168, 83))
    };
    let at = Transform::from_scale(dp, dp);
    paint.set_color_rgba8(track.0, track.1, track.2, track.3);
    icon.fill_path(&BODY_PATH, &paint, FillRule::Winding, at, None);
    let outline = Stroke { width: 1.5, ..Stroke::default() };
    paint.set_color_rgba8(fg.0, fg.1, fg.2, 255);
    icon.stroke_path(&BODY_PATH, &paint, &outline, at, None);
    icon.fill_path(&TERMINAL_PATH, &paint, FillRule::Winding, at, None);
    if level > 0 {
        let mut fill_layer = Pixmap::new(w, h).expect("the fill has a pixel extent");
        let mut mask = tiny_skia::Mask::new(w, h).expect("the battery mask has a pixel extent");
        mask.fill_path(&BODY_PATH, FillRule::Winding, true, at);
        let fill = if charging { active } else if level < 20 { low } else { normal };
        paint.set_color_rgba8(fill.0, fill.1, fill.2, 255);
        let left = 3.5 + (19.0 * dp * (1.0 - f32::from(level.min(100)) / 100.0)).floor() / dp;
        if let Some(rect) = tiny_skia::Rect::from_ltrb(left, 0.0, 22.5, HEIGHT_DP) {
            fill_layer.fill_rect(rect, &paint, at, Some(&mask));
            let erase = Paint { blend_mode: BlendMode::Clear, anti_alias: true, ..Paint::default() };
            fill_layer.stroke_path(&BODY_PATH, &erase, &outline, at, None);
            icon.draw_pixmap(0, 0, fill_layer.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }
    paint.set_color_rgba8(fg.0, fg.1, fg.2, 255);
    if charging {
        let bolt = at.pre_translate(16.0, 4.0).pre_scale(0.3, 0.3);
        icon.fill_path(&CHARGING_PATH, &paint, FillRule::Winding, bolt, None);
    }
    let mut art = Canvas::new(w, h);
    for (out, pixel) in art.data.chunks_exact_mut(4).zip(icon.pixels()) {
        let pixel = pixel.demultiply();
        out.copy_from_slice(&[pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]);
    }
    let (center, size, nudge) = if !charging {
        (13.0, 10.0, 1.5)
    } else if level < 100 {
        (10.0, 9.0, 1.25)
    } else {
        (10.0, 6.0, 1.0)
    };
    let digits = level.min(100).to_string();
    let width = art.text_width(&f.strong, &digits, size * dp);
    let baseline = (2.0 + (10.0 + size) / 2.0 - nudge) * dp;
    art.text(&f.strong, &digits, center * dp - width / 2.0, baseline, size * dp, Rgba::rgb(fg.0, fg.1, fg.2), None);
    c.lay(&art, (right - w as f32).round() as i32, (cy - h as f32 / 2.0).round() as i32, 1.0);
}
