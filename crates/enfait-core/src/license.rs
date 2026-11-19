//! Origin and licence of files from a royalty-free bank: terms of use read
//! from the licence name, warnings and credits to publish with the video.

use crate::look::Asset;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// Where a library file comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// Image bank ("Wikimedia Commons", "Pexels"…).
    pub source: String,
    /// Page of the file at the source.
    pub page: String,
    pub title: String,
    /// Author, as given by the source (empty if unknown).
    #[serde(default)]
    pub author: String,
    pub license: License,
    /// Changes made by enfait ("cut out"), to mention in the credits when the
    /// licence asks for it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
}

/// A licence, by its short name ("CC BY-SA 4.0", "Pexels License").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct License {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// What a licence allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Terms {
    /// Recognised licence; otherwise, to check by hand.
    pub known: bool,
    /// Credit the author.
    pub attribution: bool,
    /// A derived work must be published under the same licence.
    pub share_alike: bool,
    /// No commercial use.
    pub non_commercial: bool,
    /// No changes (no cropping, no cutout).
    pub no_derivatives: bool,
}

impl License {
    pub fn new(name: impl Into<String>, url: Option<String>) -> Self {
        Self {
            name: name.into(),
            url,
        }
    }

    /// Terms read from the licence name.
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
    /// The file can be changed (cut out, cropped).
    pub const fn allows_changes(&self) -> bool {
        !self.no_derivatives
    }

    /// What to know before publishing, in plain words.
    pub fn warnings(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.known {
            out.push("unrecognised licence: check it on the file page");
        }
        if self.non_commercial {
            out.push("no commercial use (monetised video, advertising)");
        }
        if self.no_derivatives {
            out.push("no changes: show it whole, without cropping or cutout");
        }
        if self.share_alike {
            out.push("share alike: the video must use the same licence");
        }
        out
    }

    /// The licence may forbid some uses (commercial, changes) or is not
    /// recognised: to point out before adding the file.
    pub const fn restrictive(&self) -> bool {
        !self.known || self.non_commercial || self.no_derivatives
    }

    /// Short summary for a list ("credit the author", "free").
    pub fn summary(&self) -> &'static str {
        match (self.known, self.attribution) {
            (false, _) => "to check",
            (true, false) => "free",
            (true, true) => "credit the author",
        }
    }
}

/// Credits to publish with the video (description, end credits): one file
/// per line, those requiring attribution first. Empty if no file comes
/// from a bank.
pub fn credits(assets: &[Asset]) -> String {
    let mut origins: Vec<&Origin> = assets.iter().filter_map(|a| a.origin.as_ref()).collect();
    if origins.is_empty() {
        return String::new();
    }
    origins.sort_by_key(|o| !o.license.terms().attribution);
    let mut out = String::from("Credits\n");
    for o in origins {
        let _ = write!(out, "- “{}”", o.title.trim());
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
        assert_eq!(unknown.summary(), "to check");
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
        train.changes.push("cut out".into());
        let text = credits(&[
            asset("logo.svg", None),
            asset("chat.jpg", Some(origin("Chat.jpg", "CC0 1.0"))),
            asset("train.webm", Some(train)),
        ]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("- “Train.webm”, Whpq, CC BY-SA 3.0, cut out — "));
        assert!(lines[2].contains("Chat.jpg"));
    }
}
