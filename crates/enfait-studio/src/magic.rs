//! « Écrire le storyboard » et mode magique : la passe monteur, la
//! recherche des plans réels et des sons qu'elle demande, puis leur
//! application au projet (une seule étape d'annulation) et à la finition.
//!
//! Commun à l'interface, au CLI et au serveur MCP ; le code décide de tout
//! ce qui n'est pas du goût : un plan ou un son n'est retenu que si sa
//! licence permet de l'utiliser sans condition gênante (ni usage non
//! commercial, ni interdiction de modifier, ni partage à l'identique).

use anyhow::Result;
use enfait_ai::editor::{EditPlan, EditorInput, PlanKind, editor_request, parse_plan};
use enfait_ai::{Effort, Provider, Quality};
use enfait_assets::search::{Hit, Media};
use enfait_core::{
    Asset, AssetKind, Block, BlockKind, Broll, Edit, Finish, Flicks, Look, Moment, Project,
    Transcript, Transition,
};
use std::collections::BTreeMap;
use std::path::Path;

/// Un effet sonore plus long n'en est pas un (ambiance, morceau).
const MAX_SFX_SECONDS: f64 = 6.0;
/// Durée minimale d'une musique de fond (elle est reprise au début si elle
/// est plus courte que la vidéo).
const MIN_MUSIC_SECONDS: f64 = 45.0;
/// Les musiques trop longues sont souvent des mixes d'une heure.
const MAX_MUSIC_SECONDS: f64 = 900.0;
/// Le son tombe un peu avant le mot : on l'entend avec l'image qui apparaît.
const SFX_LEAD: f64 = 0.05;
/// Musique baissée sous la voix, par défaut (dB).
const MUSIC_GAIN: f32 = -16.0;
/// Recherches menées en même temps (Wikimedia limite vite les clients
/// pressés).
const PARALLEL_SEARCHES: usize = 2;
/// Résultats essayés pour une même demande, si les premiers ne se
/// téléchargent pas.
const MAX_CANDIDATES: usize = 4;

/// Modèle et effort du monteur selon le niveau de soin.
pub fn editor_model(quality: Quality) -> (&'static str, Effort) {
    match quality {
        Quality::Fast => ("claude-sonnet-5-5", Effort::Medium),
        Quality::Polished | Quality::Ultra => ("claude-opus-5-5", Effort::High),
    }
}

/// Demande le storyboard au monteur (deux essais si la réponse est
/// illisible). Ses notes vont dans `work/editor.notes.md`, le plan lu dans
/// `work/editor.json`.
pub async fn write_plan(
    provider: &Provider,
    work: &Path,
    input: &EditorInput<'_>,
    quality: Quality,
) -> Result<EditPlan> {
    let (model, effort) = editor_model(quality);
    let request = editor_request(input, model, effort);
    tokio::fs::create_dir_all(work).await?;
    let mut last_error = None;
    for _ in 0..2 {
        let reply = provider.complete(&request).await?;
        tokio::fs::write(work.join("editor.notes.md"), &reply.text).await?;
        match parse_plan(&reply.text, input.blocks.len(), input.has_video) {
            Ok(plan) => {
                tokio::fs::write(work.join("editor.json"), serde_json::to_vec_pretty(&plan)?)
                    .await?;
                return Ok(plan);
            }
            Err(e) => {
                tracing::warn!("réponse du monteur illisible : {e}");
                last_error = Some(e);
            }
        }
    }
    Err(last_error.map_or_else(|| anyhow::anyhow!("storyboard impossible"), Into::into))
}

/// Les fichiers trouvés pour un plan, déjà copiés dans la bibliothèque.
#[derive(Debug, Default)]
pub struct Gathered {
    /// Nouvelles entrées de la bibliothèque (avec leur licence).
    pub assets: Vec<Asset>,
    /// Plan de chaque bloc `broll` (numéro à partir de 1 → fichier).
    pub brolls: BTreeMap<usize, String>,
    pub music: Option<String>,
    /// Effet sonore trouvé pour chaque requête.
    pub sfx: BTreeMap<String, String>,
    /// Ce qui n'a pas été trouvé, en clair.
    pub missing: Vec<String>,
}

/// Licence utilisable sans condition gênante : libre, ou citer l'auteur.
pub fn usable(hit: &Hit) -> bool {
    let terms = hit.origin.license.terms();
    terms.known && !terms.non_commercial && !terms.no_derivatives && !terms.share_alike
}

