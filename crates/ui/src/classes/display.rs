use asset_core::AssetNamespace;

use crate::{ClassLoadoutCatalog, ClassPickerFolder, pretty_weapon_name};

pub fn localized(loc: &asset_game::LocalizeCatalog, key: &str, fallback: &str) -> String {
    loc.text(key.trim_start_matches('@'))
        .unwrap_or(fallback)
        .to_owned()
}

pub fn category_label(folder: ClassPickerFolder, loc: &asset_game::LocalizeCatalog) -> String {
    folder.category.map_or_else(
        || localized(loc, "MENU_EQUIPMENT_CAPS", "Equipment"),
        |category| localized(loc, category.loc_key().unwrap_or(""), category.menu_label()),
    )
}

pub fn label(
    key: &str,
    catalog: &ClassLoadoutCatalog,
    loc: &asset_game::LocalizeCatalog,
) -> String {
    if key.is_empty() {
        return localized(loc, "MENU_NONE", "None");
    }
    if let Some(preview) = catalog.previews.get(key)
        && let Some(text) = loc.text(preview.name_key.trim_start_matches('@'))
    {
        return text.to_owned();
    }
    loc.text(&format!(
        "PERKS_{}",
        key.trim_start_matches("specialty_").to_uppercase()
    ))
    .map(str::to_owned)
    .unwrap_or_else(|| {
        pretty_weapon_name(
            key.rsplit_once('+')
                .map_or(key, |(_, attachment)| attachment),
        )
    })
}

pub fn preview_image(key: &str, catalog: &ClassLoadoutCatalog) -> String {
    let Some(preview) = catalog.previews.get(key) else {
        return String::new();
    };
    if preview.image.is_empty() || asset_core::AssetKey::parse(&preview.image).is_ok() {
        return preview.image.clone();
    }
    let ns = asset_core::AssetKey::parse(key).map_or(AssetNamespace::Iw4, |key| key.namespace);
    format!("{}:material/{}", ns.as_str(), preview.image)
}
