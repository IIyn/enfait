//! Agents animateurs : création, correction et critique d'une composition.

use crate::direction::STYLE_GUIDE;
use crate::llm::{Effort, Part, Request};
use crate::text::fill;
use std::fmt::Write as _;
use std::path::PathBuf;

pub(crate) const ANIMATOR: &str = include_str!("prompts/animator.md");

/// Consigne système commune à tous les appels d'animation. Identique pour
/// tous les blocs d'une vidéo, elle profite du cache de prompt.
pub fn animator_system(contract: &str, fonts: &str) -> String {
    fill(
        ANIMATOR,
        &[
            ("STYLE_GUIDE", STYLE_GUIDE),
            ("FONTS", fonts),
            ("CONTRACT", contract),
        ],
    )
}

#[derive(Debug, Clone)]
pub struct TimedWord {
    /// Début du mot, en secondes depuis le début de la scène.
    pub t: f64,
    pub text: String,
}

/// Tout ce qu'un animateur doit savoir sur sa scène.
#[derive(Debug, Clone)]
pub struct SceneBrief {
    pub index: usize,
    pub total: usize,
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub text: String,
    pub words: Vec<TimedWord>,
    pub art_direction: String,
    pub intent: String,
    pub previous: Option<String>,
    pub next: Option<String>,
    pub user_style: Option<String>,
    /// Fichiers de l'utilisateur utilisables dans la scène (liste
    /// Markdown : chemin et description), voir [`library_brief`].
    pub library: Option<String>,
    /// Moments précis voulus par l'utilisateur : instant (s, depuis le
    /// début de la scène), mot prononcé, ce qui doit se passer.
    pub moments: Vec<(f64, String, String)>,
    /// Scène avec la prise filmée (voir [`SceneLayer`]).
    pub layer: Option<SceneLayer>,
}

/// Une scène qui montre la prise filmée, fournie en image par le moteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneLayer {
    /// L'animation accompagne la personne filmée (à côté ou par-dessus).
    Mixed,
    /// Comme `Mixed`, avec la personne détourée en plus : l'animation peut
    /// passer derrière elle.
    MixedCutout,
    /// Un effet transforme la prise elle-même.
    Effect,
}

impl SceneLayer {
    fn brief(self) -> &'static str {
        match self {
            Self::Mixed => {
                "Scène MIXTE : la prise filmée (face cam) est fournie à chaque image dans \
                 `img[data-enfait-scene=\"from\"]` (voir le contrat). Elle reste visible et \
                 lisible : au moins la moitié de l'écran, le visage jamais couvert. \
                 L'animation vient à côté (écran partagé, panneau) ou par-dessus (éléments \
                 légers), dans la direction artistique de la vidéo."
            }
            Self::MixedCutout => {
                "Scène MIXTE AVEC DÉTOURAGE : la prise filmée (face cam) est fournie à chaque \
                 image dans `img[data-enfait-scene=\"from\"]`, et la personne seule, détourée \
                 (fond transparent, exactement alignée sur `from`), dans \
                 `img[data-enfait-scene=\"to\"]`. Empile : `from` au fond, tes éléments \
                 (grand titre, formes, chiffres) au milieu, `to` par-dessus : ils passent \
                 derrière la personne. Tu peux aussi remplacer le décor : un fond à toi, puis \
                 tes éléments, puis `to`, sans montrer `from`. Ne transforme pas `to` \
                 autrement que `from` (même position, même taille), le visage jamais couvert."
            }
            Self::Effect => {
                "Scène EFFET : la prise filmée est fournie à chaque image dans \
                 `img[data-enfait-scene=\"from\"]` (voir le contrat). Elle reste le sujet : \
                 applique-lui un effet qui sert la phrase (recadrage, zoom lent, cadre, mise en \
                 couleur, mot en surimpression, transformation de l'image) sans la remplacer."
            }
        }
    }
}

/// Consignes propres au format : pour une vidéo verticale (Shorts, Reels,
/// TikTok), les zones couvertes par l'interface des applications.
pub fn format_notes(width: u32, height: u32) -> &'static str {
    if height > width {
        "\n\nVidéo verticale pour téléphone (Shorts, Reels, TikTok) : l'interface de \
         l'application couvre le haut (environ 8 % de la hauteur), le bas (environ 20 %) et \
         la droite (environ 12 % de la largeur). Garde textes et éléments importants dans la \
         zone centrale restante ; textes grands et courts, lisibles sur un petit écran."
    } else {
        ""
    }
}