/// Mots d'une requête qui comptent pour juger un titre (trois lettres ou
/// plus, sans les mots vides anglais).
fn query_words(query: &str) -> Vec<String> {
    const STOP: [&str; 8] = ["the", "and", "with", "for", "from", "into", "over", "of"];
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .collect()
}

/// Nombre de mots de la requête présents dans le titre du résultat.
pub fn relevance(hit: &Hit, query: &str) -> usize {
    let title = hit.origin.title.to_lowercase();
    query_words(query)
        .iter()
        .filter(|w| title.contains(w.as_str()))
        .count()
}

/// Les résultats retenus, du plus pertinent au moins pertinent (l'ordre des
/// banques départage), `MAX_CANDIDATES` au plus.
fn ranked<'a>(hits: impl Iterator<Item = &'a Hit>, query: &str) -> Vec<&'a Hit> {
    let mut scored: Vec<(usize, &Hit)> = hits.map(|h| (relevance(h, query), h)).collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    scored
        .into_iter()
        .map(|(_, h)| h)
        .take(MAX_CANDIDATES)
        .collect()
}

/// Plans réels candidats : licence utilisable, ni GIF ni SVG, au moins un
/// mot de la requête dans le titre (mieux vaut une animation qu'un plan hors
/// sujet), une vidéo assez longue de préférence.
pub fn broll_candidates<'a>(
    hits: &'a [Hit],
    media: Media,
    duration: f64,
    query: &str,
) -> Vec<&'a Hit> {
    let fits = |h: &&Hit| {
        usable(h)
            && relevance(h, query) > 0
            && match media {
                Media::Video => h.kind == AssetKind::Video,
                _ => h.kind == AssetKind::Image && h.ext != "gif",
            }
    };
    let long = |h: &&Hit| h.duration.is_none_or(|d| d >= duration);
    let mut out = ranked(hits.iter().filter(fits).filter(long), query);
    for h in ranked(hits.iter().filter(fits), query) {
        if out.len() < MAX_CANDIDATES && !out.iter().any(|o| std::ptr::eq(*o, h)) {
            out.push(h);
        }
    }
    out
}

/// Effets sonores candidats : courts, licence utilisable.
pub fn sfx_candidates<'a>(hits: &'a [Hit], query: &str) -> Vec<&'a Hit> {
    ranked(
        hits.iter().filter(|h| {
            h.kind == AssetKind::Audio
                && usable(h)
                && h.duration.is_none_or(|d| d <= MAX_SFX_SECONDS)
        }),
        query,
    )
}

/// Musiques candidates : assez longues (mais pas un mix d'une heure) ;
/// à défaut, d'au moins 20 s.
pub fn music_candidates<'a>(hits: &'a [Hit], query: &str) -> Vec<&'a Hit> {
    let fits = |h: &&Hit| h.kind == AssetKind::Audio && usable(h);
    let good = |h: &&Hit| {
        h.duration
            .is_some_and(|d| (MIN_MUSIC_SECONDS..=MAX_MUSIC_SECONDS).contains(&d))
    };
    let mut out = ranked(hits.iter().filter(fits).filter(good), query);
    if out.is_empty() {
        out = ranked(
            hits.iter()
                .filter(fits)
                .filter(|h| h.duration.is_none_or(|d| d >= 20.0)),
            query,
        );
    }
    out
}

/// Ce qu'on cherche pour un plan.
#[derive(Debug, Clone)]
enum Want {
    Broll {
        index: usize,
        query: String,
        media: Media,
        duration: f64,
    },
    Sfx {
        query: String,
    },
    Music {
        query: String,
    },
}

impl Want {
    fn label(&self) -> String {
        match self {
            Self::Broll { index, query, .. } => format!("plan du bloc {index} (« {query} »)"),
            Self::Sfx { query } => format!("effet sonore « {query} »"),
            Self::Music { query } => format!("musique « {query} »"),
        }
    }
}

