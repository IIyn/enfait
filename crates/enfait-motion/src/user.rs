//! Bibliothèque de l'utilisateur dans les compositions : ses fichiers sont
//! copiés dans `assets/`, ses polices dans `lib/user-fonts/` et déclarées
//! dans `lib/user-fonts.css` (chargée par toutes les compositions).

use crate::MotionError;
use crate::library::LIB_DIR;
use skrifa::attribute::Style;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider as _};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const USER_FONTS_CSS: &str = "user-fonts.css";
pub const ASSETS_DIR: &str = "assets";

/// Une police de l'utilisateur.
pub struct UserFont {
    /// Famille déclarée (`font-family`).
    pub family: String,
    pub source: PathBuf,
}

/// Un fichier de l'utilisateur, accessible en `assets/<name>`.
pub struct UserAsset {
    pub name: String,
    pub source: PathBuf,
}

/// Famille d'une police TrueType ou OpenType (`None` pour un format
/// compressé comme WOFF2, ou un fichier illisible).
pub fn font_family(data: &[u8]) -> Option<String> {
    let font = FontRef::new(data).ok()?;
    // Famille typographique (16) de préférence, sinon famille (1).
    [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]
        .into_iter()
        .find_map(|id| {
            font.localized_strings(id)
                .english_or_first()
                .map(|s| s.to_string().trim().to_owned())
                .filter(|s| !s.is_empty())
        })
}

/// Graisse et style d'une police (400, normal par défaut).
fn font_traits(data: &[u8]) -> (u16, bool) {
    FontRef::new(data).map_or((400, false), |font| {
        let attributes = font.attributes();
        // Graisse entre 1 et 1000 : la conversion ne perd rien.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let weight = attributes.weight.value().round().clamp(1.0, 1000.0) as u16;
        (weight, attributes.style == Style::Italic)
    })
}

/// Copie polices et fichiers de l'utilisateur dans `root` (le dossier des
/// compositions) et écrit `lib/user-fonts.css`.
pub fn install_user(
    root: &Path,
    fonts: &[UserFont],
    assets: &[UserAsset],
) -> Result<(), MotionError> {
    let lib = root.join(LIB_DIR);
    let font_dir = lib.join("user-fonts");
    std::fs::create_dir_all(&font_dir)?;
    let mut css = String::from("/* Polices de l'utilisateur, générées par enfait. */\n");
    for font in fonts {
        let Some(file) = font.source.file_name() else {
            continue;
        };
        let data = std::fs::read(&font.source)?;
        std::fs::write(font_dir.join(file), &data)?;
        let (weight, italic) = font_traits(&data);
        let _ = writeln!(
            css,
            "@font-face {{ font-family: \"{}\"; src: url(\"user-fonts/{}\"); font-weight: {weight}; \
             font-style: {}; font-display: block; }}",
            font.family.replace('"', ""),
            file.to_string_lossy(),
            if italic { "italic" } else { "normal" }
        );
    }
    std::fs::write(lib.join(USER_FONTS_CSS), css)?;
    let asset_dir = root.join(ASSETS_DIR);
    std::fs::create_dir_all(&asset_dir)?;
    for asset in assets {
        std::fs::copy(&asset.source, asset_dir.join(&asset.name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTER: &[u8] = include_bytes!("../../../assets/motion/fonts/Inter-Variable.ttf");

    #[test]
    fn reads_family_names() {
        assert_eq!(font_family(INTER).as_deref(), Some("Inter"));
        assert_eq!(font_family(b"pas une police"), None);
    }

    #[test]
    fn installs_fonts_and_assets() {
        let dir = std::env::temp_dir().join(format!("enfait-user-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("Marque.ttf"), INTER).unwrap();
        std::fs::write(src.join("logo.svg"), "<svg/>").unwrap();
        let root = dir.join("comps");
        install_user(
            &root,
            &[UserFont {
                family: "Marque".into(),
                source: src.join("Marque.ttf"),
            }],
            &[UserAsset {
                name: "logo.svg".into(),
                source: src.join("logo.svg"),
            }],
        )
        .unwrap();
        let css = std::fs::read_to_string(root.join("lib").join(USER_FONTS_CSS)).unwrap();
        assert!(css.contains("font-family: \"Marque\"; src: url(\"user-fonts/Marque.ttf\")"));
        assert!(root.join("lib/user-fonts/Marque.ttf").is_file());
        assert!(root.join("assets/logo.svg").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
