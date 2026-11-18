//! Look de l'utilisateur dans l'atelier : sa bibliothèque (fichiers et
//! polices) installée dans les compositions et décrite aux animateurs.

use anyhow::{Context as _, Result};
use enfait_core::{AssetKind, Look};
use enfait_motion::user::{ASSETS_DIR, UserAsset, UserFont, install_user};
use std::path::Path;

/// Polices de l'utilisateur, au format de [`crate::fonts_list`].
pub fn font_lines(look: &Look) -> String {
    look.assets
        .iter()
        .filter(|a| a.kind == AssetKind::Font)
        .map(|f| format!("- \"{}\" : police de l'utilisateur\n", f.name))
        .collect()
}

/// Fichiers pour le brief des animateurs : images et SVG (les polices sont
/// dans la liste des polices, les vidéos servent aux plans de b-roll). Une
/// licence qui interdit les modifications est rappelée.
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
                description.push_str("licence : à montrer entier, sans recadrage ni retouche");
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

/// Copie polices et fichiers de `library` dans le dossier des compositions.
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
    install_user(comps, &fonts, &assets).context("bibliothèque de l'utilisateur")
}
