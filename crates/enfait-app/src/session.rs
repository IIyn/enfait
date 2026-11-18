//! Working session: the script, the imported take and the take choices,
//! saved as you go.
//!
//! Until several projects are supported, the app works on a single
//! session, in the user's data folder (`~/.local/share/enfait/session/` on
//! Linux, see `enfait_config::paths`): `script.md` (readable and editable
//! by hand), `session.json`, `look.json` (style, post-FX, library),
//! `finish.json` (music, SFX, ducking) and the library files in
//! `library/`.

use enfait_core::{AssetKind, Finish, Look, Origin};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionData {
    /// Imported take (the rush).
    pub rush: Option<PathBuf>,
    /// Take kept for a sentence, by hand: sentence text → rank of the take
    /// among those of this sentence. Keying by text keeps the choices when
    /// other sentences of the script are edited.
    pub choices: BTreeMap<String, usize>,
    /// Quality level of the animations.
    pub quality: enfait_ai::Quality,
    /// Model provider (API, Claude Code, local model).
    pub provider: enfait_ai::ProviderChoice,
}

pub struct Session {
    dir: PathBuf,
    pub script: String,
    pub data: SessionData,
    pub look: Look,
    /// Sound finish (`finish.json`).
    pub finish: Finish,
}

impl Session {
    /// Session folder (`ENFAIT_SESSION` to use another one, e.g. for a
    /// test).
    pub fn default_dir() -> PathBuf {
        // Same rule as the MCP server: they work on the same session.
        enfait_mcp::bridge::session_dir()
    }

    /// Reads the session (empty if it does not exist yet).
    pub fn load(dir: &Path) -> Self {
        let script = std::fs::read_to_string(dir.join("script.md")).unwrap_or_default();
        let data = std::fs::read(dir.join("session.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let look = std::fs::read(dir.join("look.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let finish = std::fs::read(dir.join("finish.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        Self {
            dir: dir.to_path_buf(),
            script,
            data,
            look,
            finish,
        }
    }

    pub fn save_look(&self) {
        match serde_json::to_vec_pretty(&self.look) {
            Ok(json) => self.write("look.json", &json),
            Err(e) => tracing::warn!("look : {e}"),
        }
    }

    pub fn save_finish(&self) {
        match serde_json::to_vec_pretty(&self.finish) {
            Ok(json) => self.write("finish.json", &json),
            Err(e) => tracing::warn!("finish: {e}"),
        }
    }

    /// Animation studio: compositions, clips, preview, job queue.
    pub fn studio_dir(&self) -> PathBuf {
        self.dir.join("studio")
    }

    pub fn library_dir(&self) -> PathBuf {
        self.dir.join("library")
    }

    /// Copies `source` into the library and adds it to the look.
    pub fn import_asset(&mut self, source: &Path) -> Result<(), String> {
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.add_file(source, &stem, None, None).map(|_| ())
    }

    /// Adds to the library a file from a bank (or derived from a library
    /// file), with its origin. `name`: displayed name and base of the file
    /// name. Returns the file name.
    pub fn import_found(
        &mut self,
        source: &Path,
        name: &str,
        origin: Option<Origin>,
    ) -> Result<String, String> {
        self.add_file(source, name, Some(name), origin)
    }

    fn add_file(
        &mut self,
        source: &Path,
        stem: &str,
        name: Option<&str>,
        origin: Option<Origin>,
    ) -> Result<String, String> {
        let mut asset =
            enfait_assets::library::add_file(&self.library_dir(), source, stem, name, origin)?;
        // A font is called by its family, read from the file.
        if asset.kind == AssetKind::Font
            && let Some(family) = std::fs::read(self.library_dir().join(&asset.file))
                .ok()
                .and_then(|data| enfait_motion::user::font_family(&data))
        {
            asset.name = family;
        }
        let file = asset.file.clone();
        self.look.assets.push(asset);
        self.save_look();
        Ok(file)
    }

    /// Removes a file from the library (and from disk).
    pub fn remove_asset(&mut self, file: &str) {
        self.look.assets.retain(|a| a.file != file);
        if self.finish.music.iter().any(|m| m.file == file)
            || self.finish.sfx.iter().any(|s| s.file == file)
        {
            self.finish.forget_file(file);
            self.save_finish();
        }
        let _ = std::fs::remove_file(self.library_dir().join(file));
        self.save_look();
    }

    pub fn save_script(&self) {
        self.write("script.md", self.script.as_bytes());
    }

    pub fn save_data(&self) {
        match serde_json::to_vec_pretty(&self.data) {
            Ok(json) => self.write("session.json", &json),
            Err(e) => tracing::warn!("session : {e}"),
        }
    }

    /// Atomic write (temporary file, then rename).
    fn write(&self, name: &str, data: &[u8]) {
        let result = std::fs::create_dir_all(&self.dir).and_then(|()| {
            let tmp = self.dir.join(format!("{name}.tmp"));
            std::fs::write(&tmp, data)?;
            std::fs::rename(&tmp, self.dir.join(name))
        });
        if let Err(e) = result {
            tracing::warn!("cannot save {name}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("enfait-session-{}", std::process::id()));
        let mut s = Session::load(&dir);
        assert!(s.script.is_empty() && s.data.rush.is_none());
        s.script = "Bonjour.".into();
        s.data.rush = Some("/tmp/rush.mp4".into());
        s.data.choices.insert("Bonjour.".into(), 1);
        s.save_script();
        s.save_data();
        let again = Session::load(&dir);
        assert_eq!(again.script, "Bonjour.");
        assert_eq!(again.data, s.data);

        // Library: a font (family read from the file) and an SVG.
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("ma police.ttf"),
            include_bytes!("../../../assets/motion/fonts/Inter-Variable.ttf"),
        )
        .unwrap();
        std::fs::write(src.join("logo_marque.svg"), "<svg/>").unwrap();
        std::fs::write(src.join("notes.pdf"), "x").unwrap();
        s.import_asset(&src.join("ma police.ttf")).unwrap();
        s.import_asset(&src.join("logo_marque.svg")).unwrap();
        s.import_asset(&src.join("logo_marque.svg")).unwrap();
        assert!(s.import_asset(&src.join("notes.pdf")).is_err());
        let files: Vec<&str> = s.look.assets.iter().map(|a| a.file.as_str()).collect();
        assert_eq!(
            files,
            ["ma-police.ttf", "logo_marque.svg", "logo_marque-2.svg"]
        );
        assert_eq!(s.look.assets[0].name, "Inter");
        assert_eq!(s.look.assets[1].name, "logo marque");
        // File from a bank: name taken from the title, origin kept.
        let origin = Origin {
            source: "Wikimedia Commons".into(),
            page: "https://commons.wikimedia.org/wiki/File:Train.svg".into(),
            title: "Train (icône)".into(),
            author: String::new(),
            license: enfait_core::License::new("Public domain", None),
            changes: Vec::new(),
        };
        let file = s
            .import_found(&src.join("logo_marque.svg"), "Train (icône)", Some(origin))
            .unwrap();
        assert_eq!(file, "Train--icône.svg");
        let found = s.look.assets.last().unwrap();
        assert_eq!(found.name, "Train (icône)");
        assert!(found.origin.is_some());
        s.remove_asset("logo_marque-2.svg");
        assert!(!s.library_dir().join("logo_marque-2.svg").exists());
        assert_eq!(Session::load(&dir).look, s.look);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
