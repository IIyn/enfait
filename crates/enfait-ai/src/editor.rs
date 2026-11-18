//! Passe « monteur » : le storyboard écrit par le modèle (type et
//! description de chaque bloc, moments précis, transitions, plans réels à
//! chercher, effets sonores et musique). C'est le bouton « écrire le
//! storyboard » et le cœur du mode magique.

use crate::llm::{AiError, Effort, Part, Request};
use crate::text::{fill, last_fenced};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

pub(crate) const EDITOR: &str = include_str!("prompts/editor.md");

/// Le storyboard proposé.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditPlan {
    pub blocks: Vec<BlockPlan>,
    pub music: Option<MusicPlan>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockPlan {
    /// Numéro du bloc, à partir de 1.
    pub index: usize,
    /// `animation`, `mixte`, `effet`, `rush` ou `broll`.
    pub kind: String,
    pub description: String,
    pub moments: Vec<MomentPlan>,
    pub broll: Option<SearchPlan>,
    pub transition: Option<TransitionPlan>,
    pub sfx: Vec<SfxPlan>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MomentPlan {
    /// Mot du bloc, tel qu'il est écrit.
    pub word: String,
    pub note: String,
}

/// Un plan réel à chercher.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchPlan {
    /// Requête courte, en anglais.
    pub query: String,
    /// `video` ou `image`.
    pub media: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionPlan {
    /// `coupe`, `auto` ou `decrite`.
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SfxPlan {
    /// Mot du bloc sur lequel tombe le son (vide : début du bloc).
    pub word: String,
    /// Requête courte, en anglais.
    pub query: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicPlan {
    pub query: String,
    pub volume_db: Option<f32>,
}

/// Ce qu'on voit pendant un bloc, en clair pour le modèle et pour le code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Animation,
    Mixed,
    Effect,
    Rush,
    Broll,
}

impl PlanKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Animation => "animation",
            Self::Mixed => "mixte",
            Self::Effect => "effet",
            Self::Rush => "rush",
            Self::Broll => "broll",
        }
    }

    /// Lit un type écrit par le modèle (accents et variantes anglaises
    /// acceptés).
    pub fn parse(text: &str) -> Option<Self> {
        Some(
            match text
                .trim()
                .to_lowercase()
                .replace(['é', 'è'], "e")
                .replace(['-', '_', ' '], "")
                .as_str()
            {
                "animation" | "anim" => Self::Animation,
                "mixte" | "mixed" | "mix" => Self::Mixed,
                "effet" | "effect" => Self::Effect,
                "rush" | "facecam" | "brut" => Self::Rush,
                "broll" | "plan" | "stock" => Self::Broll,
                _ => return None,
            },
        )
    }
}

impl BlockPlan {
    pub fn plan_kind(&self) -> Option<PlanKind> {
        PlanKind::parse(&self.kind)
    }
}

/// Un bloc tel que le monteur le voit.
pub struct EditorBlock {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Écrit par l'auteur (type et description) : à garder tel quel.
    pub authored: Option<(PlanKind, String)>,
}

pub struct EditorInput<'a> {
    pub width: u32,
    pub height: u32,
    /// Langue de la voix (`fr`, `en`…), vide si inconnue.
    pub language: &'a str,
    pub blocks: &'a [EditorBlock],
    /// La prise a des images (personne filmée).
    pub has_video: bool,
    /// Des banques de vidéos sont disponibles (clés Pexels ou Pixabay).
    pub video_banks: bool,
    pub script: Option<&'a str>,
    pub user_style: Option<&'a str>,
    /// Fichiers de la bibliothèque, décrits (un par ligne).
    pub library: Option<&'a str>,
    pub max_transitions: usize,
}

