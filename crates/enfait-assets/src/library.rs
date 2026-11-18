//! Bibliothèque de l'utilisateur sur disque : copie d'un fichier sous un nom
//! sûr et unique, commune à l'interface et au serveur MCP.

use enfait_core::{Asset, AssetKind, Origin};
use std::path::Path;

/// Copie `source` dans le dossier `dir` sous un nom tiré de `stem` (sûr,
/// 60 caractères au plus, numéroté s'il existe déjà) et renvoie l'entrée de
/// bibliothèque. `name` : nom affiché (sinon `stem`).
pub fn add_file(
    dir: &Path,
    source: &Path,
    stem: &str,
    name: Option<&str>,
    origin: Option<Origin>,
) -> Result<Asset, String> {
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let kind = AssetKind::from_extension(&ext).ok_or_else(|| {
        format!("format .{ext} non pris en charge (images, SVG, vidéos, polices, sons)")
    })?;
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(60)
        .collect();
    let safe = safe.trim_matches('-');
    let safe = if safe.is_empty() { "fichier" } else { safe };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut file = format!("{safe}.{ext}");
    let mut n = 2_u32;
    while dir.join(&file).exists() {
        file = format!("{safe}-{n}.{ext}");
        n = n.saturating_add(1);
    }
    std::fs::copy(source, dir.join(&file)).map_err(|e| e.to_string())?;
    let name = name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| stem.replace(['-', '_'], " "));
    Ok(Asset {
        file,
        kind,
        name,
        description: String::new(),
        origin,
    })
}
