//! Styles the user saves to find them again in another project: the style
//! (preset and rules), the global effects, the remembered rules and the
//! library files the rules name with `@`, copied with the style.
//!
//! One folder per style under the user's data folder:
//! `styles/<id>/style.json` and the files next to it.

use enfait_core::{Asset, Look, PostFx, StyleChoice};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STYLE_FILE: &str = "style.json";

/// Folder holding the saved styles.
pub fn default_root() -> PathBuf {
    enfait_config::paths::data_dir().join("styles")
}

/// What a saved style carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedStyle {
    pub name: String,
    pub style: StyleChoice,
    pub fx: PostFx,
    /// Rules remembered from retouches.
    pub memory: Vec<String>,
    /// Library files named in the rules, stored next to `style.json`.
    pub assets: Vec<Asset>,
}

/// A saved style and the folder it lives in.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// Name of the folder.
    pub id: String,
    pub style: SavedStyle,
}

/// Folder name for a style name: letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(60).collect();
    if out.is_empty() {
        "style".to_owned()
    } else {
        out
    }
}

/// Saved styles, by name.
pub fn list(root: &Path) -> Vec<Stored> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Stored> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            let data = std::fs::read(entry.path().join(STYLE_FILE)).ok()?;
            match serde_json::from_slice::<SavedStyle>(&data) {
                Ok(style) => Some(Stored { id, style }),
                Err(e) => {
                    tracing::warn!("saved style {id}: {e}");
                    None
                }
            }
        })
        .collect();
    out.sort_by_key(|s| s.style.name.to_lowercase());
    out
}

/// Saves the style of `look` under `name`; a style of the same name is
/// replaced. `library`: the project's library folder, where the files
/// named in the rules are.
pub fn save(root: &Path, name: &str, look: &Look, library: &Path) -> Result<Stored, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the style needs a name".to_owned());
    }
    let id = slug(name);
    let dir = root.join(&id);
    let staging = root.join(format!(".{id}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let mut assets = Vec::new();
    for asset in look.mentioned(&look.style.rules) {
        std::fs::copy(library.join(&asset.file), staging.join(&asset.file))
            .map_err(|e| format!("{}: {e}", asset.file))?;
        assets.push(asset.clone());
    }
    let style = SavedStyle {
        name: name.to_owned(),
        style: look.style.clone(),
        fx: look.fx.clamped(),
        memory: look.memory.clone(),
        assets,
    };
    let json = serde_json::to_vec_pretty(&style).map_err(|e| e.to_string())?;
    std::fs::write(staging.join(STYLE_FILE), json).map_err(|e| e.to_string())?;
    // The new folder takes the place of the old one only once complete.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&staging, &dir).map_err(|e| e.to_string())?;
    Ok(Stored { id, style })
}

/// Deletes a saved style.
pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(format!("invalid style id “{id}”"));
    }
    std::fs::remove_dir_all(root.join(id)).map_err(|e| e.to_string())
}