pub fn editor_request(input: &EditorInput<'_>, model: &str, effort: Effort) -> Request {
    let kinds = if input.has_video {
        "La personne est filmée : garde-la à l'écran sur une partie des blocs (`rush`, \
         `mixte`, `effet`), par exemple à l'ouverture, sur les moments de conviction et en \
         conclusion, et passe en `animation` ou en `broll` pour expliquer et montrer. Un \
         bloc `rush` ne coûte rien et repose l'œil ; plusieurs blocs filmés d'affilée \
         lassent."
    } else {
        "Il n'y a pas d'image filmée (voix seule) : n'utilise que `animation` et `broll`."
    };
    let broll = if input.video_banks {
        "Des banques de vidéos (Pexels, Pixabay) sont disponibles."
    } else {
        "Seules Wikimedia Commons et Openverse sont disponibles : peu de vidéos, surtout des \
         photos et des documents. Garde `broll` pour ce qui s'y trouve sûrement (monuments, \
         personnages historiques, lieux connus), en `image` le plus souvent."
    };
    let authored = input.blocks.iter().any(|b| b.authored.is_some());
    let fill_rule = if authored {
        "## Blocs déjà écrits\n\nL'auteur a déjà écrit certains blocs (marqués « écrit par \
         l'auteur »). Reprends leur type et leur description **tels quels** dans ta réponse \
         (tu peux leur ajouter moments, effets sonores et transition) et écris les autres en \
         cohérence avec eux."
    } else {
        ""
    };
    let system = fill(
        EDITOR,
        &[
            ("KINDS", kinds),
            ("BROLL", broll),
            ("FILL", fill_rule),
            ("MAX_TRANSITIONS", &input.max_transitions.to_string()),
        ],
    );
    let duration = input.blocks.last().map_or(0.0, |b| b.end);
    let mut user = format!(
        "Format : {}×{} px. Durée : {duration:.1} s. {} blocs.{}{}\n\n## Blocs\n\n",
        input.width,
        input.height,
        input.blocks.len(),
        if input.language.is_empty() {
            String::new()
        } else {
            format!(
                " Langue de la voix : {} (les textes à l'écran sont dans cette langue).",
                input.language
            )
        },
        crate::animate::format_notes(input.width, input.height),
    );
    for (i, block) in input.blocks.iter().enumerate() {
        let _ = writeln!(
            user,
            "{}. [{:.2} s → {:.2} s, {:.1} s] {}",
            i.saturating_add(1),
            block.start,
            block.end,
            block.end - block.start,
            block.text
        );
        if let Some((kind, description)) = &block.authored {
            let _ = writeln!(
                user,
                "   (écrit par l'auteur : type `{}`, « {} »)",
                kind.name(),
                description.trim()
            );
        }
    }
    if let Some(script) = input.script.filter(|s| !s.trim().is_empty()) {
        let _ = write!(user, "\n## Script d'origine\n\n{script}\n");
    }
    if let Some(style) = input.user_style {
        let _ = write!(
            user,
            "\n## Style imposé (règles fixes, prioritaires)\n\n{style}\n"
        );
    }
    if let Some(library) = input.library {
        let _ = write!(
            user,
            "\n## Bibliothèque de l'auteur\n\nFichiers que les animateurs peuvent utiliser ; \
             cite-les par leur nom dans une description quand ils servent. Une vidéo ou une \
             image de la bibliothèque peut aussi être le plan d'un bloc `broll` : donne alors \
             son nom de fichier comme requête.\n\n{library}\n"
        );
    }
    Request {
        model: model.to_owned(),
        effort,
        max_tokens: 32_000,
        system,
        user: vec![Part::Text(user)],
    }
}