impl SceneBrief {
    pub fn render(&self) -> String {
        let mut s = format!(
            "# Scène {} sur {}\n\nFormat : {}×{} px. Durée : **{:.3} s** \
             (`window.enfait.duration = {:.3}`).{}\n\n## Direction artistique de la vidéo\n\n{}\n\n\
             ## Intention pour cette scène\n\n{}\n\n## Ce que dit la voix\n\n« {} »\n\n\
             ## Instants des mots (secondes depuis le début de la scène)\n\n",
            self.index,
            self.total,
            self.width,
            self.height,
            self.duration,
            self.duration,
            format_notes(self.width, self.height),
            self.art_direction,
            if self.intent.trim().is_empty() {
                "(libre, dans l'esprit de la direction)"
            } else {
                self.intent.as_str()
            },
            self.text,
        );
        for w in &self.words {
            let _ = writeln!(s, "- {:.2} {}", w.t, w.text);
        }
        if let Some(layer) = self.layer {
            let _ = write!(s, "\n## Type de scène\n\n{}\n", layer.brief());
        }
        if !self.moments.is_empty() {
            s.push_str(
                "\n## Moments précis (voulus par l'utilisateur, à respecter à l'instant dit)\n\n",
            );
            for (t, word, note) in &self.moments {
                let _ = writeln!(s, "- à {t:.2} s, sur « {word} » : {note}");
            }
        }
        if let Some(prev) = &self.previous {
            let _ = write!(s, "\n## Scène précédente\n\n{prev}\n");
        }
        if let Some(next) = &self.next {
            let _ = write!(s, "\n## Scène suivante\n\n{next}\n");
        }
        if let Some(style) = &self.user_style {
            let _ = write!(
                s,
                "\n## Style imposé (règles fixes, prioritaires)\n\n{style}\n"
            );
        }
        if let Some(library) = &self.library {
            let _ = write!(
                s,
                "\n## Bibliothèque de l'utilisateur\n\nFichiers fournis par l'utilisateur, \
                 à utiliser quand ils servent la scène (chemins relatifs à la composition ; \
                 ne les déforme pas, ne les recolorise pas sauf demande) :\n\n{library}\n"
            );
        }
        s
    }
}

/// Liste Markdown de la bibliothèque : `(chemin, type, nom, description)`.
pub fn library_brief(items: &[(String, &str, String, String)]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut s = String::new();
    for (path, kind, name, description) in items {
        let _ = write!(s, "- `{path}` ({kind}) : {name}");
        if !description.trim().is_empty() {
            let _ = write!(s, " — {}", description.trim());
        }
        s.push('\n');
    }
    Some(s)
}

/// Paramètres communs des appels d'animation.
#[derive(Debug, Clone)]
pub struct AnimatorConfig {
    pub system: String,
    pub model: String,
    pub effort: Effort,
}

impl AnimatorConfig {
    fn request(&self, user: Vec<Part>) -> Request {
        Request {
            model: self.model.clone(),
            effort: self.effort,
            max_tokens: 64_000,
            system: self.system.clone(),
            user,
        }
    }

    pub fn create(&self, brief: &str) -> Request {
        self.request(vec![Part::Text(format!(
            "Mode : CRÉER.\n\n{brief}\n\nRéalise la composition."
        ))])
    }

    pub fn fix(&self, brief: &str, html: &str, problems: &[String]) -> Request {
        self.request(vec![Part::Text(format!(
            "Mode : CORRIGER.\n\n{brief}\n\n## Composition actuelle\n\n```html\n{html}\n```\n\n\
             ## Problèmes relevés au rendu\n\n- {}\n\nCorrige ces problèmes sans dégrader la scène \
             et renvoie le document complet.",
            problems.join("\n- ")
        ))])
    }

