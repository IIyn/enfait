//! Style presets: fixed rules (material, colour, typography, motion,
//! gestures specific to the medium, pitfalls) that the direction follows,
//! everything else staying free.

pub struct Preset {
    pub slug: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub rules: &'static str,
}

pub static PRESETS: &[Preset] = &[
    Preset {
        slug: "engraving",
        name: "Archive engraving",
        summary: "19th-century plates, hatching, a single vermilion",
        rules: include_str!("styles/engraving.md"),
    },
    Preset {
        slug: "paper",
        name: "Paper cut-out",
        summary: "paper shapes, short shadows, animated on twos (12 fps)",
        rules: include_str!("styles/paper.md"),
    },
    Preset {
        slug: "swiss",
        name: "Swiss typography",
        summary: "grid, asymmetry, text only, one red",
        rules: include_str!("styles/swiss.md"),
    },
    Preset {
        slug: "chalk",
        name: "Chalkboard",
        summary: "hand-drawn diagrams on a slate",
        rules: include_str!("styles/chalk.md"),
    },
];

/// Preset by slug; the former French slugs of saved sessions still work.
pub fn find(slug: &str) -> Option<&'static Preset> {
    let slug = match slug {
        "gravure" => "engraving",
        "papier" => "paper",
        "suisse" => "swiss",
        "craie" => "chalk",
        other => other,
    };
    PRESETS.iter().find(|p| p.slug == slug)
}

/// Imposed style rules: the preset, then the user's rules, which win in
/// case of conflict.
pub fn combine(preset: Option<&Preset>, user: Option<&str>) -> Option<String> {
    match (preset, user) {
        (None, None) => None,
        (Some(p), None) => Some(p.rules.to_owned()),
        (None, Some(u)) => Some(u.to_owned()),
        (Some(p), Some(u)) => Some(format!(
            "{}\n\n# User's rules (take precedence over the style)\n\n{u}",
            p.rules
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_follow_the_template() {
        for p in PRESETS {
            for section in ["## Essence", "## Motion", "## Pitfalls", "## Freedom"] {
                assert!(p.rules.contains(section), "{}: {section}", p.slug);
            }
            assert!(!p.rules.contains("power3"), "{}: GSAP easing name", p.slug);
        }
        assert!(find("chalk").is_some());
        assert_eq!(find("craie").map(|p| p.slug), Some("chalk"), "former slug");
        assert!(
            combine(find("chalk"), Some("always in pink"))
                .unwrap()
                .contains("pink")
        );
    }
}
