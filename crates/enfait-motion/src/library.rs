//! Bibliothèque mise à disposition des compositions : polices (OFL) et
//! anime.js (MIT), embarqués dans le binaire.

use crate::error::MotionError;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Dossier, relatif à la composition, où se trouve la bibliothèque.
pub const LIB_DIR: &str = "lib";

pub struct FontFace {
    pub family: &'static str,
    pub file: &'static str,
    /// Graisse fixe (`"400"`) ou plage d'une police variable (`"100 900"`).
    pub weight: &'static str,
    pub italic: bool,
    /// Plage de chasse d'une police variable (`"62% 125%"`).
    pub stretch: Option<&'static str>,
    data: &'static [u8],
}

macro_rules! face {
    ($family:literal, $file:literal, $weight:literal, $italic:literal $(, $stretch:literal)?) => {
        FontFace {
            family: $family,
            file: $file,
            weight: $weight,
            italic: $italic,
            stretch: face!(@stretch $($stretch)?),
            data: include_bytes!(concat!("../../../assets/motion/fonts/", $file)),
        }
    };
    (@stretch $s:literal) => { Some($s) };
    (@stretch) => { None };
}

/// Polices disponibles dans les compositions.
pub static FONTS: &[FontFace] = &[
    face!("Inter", "Inter-Variable.ttf", "100 900", false),
    face!("Inter", "Inter-VariableItalic.ttf", "100 900", true),
    face!(
        "Space Grotesk",
        "SpaceGrotesk-Variable.ttf",
        "300 700",
        false
    ),
    face!(
        "Archivo",
        "Archivo-Variable.ttf",
        "100 900",
        false,
        "62% 125%"
    ),
    face!(
        "Archivo",
        "Archivo-VariableItalic.ttf",
        "100 900",
        true,
        "62% 125%"
    ),
    face!("Fraunces", "Fraunces-Variable.ttf", "100 900", false),
    face!("Fraunces", "Fraunces-VariableItalic.ttf", "100 900", true),
    face!(
        "Instrument Serif",
        "InstrumentSerif-Regular.ttf",
        "400",
        false
    ),
    face!(
        "Instrument Serif",
        "InstrumentSerif-Italic.ttf",
        "400",
        true
    ),
    face!(
        "DM Serif Display",
        "DMSerifDisplay-Regular.ttf",
        "400",
        false
    ),
    face!("DM Serif Display", "DMSerifDisplay-Italic.ttf", "400", true),
    face!("Bebas Neue", "BebasNeue-Regular.ttf", "400", false),
    face!("Anton", "Anton-Regular.ttf", "400", false),
    face!("Caveat", "Caveat-Variable.ttf", "400 700", false),
    face!(
        "JetBrains Mono",
        "JetBrainsMono-Variable.ttf",
        "100 800",
        false
    ),
    face!(
        "JetBrains Mono",
        "JetBrainsMono-VariableItalic.ttf",
        "100 800",
        true
    ),
    face!("IBM Plex Sans", "IBMPlexSans-Regular.ttf", "400", false),
    face!("IBM Plex Sans", "IBMPlexSans-Medium.ttf", "500", false),
    face!("IBM Plex Sans", "IBMPlexSans-SemiBold.ttf", "600", false),
    face!(
        "IBM Plex Sans Condensed",
        "IBMPlexSansCondensed-SemiBold.ttf",
        "600",
        false
    ),
    face!("IBM Plex Mono", "IBMPlexMono-Regular.ttf", "400", false),
];

/// Familles disponibles, sans doublon, dans l'ordre de [`FONTS`].
pub fn font_families() -> Vec<&'static str> {
    let mut families: Vec<&'static str> = Vec::new();
    for face in FONTS {
        if !families.contains(&face.family) {
            families.push(face.family);
        }
    }
    families
}

/// Feuille `@font-face` pour toutes les polices.
pub fn fonts_css() -> String {
    let mut css = String::from("/* Généré par enfait. */\n");
    for face in FONTS {
        let style = if face.italic { "italic" } else { "normal" };
        // L'écriture dans une String ne peut pas échouer.
        let _ = write!(
            css,
            "@font-face {{ font-family: \"{}\"; src: url(\"fonts/{}\"); font-weight: {}; font-style: {};",
            face.family, face.file, face.weight, style
        );
        if let Some(stretch) = face.stretch {
            let _ = write!(css, " font-stretch: {stretch};");
        }
        css.push_str(" font-display: block; }\n");
    }
    css
}

/// Version d'anime.js embarquée (licence MIT, `assets/motion/anime`).
pub const ANIME_VERSION: &str = "4.5.0";
const ANIME_JS: &[u8] = include_bytes!("../../../assets/motion/anime/anime.umd.min.js");
const ANIME_FILE: &str = "anime.umd.min.js";

/// Installe la bibliothèque (polices, feuille de style, anime.js) dans
/// `root/lib`. Les fichiers déjà présents et identiques ne sont pas réécrits.
pub fn install(root: &Path) -> Result<PathBuf, MotionError> {
    let lib = root.join(LIB_DIR);
    let fonts = lib.join("fonts");
    std::fs::create_dir_all(&fonts)?;
    for face in FONTS {
        write_if_changed(&fonts.join(face.file), face.data)?;
    }
    write_if_changed(&lib.join("fonts.css"), fonts_css().as_bytes())?;
    // Polices de l'utilisateur : feuille vide tant qu'il n'en a pas (voir
    // `user::install_user`), pour que toutes les compositions la chargent.
    let user_css = lib.join(crate::user::USER_FONTS_CSS);
    if !user_css.exists() {
        std::fs::write(&user_css, "/* Aucune police de l'utilisateur. */\n")?;
    }
    write_if_changed(&lib.join(ANIME_FILE), ANIME_JS)?;
    Ok(lib)
}

fn write_if_changed(path: &Path, data: &[u8]) -> Result<(), MotionError> {
    let same = std::fs::metadata(path).is_ok_and(|m| m.len() == data.len() as u64)
        && std::fs::read(path).is_ok_and(|old| old == data);
    if !same {
        std::fs::write(path, data)?;
    }
    Ok(())
}

/// Balises à placer dans le `<head>` d'une composition qui vit dans `root`.
/// anime.js est exposé en global sous le nom `anime`.
pub fn head_tags() -> String {
    format!(
        "<link rel=\"stylesheet\" href=\"lib/fonts.css\">\n\
         <link rel=\"stylesheet\" href=\"lib/user-fonts.css\">\n\
         <script src=\"lib/{ANIME_FILE}\"></script>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_lists_every_face() {
        let css = fonts_css();
        assert_eq!(css.matches("@font-face").count(), FONTS.len());
        assert!(css.contains("font-stretch: 62% 125%"));
    }

    #[test]
    fn families_are_unique() {
        let families = font_families();
        assert!(families.contains(&"Inter"));
        assert_eq!(families.iter().filter(|f| **f == "Inter").count(), 1);
    }
}
