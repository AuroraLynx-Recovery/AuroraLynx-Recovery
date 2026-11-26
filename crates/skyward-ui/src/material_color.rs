//! Material 3 color roles from the default, manual or wallpaper seed.

use crate::color::Rgba;
use crate::theme::Palette;
use material_color_utils::dynamic::color_spec::SpecVersion;
use material_color_utils::dynamic::material_dynamic_colors::MaterialDynamicColors;
use material_color_utils::hct::Hct;
use material_color_utils::scheme::SchemeTonalSpot;
use material_color_utils::utils::color_utils::Argb;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSource {
    Manual,
    Wallpaper,
    #[default]
    #[serde(other)]
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub color_source: ColorSource,
    pub manual_rgb: u32,
    pub wallpaper_rgb: Option<u32>,
    pub wallpaper_slot: bool,
    pub transparency: u8,
    pub reduce_motion: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { color_source: ColorSource::Default, manual_rgb: 0x7154a5, wallpaper_rgb: None, wallpaper_slot: false, transparency: 15, reduce_motion: false }
    }
}

impl Appearance {
    pub fn normalized(self) -> Self {
        Self {
            manual_rgb: self.manual_rgb.min(0xffffff),
            wallpaper_rgb: self.wallpaper_rgb.filter(|&rgb| rgb <= 0xffffff),
            transparency: self.transparency.min(35),
            ..self
        }
    }

    pub fn seed(self) -> Option<u32> {
        match self.color_source {
            ColorSource::Manual => Some(self.manual_rgb),
            ColorSource::Wallpaper => self.wallpaper_rgb,
            ColorSource::Default => None,
        }
    }
}

pub(crate) fn palette(seed: u32, dark: bool, base: Palette) -> Palette {
    let spec = SpecVersion::Spec2025;
    let colors = MaterialDynamicColors::new_with_spec(spec);
    let hct = Hct::from_argb(Argb(0xff000000 | seed));
    let scheme = SchemeTonalSpot::builder(hct, dark, 0.0).spec_version(spec).build();
    let ink = |role: std::sync::Arc<material_color_utils::dynamic::dynamic_color::DynamicColor>| {
        let argb = role.get_argb(&scheme);
        Rgba::rgb(argb.red(), argb.green(), argb.blue())
    };
    let surface = ink(colors.surface());
    let on_surface = ink(colors.on_surface());
    let variant = ink(colors.on_surface_variant());
    Palette {
        bg: surface,
        card: ink(colors.surface_container()),
        accent: ink(colors.primary()),
        on_accent: ink(colors.on_primary()),
        badge: ink(colors.primary_container()),
        text: on_surface,
        subtitle: variant,
        value: variant,
        section: variant,
        status: on_surface,
        chevron: ink(colors.outline()),
        outline: ink(colors.outline()),
        track: ink(colors.secondary_container()),
        stop: ink(colors.on_secondary_container()),
        switch_off: ink(colors.surface_container_high()),
        search: ink(colors.surface_container_highest()),
        search_icon: variant,
        round: ink(colors.surface_container_highest()),
        round_mark: variant,
        popup: ink(colors.surface_container()),
        danger: ink(colors.error()),
        danger_soft: ink(colors.error_container()),
        ..base
    }
}