/// Cherche et télécharge les plans et les sons du plan de montage, en
/// réutilisant les fichiers de la bibliothèque quand le monteur les cite.
/// `progress` reçoit une ligne par recherche.
pub async fn gather(
    plan: &EditPlan,
    project: &Project,
    look: &Look,
    library: &Path,
    downloads: &Path,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<Gathered> {
    use futures::StreamExt as _;
    let mut out = Gathered::default();
    let mut wants = Vec::new();
    for b in &plan.blocks {
        if b.plan_kind() == Some(PlanKind::Broll)
            && let Some(search) = &b.broll
        {
            let query = search.query.trim().to_owned();
            // Un fichier de la bibliothèque cité par son nom.
            let own = look.assets.iter().find(|a| {
                matches!(a.kind, AssetKind::Video | AssetKind::Image)
                    && (a.file == query || a.name.eq_ignore_ascii_case(&query))
            });
            if let Some(asset) = own {
                out.brolls.insert(b.index, asset.file.clone());
                continue;
            }
            let duration = b
                .index
                .checked_sub(1)
                .and_then(|i| project.blocks().get(i))
                .map_or(0.0, |block| block.range.duration().as_secs_f64());
            let media = if search.media.trim().eq_ignore_ascii_case("image") {
                Media::Image
            } else {
                Media::Video
            };
            wants.push(Want::Broll {
                index: b.index,
                query,
                media,
                duration,
            });
        }
        for s in &b.sfx {
            let query = s.query.trim().to_lowercase();
            let new = !wants
                .iter()
                .any(|w| matches!(w, Want::Sfx { query: q } if *q == query));
            if new {
                wants.push(Want::Sfx { query });
            }
        }
    }
    if let Some(music) = &plan.music {
        wants.push(Want::Music {
            query: music.query.trim().to_owned(),
        });
    }
    if wants.is_empty() {
        return Ok(out);
    }
    let client = enfait_assets::search::client()?;
    let client = &client;
    let found: Vec<(Want, Vec<Hit>)> = futures::stream::iter(wants)
        .map(|want| async move {
            progress(&format!("recherche : {}…", want.label()));
            let hits = find(client, &want).await;
            (want, hits)
        })
        .buffered(PARALLEL_SEARCHES)
        .collect()
        .await;
    tokio::fs::create_dir_all(downloads).await?;
    for (n, (want, hits)) in found.into_iter().enumerate() {
        progress(&format!("téléchargement : {}…", want.label()));
        let Some(mut asset) = fetch(
            client,
            &hits,
            library,
            &downloads.join(format!("magique-{n}")),
        )
        .await
        else {
            if !hits.is_empty() {
                tracing::warn!("{} : aucun téléchargement n'a abouti", want.label());
            }
            out.missing.push(want.label());
            continue;
        };
        asset.description = match &want {
            Want::Broll { index, query, .. } => {
                format!("Plan réel du bloc {index}, trouvé par le monteur (« {query} »).")
            }
            Want::Sfx { query } => format!("Effet sonore « {query} », posé par le monteur."),
            Want::Music { query } => {
                format!("Musique de fond « {query} », choisie par le monteur.")
            }
        };
        let file = asset.file.clone();
        out.assets.push(asset);
        match want {
            Want::Broll { index, .. } => {
                out.brolls.insert(index, file);
            }
            Want::Sfx { query } => {
                out.sfx.insert(query, file);
            }
            Want::Music { .. } => out.music = Some(file),
        }
    }
    Ok(out)
}

/// Télécharge le premier candidat qui veut bien venir et le copie dans la
/// bibliothèque. Une banque qui demande de ralentir (Wikimedia, 429) est
/// laissée de côté pour les candidats suivants.
async fn fetch(
    client: &reqwest::Client,
    hits: &[Hit],
    library: &Path,
    tmp: &Path,
) -> Option<Asset> {
    let mut refused: Vec<enfait_assets::search::Source> = Vec::new();
    for hit in hits {
        if refused.contains(&hit.source) {
            continue;
        }
        let tmp = tmp.with_extension(&hit.ext);
        let result = enfait_assets::search::download(client, &hit.file, &tmp).await;
        let added = match result {
            Ok(()) => enfait_assets::library::add_file(
                library,
                &tmp,
                &hit.origin.title,
                Some(&hit.origin.title),
                Some(hit.origin.clone()),
            ),
            Err(e) => {
                tracing::warn!("{} ({}) : {e}", hit.origin.title, hit.source.name());
                if e.to_string().contains("429") {
                    refused.push(hit.source);
                }
                Err(e.to_string())
            }
        };
        let _ = tokio::fs::remove_file(&tmp).await;
        match added {
            Ok(asset) => return Some(asset),
            Err(e) => tracing::debug!("candidat écarté : {e}"),
        }
    }
    None
}

/// Une recherche et ses meilleurs candidats (une vidéo introuvable est
/// cherchée en photo ; une musique introuvable, en musique d'ambiance).
async fn find(client: &reqwest::Client, want: &Want) -> Vec<Hit> {
    use enfait_assets::search::search;
    let owned = |v: Vec<&Hit>| v.into_iter().cloned().collect::<Vec<_>>();
    match want {
        Want::Broll {
            query,
            media,
            duration,
            ..
        } => {
            let results = search(client, query, *media).await;
            let found = owned(broll_candidates(&results.hits, *media, *duration, query));
            if !found.is_empty() || *media != Media::Video {
                return found;
            }
            let photos = search(client, query, Media::Image).await;
            owned(broll_candidates(&photos.hits, Media::Image, 0.0, query))
        }
        Want::Sfx { query } => {
            let results = search(client, query, Media::Audio).await;
            owned(sfx_candidates(&results.hits, query))
        }
        Want::Music { query } => {
            let results = search(client, query, Media::Audio).await;
            let found = owned(music_candidates(&results.hits, query));
            if !found.is_empty() {
                return found;
            }
            let fallback = "ambient background music";
            let results = search(client, fallback, Media::Audio).await;
            owned(music_candidates(&results.hits, fallback))
        }
    }
}

/// Le type de bloc d'un plan.
pub const fn block_kind(kind: PlanKind) -> BlockKind {
    match kind {
        PlanKind::Animation => BlockKind::Animation,
        PlanKind::Mixed => BlockKind::Mixed,
        PlanKind::Effect => BlockKind::Effect,
        PlanKind::Rush => BlockKind::Rush,
        PlanKind::Broll => BlockKind::Broll,
    }
}

/// Le type d'un bloc, pour le monteur.
pub const fn plan_kind(kind: BlockKind) -> PlanKind {
    match kind {
        BlockKind::Animation => PlanKind::Animation,
        BlockKind::Mixed => PlanKind::Mixed,
        BlockKind::Effect => PlanKind::Effect,
        BlockKind::Rush => PlanKind::Rush,
        BlockKind::Broll => PlanKind::Broll,
    }
}

/// Un bloc écrit par l'auteur : sa description n'est pas vide. Le monteur
/// ne le réécrit pas (« écrire le storyboard » complète, il ne remplace pas).
pub fn authored(block: &Block) -> bool {
    !block.description.trim().is_empty()
}

/// Les éditions qui appliquent le plan au projet, à passer d'un coup
/// (`Edit::Batch`). `keep_authored` : les blocs déjà décrits gardent type
/// et description. Les plans réels introuvables laissent le bloc en
/// animation, avec sa description.
pub fn edits(
    project: &Project,
    transcript: &Transcript,
    plan: &EditPlan,
    gathered: &Gathered,
    keep_authored: bool,
    max_transitions: usize,
) -> Vec<Edit> {
    let blocks = project.blocks();
    let mut out = Vec::new();
    let mut transitions = blocks
        .iter()
        .filter(|b| b.transition_after != Transition::Cut)
        .count();
    for p in &plan.blocks {
        let Some((i, block)) = p
            .index
            .checked_sub(1)
            .and_then(|i| blocks.get(i).map(|b| (i, b)))
        else {
            continue;
        };
        let keep = keep_authored && authored(block);
        if !keep {
            let wanted = p.plan_kind().unwrap_or(PlanKind::Animation);
            let broll = gathered.brolls.get(&p.index);
            let kind = match (wanted, broll) {
                (PlanKind::Broll, None) => BlockKind::Animation,
                (kind, _) => block_kind(kind),
            };
            if kind != block.kind {
                out.push(Edit::SetKind {
                    block: block.id,
                    kind,
                });
            }
            if let (BlockKind::Broll, Some(file)) = (kind, broll) {
                out.push(Edit::SetBroll {
                    block: block.id,
                    broll: Some(Broll {
                        file: file.clone(),
                        start: Flicks::ZERO,
                    }),
                });
            }
            let description = p.description.trim();
            if !description.is_empty() && description != block.description {
                out.push(Edit::SetDescription {
                    block: block.id,
                    text: description.to_owned(),
                });
            }
        }
        // Moments : sur un mot du bloc, sans écraser ceux de l'auteur.
        for m in &p.moments {
            let Some(at) = word_time(transcript, block, &m.word) else {
                continue;
            };
            let taken = block.moments.iter().any(|x| x.at == at)
                || out.iter().any(|e| {
                    matches!(e, Edit::AddMoment { block: b, moment } if *b == block.id && moment.at == at)
                });
            if !taken {
                out.push(Edit::AddMoment {
                    block: block.id,
                    moment: Moment {
                        at,
                        word: word_text(transcript, at),
                        note: m.note.trim().to_owned(),
                    },
                });
            }
        }
        // Transition : pas après le dernier bloc, pas au-delà du nombre
        // permis, pas à la place d'une transition choisie par l'auteur.
        let last = i.saturating_add(1) >= blocks.len();
        if let Some(t) = &p.transition
            && !last
            && block.transition_after == Transition::Cut
        {
            let transition = match t.kind.trim().to_lowercase().as_str() {
                "auto" => Transition::Auto,
                "decrite" | "décrite" | "described" if !t.description.trim().is_empty() => {
                    Transition::Described(t.description.trim().to_owned())
                }
                "decrite" | "décrite" | "described" => Transition::Auto,
                _ => Transition::Cut,
            };
            if transition != Transition::Cut && transitions < max_transitions {
                transitions = transitions.saturating_add(1);
                out.push(Edit::SetTransition {
                    block: block.id,
                    transition,
                });
            }
        }
    }
    out
}

/// Pose la musique et les effets sonores trouvés. Les sons déjà posés par
/// l'auteur restent ; une musique n'est ajoutée que s'il n'y en a pas.
/// Renvoie le nombre de sons posés.
pub fn place_sounds(
    project: &Project,
    transcript: &Transcript,
    plan: &EditPlan,
    gathered: &Gathered,
    finish: &mut Finish,
) -> usize {
    let blocks = project.blocks();
    let mut placed = 0_usize;
    if let (Some(file), Some(first), Some(last)) = (&gathered.music, blocks.first(), blocks.last())
        && finish.music.is_empty()
    {
        let id = finish.add_music(file.clone(), first.id, last.id);
        let gain = plan
            .music
            .as_ref()
            .and_then(|m| m.volume_db)
            .unwrap_or(MUSIC_GAIN)
            .clamp(-30.0, -6.0);
        if let Some(m) = finish.music_mut(id) {
            m.gain_db = gain;
        }
        placed = placed.saturating_add(1);
    }
    for p in &plan.blocks {
        let Some(block) = p.index.checked_sub(1).and_then(|i| blocks.get(i)) else {
            continue;
        };
        for s in &p.sfx {
            let Some(file) = gathered.sfx.get(&s.query.trim().to_lowercase()) else {
                continue;
            };
            let start = block.range.start().as_secs_f64();
            let offset = word_time(transcript, block, &s.word)
                .map_or(0.0, |t| (t.as_secs_f64() - start - SFX_LEAD).max(0.0));
            let duplicate = finish.sfx.iter().any(|x| {
                x.block == block.id && x.file == *file && (x.offset - offset).abs() < 0.05
            });
            if !duplicate {
                finish.add_sfx(file.clone(), block.id, offset);
                placed = placed.saturating_add(1);
            }
        }
    }
    placed
}

/// Mot ramené à l'essentiel pour comparer (minuscules, sans ponctuation).
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Instant du mot `word` dans le bloc (le premier mot d'une expression ;
/// à défaut, un mot qui le contient).
pub fn word_time(transcript: &Transcript, block: &Block, word: &str) -> Option<Flicks> {
    let pieces: Vec<String> = word
        .split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    let first = pieces.first()?;
    let words: Vec<_> = transcript.words_in(block.range).collect();
    let norm: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
    // L'expression entière, puis son premier mot, puis un mot qui le
    // contient (« l'Europe » pour « Europe »).
    let whole = (0..norm.len()).find(|&i| {
        pieces
            .iter()
            .enumerate()
            .all(|(k, p)| norm.get(i.saturating_add(k)) == Some(p))
    });
    let index = whole
        .or_else(|| norm.iter().position(|w| w == first))
        .or_else(|| norm.iter().position(|w| w.contains(first.as_str())))?;
    words.get(index).map(|w| w.range.start())
}

/// Texte du mot qui commence à `at`.
fn word_text(transcript: &Transcript, at: Flicks) -> String {
    transcript
        .words
        .iter()
        .find(|w| w.range.start() == at)
        .map(|w| {
            w.text
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_owned()
        })
        .unwrap_or_default()
}

/// Les blocs vus par le monteur.
pub fn editor_blocks(
    project: &Project,
    transcript: &Transcript,
    keep_authored: bool,
) -> Vec<enfait_ai::editor::EditorBlock> {
    project
        .blocks()
        .iter()
        .map(|b| enfait_ai::editor::EditorBlock {
            start: b.range.start().as_secs_f64(),
            end: b.range.end().as_secs_f64(),
            text: transcript.text_in(b.range),
            authored: (keep_authored && authored(b))
                .then(|| (plan_kind(b.kind), b.description.trim().to_owned())),
        })
        .collect()
}

/// Transitions travaillées permises : environ une pour quatre blocs, quatre
/// au plus.
pub const fn max_transitions(blocks: usize) -> usize {
    if blocks < 2 {
        0
    } else {
        let n = blocks / 4;
        if n < 1 {
            1
        } else if n > 4 {
            4
        } else {
            n
        }
    }
}

/// La bibliothèque pour le monteur : images, SVG et vidéos, une ligne par
/// fichier (les polices et les sons ne font pas un plan).
pub fn library_lines(look: &Look) -> Option<String> {
    let lines: Vec<String> = look
        .assets
        .iter()
        .filter(|a| matches!(a.kind, AssetKind::Image | AssetKind::Svg | AssetKind::Video))
        .map(|a| {
            let mut line = format!("- `{}` ({}) : {}", a.file, a.kind.label(), a.name);
            if !a.description.trim().is_empty() {
                line.push_str(" — ");
                line.push_str(a.description.trim());
            }
            line
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Bilan en une phrase pour l'interface.
pub fn summary(plan: &EditPlan, gathered: &Gathered, edits: usize, sounds: usize) -> String {
    let brolls = gathered.brolls.len();
    let mut out = format!(
        "Storyboard écrit : {} bloc{} décrit{}",
        plan.blocks.len(),
        if plan.blocks.len() > 1 { "s" } else { "" },
        if plan.blocks.len() > 1 { "s" } else { "" },
    );
    if brolls > 0 {
        out.push_str(&format!(
            ", {brolls} plan{} réel{}",
            if brolls > 1 { "s" } else { "" },
            if brolls > 1 { "s" } else { "" }
        ));
    }
    if sounds > 0 {
        out.push_str(&format!(
            ", {sounds} son{} posé{}",
            if sounds > 1 { "s" } else { "" },
            if sounds > 1 { "s" } else { "" }
        ));
    }
    if edits == 0 {
        out.push_str(" (rien à changer)");
    }
    out.push('.');
    if !gathered.missing.is_empty() {
        out.push_str(&format!(" Introuvable : {}.", gathered.missing.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_ai::editor::{
        BlockPlan, MomentPlan, MusicPlan, SearchPlan, SfxPlan, TransitionPlan,
    };
    use enfait_core::{License, Origin, ProjectSettings, TimeRange, Word};

    fn secs(t: f64) -> Flicks {
        #[allow(clippy::cast_possible_truncation)]
        Flicks::from_millis((t * 1000.0).round() as i64).unwrap()
    }

    /// Trois blocs de 3 s, trois mots chacun.
    fn project() -> (Project, Transcript) {
        let texts = [
            ["Bonjour", "à", "tous."],
            ["La", "gare", "fume."],
            ["Le", "train", "part."],
        ];
        let mut words = Vec::new();
        let mut blocks = Vec::new();
        for (b, text) in texts.iter().enumerate() {
            let t0 = 3.0 * b as f64;
            for (k, w) in text.iter().enumerate() {
                let t = t0 + 0.2 + 0.8 * k as f64;
                words.push(Word {
                    text: (*w).to_owned(),
                    range: TimeRange::new(secs(t), secs(t + 0.5)).unwrap(),
                    confidence: 1.0,
                });
            }
            blocks.push(Block::new(
                TimeRange::new(secs(t0), secs(t0 + 3.0)).unwrap(),
                BlockKind::Animation,
            ));
        }
        let project = Project::with_blocks("t", ProjectSettings::default(), blocks).unwrap();
        let transcript = Transcript {
            language: "fr".into(),
            words,
            media_duration: secs(9.0),
        };
        (project, transcript)
    }

    fn plan() -> EditPlan {
        EditPlan {
            blocks: vec![
                BlockPlan {
                    index: 1,
                    kind: "rush".into(),
                    description: "face caméra".into(),
                    transition: Some(TransitionPlan {
                        kind: "decrite".into(),
                        description: "balayage".into(),
                    }),
                    ..Default::default()
                },
                BlockPlan {
                    index: 2,
                    kind: "broll".into(),
                    description: "une gare dans la brume".into(),
                    broll: Some(SearchPlan {
                        query: "train station fog".into(),
                        media: "video".into(),
                    }),
                    moments: vec![MomentPlan {
                        word: "gare".into(),
                        note: "la fumée envahit".into(),
                    }],
                    sfx: vec![SfxPlan {
                        word: "fume".into(),
                        query: "Steam hiss".into(),
                    }],
                    transition: Some(TransitionPlan {
                        kind: "auto".into(),
                        description: String::new(),
                    }),
                },
                BlockPlan {
                    index: 3,
                    kind: "broll".into(),
                    description: "le train démarre".into(),
                    broll: Some(SearchPlan {
                        query: "train departure".into(),
                        media: "video".into(),
                    }),
                    transition: Some(TransitionPlan {
                        kind: "auto".into(),
                        description: String::new(),
                    }),
                    ..Default::default()
                },
            ],
            music: Some(MusicPlan {
                query: "calm piano".into(),
                volume_db: Some(-40.0),
            }),
        }
    }

    fn gathered() -> Gathered {
        let mut g = Gathered::default();
        g.brolls.insert(2, "gare.mp4".into());
        g.sfx.insert("steam hiss".into(), "vapeur.wav".into());
        g.music = Some("piano.mp3".into());
        g.missing
            .push("plan du bloc 3 (« train departure »)".into());
        g
    }

    #[test]
    fn plan_becomes_one_undoable_batch() {
        let (mut project, transcript) = project();
        let before = project.clone();
        let edits = edits(&project, &transcript, &plan(), &gathered(), true, 1);
        let mut history = enfait_core::History::new();
        history.apply(&mut project, Edit::Batch { edits }).unwrap();
        let b = project.blocks();
        assert_eq!(b[0].kind, BlockKind::Rush);
        assert_eq!(
            b[0].transition_after,
            Transition::Described("balayage".into())
        );
        assert_eq!(b[1].kind, BlockKind::Broll);
        assert_eq!(b[1].broll.as_ref().unwrap().file, "gare.mp4");
        assert_eq!(b[1].moments.len(), 1);
        assert_eq!(b[1].moments[0].word, "gare");
        assert_eq!(b[1].moments[0].at, secs(4.0));
        assert_eq!(
            b[1].transition_after,
            Transition::Cut,
            "une seule transition permise"
        );
        assert_eq!(
            b[2].kind,
            BlockKind::Animation,
            "plan introuvable : animation"
        );
        assert_eq!(b[2].description, "le train démarre");
        assert_eq!(
            b[2].transition_after,
            Transition::Cut,
            "rien après le dernier bloc"
        );
        history.undo(&mut project).unwrap();
        assert_eq!(project, before);
    }

    #[test]
    fn authored_blocks_are_kept() {
        let (mut project, transcript) = project();
        let id = project.blocks()[1].id;
        Edit::SetDescription {
            block: id,
            text: "ma gare à moi".into(),
        }
        .apply(&mut project)
        .unwrap();
        let edits = edits(&project, &transcript, &plan(), &gathered(), true, 3);
        assert!(!edits.iter().any(|e| matches!(
            e,
            Edit::SetDescription { block, .. } | Edit::SetKind { block, .. } if *block == id
        )));
        // Ses moments et sa transition viennent quand même.
        assert!(
            edits
                .iter()
                .any(|e| matches!(e, Edit::AddMoment { block, .. } if *block == id))
        );
        // En mode magique, tout est réécrit.
        let all = super::edits(&project, &transcript, &plan(), &gathered(), false, 3);
        assert!(
            all.iter()
                .any(|e| matches!(e, Edit::SetDescription { block, .. } if *block == id))
        );
    }

    #[test]
    fn sounds_are_placed_on_their_words() {
        let (project, transcript) = project();
        let mut finish = Finish::default();
        let n = place_sounds(&project, &transcript, &plan(), &gathered(), &mut finish);
        assert_eq!(n, 2);
        assert_eq!(finish.music.len(), 1);
        assert_eq!(finish.music[0].gain_db, -30.0, "volume borné");
        assert_eq!(finish.sfx.len(), 1);
        let sfx = &finish.sfx[0];
        assert_eq!(sfx.block, project.blocks()[1].id);
        // « fume. » commence à 1,8 s dans le bloc, le son un peu avant.
        assert!((sfx.offset - 1.75).abs() < 1e-9, "{}", sfx.offset);
        // Une seconde fois : pas de doublon, pas de deuxième musique.
        assert_eq!(
            place_sounds(&project, &transcript, &plan(), &gathered(), &mut finish),
            0
        );
    }

    #[test]
    fn words_are_found_loosely() {
        let (project, transcript) = project();
        let b = &project.blocks()[0];
        assert_eq!(word_time(&transcript, b, "TOUS"), Some(secs(1.8)));
        assert_eq!(word_time(&transcript, b, "à tous"), Some(secs(1.0)));
        assert_eq!(word_time(&transcript, b, "gare"), None, "pas dans ce bloc");
    }

    fn hit(kind: AssetKind, license: &str, duration: Option<f64>, ext: &str, title: &str) -> Hit {
        Hit {
            source: enfait_assets::search::Source::Commons,
            kind,
            thumb: String::new(),
            file: "https://example.org/f".into(),
            ext: ext.into(),
            width: 1920,
            height: 1080,
            duration,
            origin: Origin {
                source: "Wikimedia Commons".into(),
                page: String::new(),
                title: title.into(),
                author: String::new(),
                license: License::new(license, None),
                changes: Vec::new(),
            },
        }
    }

    #[test]
    fn candidates_respect_licenses_durations_and_topic() {
        let hits = vec![
            hit(
                AssetKind::Video,
                "CC BY-NC 4.0",
                Some(20.0),
                "mp4",
                "Train station",
            ),
            hit(
                AssetKind::Video,
                "CC BY-SA 4.0",
                Some(20.0),
                "mp4",
                "Train station",
            ),
            hit(
                AssetKind::Video,
                "CC0",
                Some(1.0),
                "mp4",
                "Old train station",
            ),
            hit(
                AssetKind::Video,
                "CC BY 4.0",
                Some(12.0),
                "webm",
                "Station at night",
            ),
            hit(AssetKind::Video, "CC0", Some(30.0), "mp4", "Cat sleeping"),
        ];
        let q = "train station";
        let c = broll_candidates(&hits, Media::Video, 5.0, q);
        assert_eq!(c.len(), 2, "licences gênantes et hors sujet écartés");
        assert_eq!(c[0].ext, "webm", "assez longue d'abord");
        let c = broll_candidates(&hits, Media::Video, 30.0, q);
        assert_eq!(c[0].origin.title, "Old train station", "plus pertinente");
        let maps = vec![
            hit(
                AssetKind::Image,
                "Public domain",
                None,
                "jpg",
                "Africa, with all its States",
            ),
            hit(AssetKind::Image, "CC0", None, "gif", "Map of Europe"),
            hit(
                AssetKind::Image,
                "inconnue",
                None,
                "jpg",
                "Map of Europe, 1750",
            ),
            hit(
                AssetKind::Image,
                "Public domain",
                None,
                "jpg",
                "Carte de l'Europe",
            ),
        ];
        let c = broll_candidates(&maps, Media::Image, 0.0, "antique map Europe 18th century");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].origin.title, "Carte de l'Europe");
        let sounds = vec![
            hit(AssetKind::Audio, "CC0", Some(30.0), "mp3", "Steam"),
            hit(AssetKind::Audio, "CC0", Some(0.8), "wav", "Click"),
            hit(AssetKind::Audio, "CC0", Some(1.5), "ogg", "Steam hiss"),
            hit(AssetKind::Audio, "CC BY 4.0", Some(120.0), "mp3", "Piano"),
        ];
        let c = sfx_candidates(&sounds, "steam hiss");
        assert_eq!(c[0].ext, "ogg", "le titre qui correspond");
        assert_eq!(c.len(), 2, "trop long : pas un effet");
        assert_eq!(
            music_candidates(&sounds, "calm piano")[0].duration,
            Some(120.0)
        );
    }

    #[test]
    fn transitions_grow_with_the_video() {
        assert_eq!(max_transitions(1), 0);
        assert_eq!(max_transitions(3), 1);
        assert_eq!(max_transitions(9), 2);
        assert_eq!(max_transitions(40), 4);
    }

    #[test]
    fn summary_says_what_is_missing() {
        let text = summary(&plan(), &gathered(), 5, 2);
        assert!(text.starts_with("Storyboard écrit : 3 blocs décrits, 1 plan réel, 2 sons posés."));
        assert!(text.contains("Introuvable : plan du bloc 3"));
    }
}
