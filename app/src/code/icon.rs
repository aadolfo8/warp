use warp_core::ui::appearance::Appearance;
use warpui::Element;
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{CacheOption, Image};

use super::material_icons;

/// Returns the VS Code-style (Material Icon Theme) icon for the given file path.
///
/// Every file gets an icon, falling back to the theme's generic file icon; the
/// `Option` is kept so existing callers can still provide their own fallback.
pub fn icon_from_file_path(path: &str, _appearance: &Appearance) -> Option<Box<dyn Element>> {
    Some(material_icon(material_icons::file_icon(path)))
}

/// Renders a bundled Material icon SVG with its own colors.
pub fn material_icon(path: &'static str) -> Box<dyn Element> {
    Image::new(AssetSource::Bundled { path }, CacheOption::BySize).finish()
}
