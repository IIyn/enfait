//! Styles prédéfinis : des règles fixes (matière, couleur, typographie,
//! mouvement, gestes propres au médium, pièges) que la direction respecte,
//! tout le reste restant libre.

pub struct Preset {
    pub slug: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub rules: &'static str,
}

pub static PRESETS: &[Preset] = &[
    Preset {
        slug: "gravure",
        name: "Gravure d'archives",
        summary: "planches du XIXᵉ siècle, hachures, un seul vermillon",
        rules: include_str!("styles/gravure.md"),
    },
    Preset {
        slug: "papier",
        name: "Papier découpé",
        summary: "formes en papier, ombres courtes, animé à 12 images/s",
        rules: include_str!("styles/papier.md"),
    },
    Preset {
        slug: "suisse",
        name: "Typographie suisse",
        summary: "grille, asymétrie, texte seul, un rouge",
        rules: include_str!("styles/suisse.md"),
    },
    Preset {
        slug: "craie",
        name: "Tableau à la craie",
        summary: "schémas dessinés à la main sur ardoise",
        rules: include_str!("styles/craie.md"),
    },
];

pub fn find(slug: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.slug == slug)
}

/// Règles de style imposées : le style prédéfini puis les règles de
/// l'utilisateur, qui l'emportent en cas de conflit.
pub fn combine(preset: Option<&Preset>, user: Option<&str>) -> Option<String> {
    match (preset, user) {
        (None, None) => None,
        (Some(p), None) => Some(p.rules.to_owned()),
        (None, Some(u)) => Some(u.to_owned()),
        (Some(p), Some(u)) => Some(format!(
            "{}\n\n# Règles de l'utilisateur (l'emportent sur le style)\n\n{u}",
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
            for section in ["## Essence", "## Mouvement", "## Pièges", "## Liberté"] {
                assert!(p.rules.contains(section), "{} : {section}", p.slug);
            }
            assert!(
                !p.rules.contains("power3"),
                "{} : nom de courbe GSAP",
                p.slug
            );
        }
        assert!(find("craie").is_some());
        assert!(
            combine(find("craie"), Some("toujours en rose"))
                .unwrap()
                .contains("rose")
        );
    }
}