/// Lit le storyboard dans la réponse du monteur et l'ajuste à ce qui est
/// permis : blocs hors de la vidéo retirés, types inconnus ou impossibles
/// (personne filmée sans image) ramenés à `animation`, un seul plan par
/// numéro.
pub fn parse_plan(reply: &str, block_count: usize, has_video: bool) -> Result<EditPlan, AiError> {
    let json = last_fenced(reply, "json")
        .or_else(|| {
            let start = reply.find('{')?;
            let end = reply.rfind('}')?.checked_add(1)?;
            reply.get(start..end)
        })
        .ok_or_else(|| AiError::Unexpected("pas de JSON dans la réponse du monteur".into()))?;
    let mut plan: EditPlan = serde_json::from_str(json)?;
    let mut seen = vec![false; block_count];
    plan.blocks.retain(|b| {
        let slot = b.index.checked_sub(1).and_then(|i| seen.get_mut(i));
        match slot {
            Some(seen) if !*seen => {
                *seen = true;
                true
            }
            _ => false,
        }
    });
    plan.blocks.sort_by_key(|b| b.index);
    for block in &mut plan.blocks {
        let kind = match block.plan_kind() {
            Some(PlanKind::Mixed | PlanKind::Effect | PlanKind::Rush) if !has_video => {
                PlanKind::Animation
            }
            Some(PlanKind::Broll)
                if block
                    .broll
                    .as_ref()
                    .is_none_or(|b| b.query.trim().is_empty()) =>
            {
                PlanKind::Animation
            }
            Some(kind) => kind,
            None => PlanKind::Animation,
        };
        block.kind = kind.name().to_owned();
        block
            .moments
            .retain(|m| !m.word.trim().is_empty() && !m.note.trim().is_empty());
        block.moments.truncate(2);
        block.sfx.retain(|s| !s.query.trim().is_empty());
        block.sfx.truncate(2);
    }
    if plan
        .music
        .as_ref()
        .is_some_and(|m| m.query.trim().is_empty())
    {
        plan.music = None;
    }
    if plan.blocks.len() < block_count {
        tracing::warn!(
            "le monteur a écrit {} blocs sur {block_count}, les autres restent tels quels",
            plan.blocks.len()
        );
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> EditorBlock {
        EditorBlock {
            start: 0.0,
            end: 3.0,
            text: text.into(),
            authored: None,
        }
    }

    #[test]
    fn request_adapts_to_rush_and_authored_blocks() {
        let mut blocks = vec![block("Bonjour à tous."), block("La gare.")];
        let input = EditorInput {
            width: 1920,
            height: 1080,
            language: "fr",
            blocks: &blocks,
            has_video: false,
            video_banks: false,
            script: None,
            user_style: None,
            library: None,
            max_transitions: 2,
        };
        let req = editor_request(&input, "m", Effort::Medium);
        assert!(!req.system.contains("{{"));
        assert!(req.system.contains("voix seule"));
        assert!(!req.system.contains("Blocs déjà écrits"));
        blocks[0].authored = Some((PlanKind::Rush, "face caméra".into()));
        let input = EditorInput {
            width: 1920,
            height: 1080,
            language: "fr",
            blocks: &blocks,
            has_video: true,
            video_banks: false,
            script: None,
            user_style: None,
            library: None,
            max_transitions: 2,
        };
        let req = editor_request(&input, "m", Effort::Medium);
        assert!(req.system.contains("La personne est filmée"));
        assert!(req.system.contains("Blocs déjà écrits"));
        let Part::Text(user) = &req.user[0] else {
            panic!()
        };
        assert!(user.contains("écrit par l'auteur : type `rush`, « face caméra »"));
        assert!(user.contains("Langue de la voix : fr"));
    }

    #[test]
    fn plan_is_checked_against_the_video() {
        let reply = r#"Réflexion.
```json
{"blocks":[
 {"index":2,"kind":"Mixte","description":"titre à côté"},
 {"index":1,"kind":"b-roll","description":"gare","broll":{"query":"train station","media":"video"},
  "sfx":[{"word":"gare","query":"train horn"},{"query":""}],
  "moments":[{"word":"gare","note":"le train entre"},{"word":"","note":"x"}],
  "transition":{"type":"auto"}},
 {"index":2,"kind":"rush"},
 {"index":7,"kind":"rush"},
 {"index":3,"kind":"broll","broll":{"query":"  "}}
],
 "music":{"query":"calm piano","volume_db":-20}}
```"#;
        let plan = parse_plan(reply, 3, false).unwrap();
        assert_eq!(plan.blocks.len(), 3, "doublon et bloc hors vidéo retirés");
        assert_eq!(plan.blocks[0].index, 1);
        assert_eq!(plan.blocks[0].kind, "broll");
        assert_eq!(plan.blocks[0].sfx.len(), 1);
        assert_eq!(plan.blocks[0].moments.len(), 1);
        assert_eq!(plan.blocks[1].kind, "animation", "pas de prise filmée");
        assert_eq!(plan.blocks[2].kind, "animation", "plan sans requête");
        assert_eq!(plan.music.unwrap().volume_db, Some(-20.0));
        let filmed = parse_plan(reply, 3, true).unwrap();
        assert_eq!(filmed.blocks[1].kind, "mixte");
    }

    #[test]
    fn kinds_are_read_loosely() {
        assert_eq!(PlanKind::parse("Effet"), Some(PlanKind::Effect));
        assert_eq!(PlanKind::parse("B-Roll"), Some(PlanKind::Broll));
        assert_eq!(PlanKind::parse("face cam"), Some(PlanKind::Rush));
        assert_eq!(PlanKind::parse("dessin"), None);
    }
}
