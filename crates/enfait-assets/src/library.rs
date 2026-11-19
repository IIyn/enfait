//! The user's library on disk: copying a file under a safe, unique name,
//! shared by the interface and the MCP server.

use enfait_core::{Asset, AssetKind, Origin};
use std::path::Path;

/// Copies `source` into folder `dir` under a name taken from `stem` (safe,
/// 60 characters at most, numbered if it already exists) and returns the
/// library entry. `name`: displayed name (else `stem`).
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
    let kind = AssetKind::from_extension(&ext)
        .ok_or_else(|| format!("unsupported format .{ext} (images, SVG, videos, fonts, sounds)"))?;
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
    let safe = if safe.is_empty() { "file" } else { safe };
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
