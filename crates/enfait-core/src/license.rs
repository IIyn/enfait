//! Origine et licence des fichiers venus d'une banque libre de droits :
//! conditions d'utilisation lues dans le nom de la licence, avertissements
//! et crédits à publier avec la vidéo.

use crate::look::Asset;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// D'où vient un fichier de la bibliothèque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// Banque d'images (« Wikimedia Commons », « Pexels »…).
    pub source: String,
    /// Page du fichier chez la source.
    pub page: String,
    pub title: String,
    /// Auteur, tel que la source le donne (vide s'il est inconnu).
    #[serde(default)]
    pub author: String,
    pub license: License,
    /// Modifications faites par enfait (« détouré »), à signaler dans les
    /// crédits quand la licence le demande.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
}

/// Une licence, par son nom court (« CC BY-SA 4.0 », « Licence Pexels »).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct License {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Ce que permet une licence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Terms {
    /// Licence reconnue ; sinon, à vérifier à la main.
    pub known: bool,
    /// Citer l'auteur.
    pub attribution: bool,
    /// Une œuvre dérivée doit être publiée sous la même licence.
    pub share_alike: bool,
    /// Pas d'usage commercial.
    pub non_commercial: bool,
    /// Pas de modification (ni recadrage, ni détourage).
    pub no_derivatives: bool,
}

impl License {
    pub fn new(name: impl Into<String>, url: Option<String>) -> Self {
        Self {
            name: name.into(),
            url,
        }
    }

    /// Conditions lues dans le nom de la licence.
    pub fn terms(&self) -> Terms {
        let lower = self.name.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric() && c != '.')
            .filter(|w| !w.is_empty())
            .collect();
        let has = |w: &str| words.contains(&w);
        let phrase = |p: &str| lower.contains(p);
        let free = Terms {
            known: true,
            ..Terms::default()
        };
        if has("cc0")
            || phrase("public domain")
            || phrase("domaine public")
            || has("pd")
            || has("pdm")
            || has("pexels")
            || has("pixabay")
            || has("unsplash")
        {
            return free;
        }
        if has("gfdl") || phrase("free documentation") {
            return Terms {
                known: true,
                attribution: true,
                share_alike: true,
                ..Terms::default()
            };
        }
        let creative_commons = has("cc") || phrase("creative commons") || has("by");
        if !creative_commons && !phrase("attribution") {
            return Terms::default();
        }
        Terms {
            known: true,
            attribution: true,
            share_alike: has("sa") || phrase("share alike") || phrase("sharealike"),
            non_commercial: has("nc")
                || phrase("noncommercial")
                || phrase("non commercial")
                || phrase("non-commercial"),
            no_derivatives: has("nd")
                || phrase("noderiv")
                || phrase("no deriv")
                || phrase("no-deriv"),
        }
    }
}

impl Terms {
    /// Le fichier peut être modifié (détouré, recadré).
    pub const fn allows_changes(&self) -> bool {
        !self.no_derivatives
    }

    /// Ce qu'il faut savoir avant de publier, en clair.
    pub fn warnings(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.known {
            out.push("licence non reconnue : vérifie-la sur la page du fichier");
        }
        if self.non_commercial {
            out.push("pas d'usage commercial (vidéo monétisée, publicité)");
        }
        if self.no_derivatives {
            out.push("pas de modification : à montrer entier, sans recadrage ni détourage");
        }
        if self.share_alike {
            out.push("partage dans les mêmes conditions : la vidéo doit reprendre cette licence");
        }
        out
    }

    /// La licence peut interdire certains usages (commercial, modification)
    /// ou n'est pas reconnue : à signaler avant tout ajout.
    pub const fn restrictive(&self) -> bool {
        !self.known || self.non_commercial || self.no_derivatives
    }

    /// Résumé court pour une liste (« citer l'auteur », « libre »).
    pub fn summary(&self) -> &'static str {
        match (self.known, self.attribution) {
            (false, _) => "à vérifier",
            (true, false) => "libre",
            (true, true) => "citer l'auteur",
        }
    }
}

/// Crédits à publier avec la vidéo (description, générique) : un fichier
/// par ligne, ceux qui demandent une attribution d'abord. Vide si aucun
/// fichier ne vient d'une banque.
pub fn credits(assets: &[Asset]) -> String {
    let mut origins: Vec<&Origin> = assets.iter().filter_map(|a| a.origin.as_ref()).collect();
    if origins.is_empty() {
        return String::new();
    }
    origins.sort_by_key(|o| !o.license.terms().attribution);
    let mut out = String::from("Crédits\n");
    for o in origins {
        let _ = write!(out, "- « {} »", o.title.trim());
        if !o.author.trim().is_empty() {
            let _ = write!(out, ", {}", o.author.trim());
        }
        let _ = write!(out, ", {}", o.license.name);
        if let Some(url) = &o.license.url {
            let _ = write!(out, " ({url})");
        }
        if !o.changes.is_empty() {
            let _ = write!(out, ", {}", o.changes.join(", "));
        }
        let _ = writeln!(out, " — {} : {}", o.source, o.page);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::AssetKind;

    fn terms(name: &str) -> Terms {
        License::new(name, None).terms()
    }

    #[test]
    fn reads_licence_names() {
        for free in [
            "CC0 1.0",
            "Public domain",
            "PDM 1.0",
            "Licence Pexels",
            "Pixabay Content License",
        ] {
            let t = terms(free);
            assert!(
                t.known && !t.attribution && t.warnings().is_empty(),
                "{free}"
            );
        }
        let by = terms("CC BY 4.0");
        assert!(by.known && by.attribution && !by.share_alike && by.allows_changes());
        let sa = terms("CC BY-SA 3.0");
        assert!(sa.share_alike && !sa.non_commercial);
        let long = terms("Creative Commons Attribution-Share Alike 3.0");
        assert!(long.attribution && long.share_alike);
        let nc_nd = terms("CC BY-NC-ND 2.0");
        assert!(nc_nd.non_commercial && nc_nd.no_derivatives && !nc_nd.allows_changes());
        assert_eq!(nc_nd.warnings().len(), 2);
        let gfdl = terms("GFDL 1.2");
        assert!(gfdl.attribution && gfdl.share_alike);
        let unknown = terms("Tous droits réservés");
        assert!(!unknown.known);
        assert_eq!(unknown.summary(), "à vérifier");
    }

    #[test]
    fn credits_list_attribution_first() {
        let asset = |file: &str, origin: Option<Origin>| Asset {
            file: file.into(),
            kind: AssetKind::Image,
            name: file.into(),
            description: String::new(),
            origin,
        };
        let origin = |title: &str, licence: &str| Origin {
            source: "Wikimedia Commons".into(),
            page: format!("https://commons.wikimedia.org/wiki/File:{title}"),
            title: title.into(),
            author: "Whpq".into(),
            license: License::new(licence, None),
            changes: Vec::new(),
        };
        assert!(credits(&[asset("logo.svg", None)]).is_empty());
        let mut train = origin("Train.webm", "CC BY-SA 3.0");
        train.changes.push("détouré".into());
        let text = credits(&[
            asset("logo.svg", None),
            asset("chat.jpg", Some(origin("Chat.jpg", "CC0 1.0"))),
            asset("train.webm", Some(train)),
        ]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("- « Train.webm », Whpq, CC BY-SA 3.0, détouré — "));
        assert!(lines[2].contains("Chat.jpg"));
    }
}