    /// `samples` : instants (s) et images rendues correspondantes.
    pub fn critique(
        &self,
        brief: &str,
        html: &str,
        samples: &[(f64, PathBuf)],
        warnings: &[String],
    ) -> Request {
        let mut parts = vec![Part::Text(format!(
            "Mode : CRITIQUER.\n\n{brief}\n\n## Composition actuelle\n\n```html\n{html}\n```\n\n\
             ## Images rendues\n\nVoici {} images de la scène, réduites de moitié.",
            samples.len()
        ))];
        for (t, path) in samples {
            parts.push(Part::Text(format!("Image à t = {t:.2} s :")));
            parts.push(Part::Image(path.clone()));
        }
        let mut closing = String::new();
        if !warnings.is_empty() {
            let _ = write!(
                closing,
                "## Remarques automatiques\n\n- {}\n\n",
                warnings.join("\n- ")
            );
        }
        closing.push_str(
            "Juge la scène : lisibilité, calage sur la voix, composition, qualité du mouvement \
             (déduis-le du code et des images), respect de la direction et du guide. Si un style \
             est imposé, vérifie un par un ses règles fixes et ses pièges.",
        );
        parts.push(Part::Text(closing));
        self.request(parts)
    }

    /// Retouche demandée par l'utilisateur : `notes` = (instant en s depuis
    /// le début de la scène, remarque).
    pub fn revise(&self, brief: &str, html: &str, notes: &[(f64, String)]) -> Request {
        let mut list = String::new();
        for (t, note) in notes {
            let _ = writeln!(list, "- à {t:.2} s : {note}");
        }
        self.request(vec![Part::Text(format!(
            "Mode : RETOUCHER.\n\n{brief}\n\n## Composition actuelle\n\n```html\n{html}\n```\n\n\
             ## Retouches demandées par l'utilisateur\n\n{list}\n\
             Applique ces retouches à l'instant indiqué, sans changer le reste, et renvoie le \
             document complet."
        ))])
    }

    pub fn transition(&self, brief: &TransitionBrief) -> Request {
        self.request(vec![Part::Text(brief.render())])
    }
}

/// Verdict d'une critique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Redo,
}

/// Lit le verdict ; sans ligne `VERDICT`, on considère qu'il faut refaire
/// seulement si la réponse contient une nouvelle composition.
pub fn parse_verdict(reply: &str) -> Verdict {
    for line in reply.lines().take(10) {
        let upper = line.trim().trim_matches('*').to_uppercase();
        if let Some(rest) = upper.strip_prefix("VERDICT") {
            return if rest.contains("REFAIRE") {
                Verdict::Redo
            } else {
                Verdict::Keep
            };
        }
    }
    if reply.contains("```html") {
        Verdict::Redo
    } else {
        Verdict::Keep
    }
}

/// Brief d'une transition entre deux scènes.
#[derive(Debug, Clone)]
pub struct TransitionBrief {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub art_direction: String,
    pub description: String,
    pub from_intent: String,
    pub to_intent: String,
}

impl TransitionBrief {
    pub fn render(&self) -> String {
        format!(
            "Mode : CRÉER (transition).\n\n# Transition entre deux scènes\n\n\
             Format : {}×{} px. Durée : **{:.3} s** (`window.enfait.duration = {:.3}`).{}\n\n\
             Cette composition relie deux scènes déjà rendues, fournies en images \
             (`img[data-enfait-scene=\"from\"]` et `img[data-enfait-scene=\"to\"]`, voir le \
             contrat). À t = 0 on ne voit que `from` ; à la fin, que `to`. Ajoute tes propres \
             éléments par-dessus si la transition le demande.\n\n\
             ## Direction artistique\n\n{}\n\n## Transition voulue\n\n{}\n\n\
             ## Scène de départ\n\n{}\n\n## Scène d'arrivée\n\n{}\n\nRéalise la composition.",
            self.width,
            self.height,
            self.duration,
            self.duration,
            format_notes(self.width, self.height),
            self.art_direction,
            self.description,
            self.from_intent,
            self.to_intent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        assert_eq!(parse_verdict("VERDICT: GARDER\nBien."), Verdict::Keep);
        assert_eq!(
            parse_verdict("**VERDICT : REFAIRE**\n```html\n"),
            Verdict::Redo
        );
        assert_eq!(
            parse_verdict("Je refais.\n```html\n<html></html>\n```"),
            Verdict::Redo
        );
        assert_eq!(parse_verdict("Parfait."), Verdict::Keep);
    }

    #[test]
    fn system_has_no_placeholder() {
        let s = animator_system("CONTRAT", "- Inter");
        assert!(!s.contains("{{"));
        assert!(s.contains("CONTRAT") && s.contains("Inter"));
    }
}
