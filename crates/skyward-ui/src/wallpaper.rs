//! Decode bounded wallpaper images and score the pixels visible on the panel.

use crate::canvas::Canvas;
use crate::color::Rgba;
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbImage, imageops::FilterType};
use material_color_utils::quantize::{quantizer::Quantizer, quantizer_celebi::QuantizerCelebi};
use material_color_utils::score::score_colors::Score;
use material_color_utils::utils::color_utils::Argb;
use std::io::{BufRead, Seek, Write};

pub const MAX_BYTES: u64 = 24 * 1024 * 1024;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_SIDE: u32 = 8192;
const STORED_SIDE: u32 = 1800;

pub struct Wallpaper(RgbImage);

impl Wallpaper {
    pub fn read(source: impl BufRead + Seek) -> Result<Self> {
        let mut reader = ImageReader::new(source).with_guessed_format()?;
        ensure!(
            matches!(reader.format(), Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)),
            "select a PNG, JPEG or WebP image"
        );
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        limits.max_alloc = Some(160 * 1024 * 1024);
        reader.limits(limits);
        let mut decoder = reader.into_decoder()?;
        let (w, h) = decoder.dimensions();
        ensure!(u64::from(w) * u64::from(h) <= MAX_PIXELS && w > 0 && h > 0, "wallpaper exceeds 32 megapixels or is empty");
        let orientation = decoder.orientation()?;
        let mut image = DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        let image = image.resize(STORED_SIDE.min(image.width()), STORED_SIDE.min(image.height()), FilterType::Triangle);
        let pixels = image.into_rgba8();
        let rgb = RgbImage::from_fn(pixels.width(), pixels.height(), |x, y| {
            let [r, g, b, a] = pixels.get_pixel(x, y).0;
            image::Rgb([r, g, b].map(|channel| ((u16::from(channel) * u16::from(a) + 127) / 255) as u8))
        });
        Ok(Self(rgb))
    }

    pub fn write(&self, out: impl Write) -> Result<()> {
        image::codecs::jpeg::JpegEncoder::new_with_quality(out, 91).encode_image(&self.0).context("encode wallpaper")
    }

    pub fn seed(&self, panel_w: u32, panel_h: u32) -> u32 {
        let (x, y, w, h) = self.cover(panel_w, panel_h);
        let area = image::imageops::crop_imm(&self.0, x, y, w, h).to_image();
        let mut pixels = area.pixels().step_by((u64::from(w) * u64::from(h) / 12544).max(1) as usize)
            .map(|p| Argb::from_rgb(p[0], p[1], p[2])).collect::<Vec<_>>();
        // Wu fills a histogram, then Celebi starts K-means from those
        // clusters in first-seen order. Sort so the same crop always starts
        // the same way. 128 clusters is what Android's wallpaper scorer asks
        // for; fewer merges colours Score would otherwise keep apart.
        pixels.sort_unstable_by_key(|color| color.0);
        let colors = QuantizerCelebi::new().quantize(&pixels, 128).color_to_count;
        let fallback = colors.iter().max_by_key(|&(color, count)| (*count, color.0))
            .map_or(Argb(0xff7154a5), |(&color, _)| color);
        Score::score(&colors).desired_count(1).fallback_color_argb(fallback).call()[0].0 & 0xffffff
    }

    pub fn canvas(&self, panel_w: u32, panel_h: u32) -> Canvas {
        let (x, y, w, h) = self.cover(panel_w, panel_h);
        let crop = image::imageops::crop_imm(&self.0, x, y, w, h);
        let image = image::imageops::resize(&*crop, panel_w, panel_h, FilterType::Triangle);
        let mut canvas = Canvas::new(panel_w, panel_h);
        for y in 0..panel_h {
            canvas.paint_row(y, |x| {
                let [r, g, b] = image[(x as u32, y)].0;
                Rgba::rgb(r, g, b)
            });
        }
        canvas
    }

    fn cover(&self, panel_w: u32, panel_h: u32) -> (u32, u32, u32, u32) {
        let (w, h, pw, ph) = (u64::from(self.0.width()), u64::from(self.0.height()), u64::from(panel_w.max(1)), u64::from(panel_h.max(1)));
        let cw = ((h * pw / ph).clamp(1, w)) as u32;
        let ch = ((w * ph / pw).clamp(1, h)) as u32;
        ((w as u32 - cw) / 2, (h as u32 - ch) / 2, cw, ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn wallpaper_color_comes_from_the_visible_crop_and_rejects_corrupt_images() {
        let picture = Wallpaper(RgbImage::from_fn(120, 60, |x, _| {
            if (45..75).contains(&x) { image::Rgb([42, 88, 207]) } else { image::Rgb([43, 219, 61]) }
        }));
        assert_eq!(picture.seed(30, 60), 0x2a58cf);
        assert!(Wallpaper::read(Cursor::new(b"not a picture")).is_err());
        let mut encoded = Vec::new();
        picture.write(&mut encoded).unwrap();
        let decoded = Wallpaper::read(Cursor::new(encoded)).unwrap();
        assert_eq!((decoded.canvas(30, 60).w, decoded.canvas(30, 60).h), (30, 60));
    }
}
