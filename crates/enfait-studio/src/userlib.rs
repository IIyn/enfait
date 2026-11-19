//! The user's look in the studio: their library (files and fonts)
//! installed in the compositions and described to the animators.

use anyhow::{Context as _, Result};
use enfait_core::{AssetKind, Look};
use enfait_motion::user::{ASSETS_DIR, UserAsset, UserFont, install_user};
use std::path::Path;

/// The user's fonts, in the format of [`crate::fonts_list`].
pub fn font_lines(look: &Look) -> String {
    look.assets
        .iter()
        .filter(|a| a.kind == AssetKind::Font)
        .map(|f| format!("- \"{}\" : police de l'utilisateur\n", f.name))
        .collect()
}

/// Files for the animators' brief: images and SVG (fonts are in the font
/// list, videos are for b-roll shots). A licence forbidding changes is
/// recalled.
pub fn brief(look: &Look) -> Option<String> {
    let items: Vec<(String, &str, String, String)> = look
        .assets
        .iter()
        .filter(|a| matches!(a.kind, AssetKind::Image | AssetKind::Svg))
        .map(|a| {
            let mut description = a.description.clone();
            if a.origin
                .as_ref()
                .is_some_and(|o| !o.license.terms().allows_changes())
            {
                if !description.trim().is_empty() {
                    description.push_str(" ; ");
                }
                description.push_str("licence: show it whole, without cropping or retouching");
            }
            (
                format!("{ASSETS_DIR}/{}", a.file),
                a.kind.label(),
                a.name.clone(),
                description,
            )
        })
        .collect();
    enfait_ai::animate::library_brief(&items)
}

/// Copies fonts and files of `library` into the compositions folder.
pub fn install(look: &Look, library: &Path, comps: &Path) -> Result<()> {
    let fonts: Vec<UserFont> = look
        .assets
        .iter()
        .filter(|a| a.kind == AssetKind::Font)
        .map(|f| UserFont {
            family: f.name.clone(),
            source: library.join(&f.file),
        })
        .collect();
    let assets: Vec<UserAsset> = look
        .assets
        .iter()
        .filter(|a| !matches!(a.kind, AssetKind::Font | AssetKind::Audio))
        .map(|a| UserAsset {
            name: a.file.clone(),
            source: library.join(&a.file),
        })
        .collect();
    install_user(comps, &fonts, &assets).context("user library")
}
