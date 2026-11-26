//! Archived: the Miuix Home, Advanced and Settings layouts from before
//! those three destinations shared the Material 3 Expressive page design.
//!
//! Not compiled. The live copies remain in `pages.rs` as
//! `legacy_miuix_home`, `legacy_miuix_advanced` and
//! `legacy_miuix_settings` (`#[allow(dead_code)]`). Both styles now
//! render `material_home` / `material_advanced` / `material_settings`,
//! keeping Miuix colour and type. To restore the old rows, point
//! `pages::render` at those three functions again when `UiStyle::Miuix`.
use super::*;