/// Replaces `@old` with `@new` where `old` is a whole mention (not the
/// start of a longer file name).
fn rename_mention(text: &str, old: &str, new: &str) -> String {
    let needle = format!("@{old}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(&needle) {
        let after = rest
            .get(i.saturating_add(needle.len())..)
            .unwrap_or_default();
        let ends = after
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || (c.is_ascii_punctuation() && c != '-' && c != '_'))
            // `@logo.png` mentions `logo.png`, not `logo`: a dot followed
            // by a letter continues the name.
            && !(after.starts_with('.')
                && after.chars().nth(1).is_some_and(char::is_alphanumeric));
        out.push_str(rest.get(..i).unwrap_or_default());
        if ends {
            out.push('@');
            out.push_str(new);
        } else {
            out.push_str(&needle);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Applies a saved style to `look`: style, effects, remembered rules
/// (added to those of the project) and files, copied into `library`.
/// A file already in the library with the same content is reused;
/// another one of the same name makes the copy take a new name, and the
/// rules follow it.
pub fn apply(root: &Path, stored: &Stored, look: &mut Look, library: &Path) -> Result<(), String> {
    let dir = root.join(&stored.id);
    let saved = &stored.style;
    let mut rules = saved.style.rules.clone();
    let mut added = Vec::new();
    for asset in &saved.assets {
        let source = dir.join(&asset.file);
        let same = look.assets.iter().any(|a| a.file == asset.file)
            && std::fs::read(library.join(&asset.file)).ok() == std::fs::read(&source).ok();
        if same {
            continue;
        }
        let stem = Path::new(&asset.file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut copy = crate::library::add_file(
            library,
            &source,
            &stem,
            Some(&asset.name),
            asset.origin.clone(),
        )?;
        copy.description.clone_from(&asset.description);
        if copy.file != asset.file {
            rules = rename_mention(&rules, &asset.file, &copy.file);
        }
        added.push(copy);
    }
    look.assets.extend(added);
    look.style = StyleChoice {
        preset: saved.style.preset.clone(),
        rules,
    };
    look.fx = saved.fx.clamped();
    for rule in &saved.memory {
        if !look.memory.contains(rule) {
            look.memory.push(rule.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::AssetKind;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("enfait-styles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn asset(file: &str) -> Asset {
        Asset {
            file: file.to_owned(),
            kind: AssetKind::Image,
            name: "logo".to_owned(),
            description: "top right".to_owned(),
            origin: None,
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("  Ma marque — 2026 "), "ma-marque-2026");
        assert_eq!(slug("///"), "style");
    }

    #[test]
    fn mentions_are_renamed_whole() {
        assert_eq!(
            rename_mention(
                "@logo.png, pas @logo.png-2 ni @logo.pngx.",
                "logo.png",
                "logo-2.png"
            ),
            "@logo-2.png, pas @logo.png-2 ni @logo.pngx."
        );
        assert_eq!(rename_mention("(@a.png)", "a.png", "b.png"), "(@b.png)");
    }

    #[test]
    fn saved_then_applied_in_another_project() {
        let root = temp("root");
        let first = temp("first");
        let second = temp("second");
        std::fs::write(first.join("logo.png"), b"brand").unwrap();
        let look = Look {
            style: StyleChoice {
                preset: Some("swiss".into()),
                rules: "Always put @logo.png in a corner.".into(),
            },
            fx: PostFx {
                grain: 0.3,
                ..PostFx::NEUTRAL
            },
            assets: vec![asset("logo.png"), asset("unused.png")],
            memory: vec!["never pink".into()],
        };
        let stored = save(&root, "Ma marque", &look, &first).unwrap();
        assert_eq!(stored.style.assets.len(), 1);
        assert_eq!(list(&root), vec![stored.clone()]);

        // The new project already has another file called logo.png.
        std::fs::write(second.join("logo.png"), b"other").unwrap();
        let mut target = Look {
            assets: vec![asset("logo.png")],
            memory: vec!["never pink".into(), "short texts".into()],
            ..Look::default()
        };
        apply(&root, &stored, &mut target, &second).unwrap();
        assert_eq!(target.style.preset.as_deref(), Some("swiss"));
        assert_eq!(target.style.rules, "Always put @logo-2.png in a corner.");
        assert_eq!(std::fs::read(second.join("logo-2.png")).unwrap(), b"brand");
        assert_eq!(target.assets.len(), 2);
        assert_eq!(target.assets[1].description, "top right");
        assert!((target.fx.grain - 0.3).abs() < 1e-6);
        assert_eq!(target.memory, vec!["never pink", "short texts"]);

        // Applied again: the file is already there, nothing is copied.
        let mut again = target.clone();
        again.style.rules = stored.style.style.rules.clone();
        std::fs::write(second.join("logo.png"), b"brand").unwrap();
        apply(&root, &stored, &mut again, &second).unwrap();
        assert_eq!(again.assets.len(), 2);
        assert_eq!(again.style.rules, "Always put @logo.png in a corner.");

        // Saved again under the same name: replaced.
        save(&root, "ma marque", &Look::default(), &first).unwrap();
        let all = list(&root);
        assert_eq!(all.len(), 1);
        assert!(all[0].style.assets.is_empty());
        remove(&root, &all[0].id).unwrap();
        assert!(list(&root).is_empty());

        for dir in [root, first, second] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
