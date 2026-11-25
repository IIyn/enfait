//! "Write the storyboard" and magic mode: the editor pass, the search for
//! the real shots and sounds it asks for, then applying them to the project
//! (a single undo step) and to the finish.
//!
//! Shared by the interface, the CLI and the MCP server; the code decides
//! everything that is not a matter of taste: a shot or a sound is only
//! kept if its licence allows using it without an awkward condition (no
//! non-commercial only, no ban on changes, no share-alike).

use anyhow::Result;
use enfait_ai::editor::{
    EditPlan, EditorInput, EditorPart, PlanKind, editor_request, parse_plan, part_request,
};
use enfait_ai::{Effort, Provider, Quality};
use enfait_assets::search::{Hit, Media};
use enfait_core::{
    Asset, AssetKind, Block, BlockKind, Broll, Edit, Finish, Flicks, Look, Moment, Project,
    Transcript, Transition,
};
use std::collections::BTreeMap;
use std::path::Path;

/// A longer sound effect is not one (ambience, track).
const MAX_SFX_SECONDS: f64 = 6.0;
/// Minimum duration of a background music (it loops if it is shorter than
/// the video).
const MIN_MUSIC_SECONDS: f64 = 45.0;
/// Music that is too long is often an hour-long mix.
const MAX_MUSIC_SECONDS: f64 = 900.0;
/// The sound falls a little before the word: heard with the picture appearing.
const SFX_LEAD: f64 = 0.05;
/// Music dipped under the voice, by default (dB).
const MUSIC_GAIN: f32 = -16.0;
/// Searches run at the same time (Wikimedia quickly limits hasty
/// clients).
const PARALLEL_SEARCHES: usize = 2;
/// Results tried for one request, if the first ones do not download.
const MAX_CANDIDATES: usize = 4;

/// Model and effort of the editor depending on the quality level.
pub fn editor_model(quality: Quality) -> (&'static str, Effort) {
    match quality {
        Quality::Light | Quality::Fast => ("claude-sonnet-5-5", Effort::Medium),
        Quality::Polished | Quality::Ultra => ("claude-opus-5-5", Effort::High),
    }
}

/// Kinds of the last blocks recalled to the next part (rhythm).
const RECENT_KINDS: usize = 4;

/// `count` blocks in parts of `size` at most, of even lengths.
pub fn parts(count: usize, size: usize) -> Vec<std::ops::Range<usize>> {
    let size = size.max(1);
    let n = count.div_ceil(size).max(1);
    let (base, extra) = (count / n, count % n);
    let mut out = Vec::with_capacity(n);
    let mut start = 0_usize;
    for i in 0..n {
        let len = base.saturating_add(usize::from(i < extra));
        out.push(start..start.saturating_add(len));
        start = start.saturating_add(len);
    }
    out
}

/// Asks the editor for the storyboard (two tries if an answer is
/// unreadable): in a single request, or part by part past `per_request`
/// blocks (`None`: always in a single request; a long video in a single
/// request weighs a lot: context, attention). `progress(part, parts)`
/// before each part. Its notes go to `work/editor.notes.md`, the parsed plan to
/// `work/editor.json`.
pub async fn write_plan(
    provider: &Provider,
    work: &Path,
    input: &EditorInput<'_>,
    quality: Quality,
    per_request: Option<usize>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<EditPlan> {
    let (model, effort) = editor_model(quality);
    tokio::fs::create_dir_all(work).await?;
    let ranges = parts(
        input.blocks.len(),
        per_request.unwrap_or(input.blocks.len()),
    );
    let plan = if ranges.len() <= 1 {
        let request = editor_request(input, model, effort);
        let (plan, notes) = ask(provider, &request, input).await??;
        tokio::fs::write(work.join("editor.notes.md"), notes).await?;
        plan
    } else {
        write_parts(provider, work, input, &ranges, model, effort, progress).await?
    };
    tokio::fs::write(work.join("editor.json"), serde_json::to_vec_pretty(&plan)?).await?;
    Ok(plan)
}

/// One request, two tries: the plan and the notes of the editor. Outer
/// error: the provider failed; inner error: the answer stayed unreadable
/// or was truncated (a part is then skipped).
async fn ask(
    provider: &Provider,
    request: &enfait_ai::Request,
    input: &EditorInput<'_>,
) -> Result<Result<(EditPlan, String), enfait_ai::AiError>> {
    let mut last_error = enfait_ai::AiError::Unexpected("no storyboard".into());
    for _ in 0..2 {
        let reply = match provider.complete(request).await {
            Ok(reply) => reply,
            // The same request would stop at the same place.
            Err(e @ enfait_ai::AiError::Truncated) => return Ok(Err(e)),
            Err(e) => return Err(e.into()),
        };
        match parse_plan(&reply.text, input.blocks.len(), input.has_video) {
            Ok(plan) => return Ok(Ok((plan, reply.text))),
            Err(e) => {
                tracing::warn!("unreadable editor answer: {e}");
                last_error = e;
            }
        }
    }
    Ok(Err(last_error))
}

/// The parts one after the other: each one sees the end of the previous
/// one (last block written, its transition, the kinds chosen) and the
/// crafted transitions left. A part that stays unreadable is skipped (its
/// blocks stay as they are); a provider error stops everything.
async fn write_parts(
    provider: &Provider,
    work: &Path,
    input: &EditorInput<'_>,
    ranges: &[std::ops::Range<usize>],
    model: &str,
    effort: Effort,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<EditPlan> {
    let authored_transitions = input
        .blocks
        .iter()
        .filter(|b| b.transition.is_some())
        .count();
    let mut plan = EditPlan::default();
    let mut notes = String::new();
    let mut last_error = None;
    for (n, range) in ranges.iter().enumerate() {
        progress(n.saturating_add(1), ranges.len());
        let chosen = plan
            .blocks
            .iter()
            .filter(|b| {
                b.transition
                    .as_ref()
                    .is_some_and(|t| enfait_ai::editor::is_crafted(&t.kind))
            })
            .count();
        let before = range
            .start
            .checked_sub(1)
            .and_then(|i| plan.blocks.iter().find(|b| b.index == i.saturating_add(1)));
        let recent_kinds = plan
            .blocks
            .iter()
            .rev()
            .take(RECENT_KINDS)
            .rev()
            .map(|b| b.kind.as_str())
            .collect();
        let part = EditorPart {
            range: range.clone(),
            before,
            recent_kinds,
            transitions_left: input
                .max_transitions
                .saturating_sub(authored_transitions.saturating_add(chosen)),
        };
        let request = part_request(input, &part, model, effort);
        match ask(provider, &request, input).await? {
            Ok((part_plan, text)) => {
                let _ = std::fmt::Write::write_fmt(
                    &mut notes,
                    format_args!(
                        "# Blocks {} to {}\n\n{text}\n\n",
                        range.start.saturating_add(1),
                        range.end
                    ),
                );
                plan.blocks.extend(
                    part_plan
                        .blocks
                        .into_iter()
                        .filter(|b| range.contains(&b.index.saturating_sub(1))),
                );
                if range.start == 0 {
                    plan.music = part_plan.music;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "blocks {} to {} skipped: {e}",
                    range.start.saturating_add(1),
                    range.end
                );
                last_error = Some(e);
            }
        }
    }
    tokio::fs::write(work.join("editor.notes.md"), notes).await?;
    if plan.blocks.is_empty()
        && let Some(e) = last_error
    {
        return Err(e.into());
    }
    Ok(plan)
}

/// The files found for a plan, already copied into the library.
#[derive(Debug, Default)]
pub struct Gathered {
    /// New library entries (with their licence).
    pub assets: Vec<Asset>,
    /// Shot of each `broll` block (number from 1 → file).
    pub brolls: BTreeMap<usize, String>,
    pub music: Option<String>,
    /// Sound effect found for each query.
    pub sfx: BTreeMap<String, String>,
    /// What was not found, in plain words.
    pub missing: Vec<String>,
}

/// Licence usable without an awkward condition: free, or credit the author.
pub fn usable(hit: &Hit) -> bool {
    let terms = hit.origin.license.terms();
    terms.known && !terms.non_commercial && !terms.no_derivatives && !terms.share_alike
}

/// Words of a query that count to judge a title (three letters or more,
/// without English stop words).
fn query_words(query: &str) -> Vec<String> {
    const STOP: [&str; 8] = ["the", "and", "with", "for", "from", "into", "over", "of"];
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .collect()
}

/// Number of words of the query present in the title of the result.
pub fn relevance(hit: &Hit, query: &str) -> usize {
    let title = hit.origin.title.to_lowercase();
    query_words(query)
        .iter()
        .filter(|w| title.contains(w.as_str()))
        .count()
}

/// The kept results, from the most to the least relevant (the order of the
/// banks breaks ties), `MAX_CANDIDATES` at most.
fn ranked<'a>(hits: impl Iterator<Item = &'a Hit>, query: &str) -> Vec<&'a Hit> {
    let mut scored: Vec<(usize, &Hit)> = hits.map(|h| (relevance(h, query), h)).collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    scored
        .into_iter()
        .map(|(_, h)| h)
        .take(MAX_CANDIDATES)
        .collect()
}

/// Candidate real shots: usable licence, neither GIF nor SVG, at least one
/// word of the query in the title (better an animation than an off-topic
/// shot), a long enough video preferably.
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

/// Candidate sound effects: short, usable licence.
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

/// Candidate music: long enough (but not an hour-long mix); failing that,
/// at least 20 s.
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

/// What is searched for a plan.
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
            Self::Broll { index, query, .. } => format!("shot of block {index} (“{query}”)"),
            Self::Sfx { query } => format!("sound effect “{query}”"),
            Self::Music { query } => format!("music “{query}”"),
        }
    }
}

/// Searches and downloads the shots and sounds of the edit plan, reusing
/// the library files when the editor mentions them. `progress` gets one
/// line per search.
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
            // A library file mentioned by name.
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
        progress(&format!("downloading: {}…", want.label()));
        let Some(mut asset) = fetch(
            client,
            &hits,
            library,
            &downloads.join(format!("magique-{n}")),
        )
        .await
        else {
            if !hits.is_empty() {
                tracing::warn!("{}: no download succeeded", want.label());
            }
            out.missing.push(want.label());
            continue;
        };
        asset.description = match &want {
            Want::Broll { index, query, .. } => {
                format!("Real shot of block {index}, found by the editor (“{query}”).")
            }
            Want::Sfx { query } => format!("Sound effect “{query}”, placed by the editor."),
            Want::Music { query } => {
                format!("Background music “{query}”, chosen by the editor.")
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

/// Downloads the first candidate that comes and copies it into the
/// library. A bank asking to slow down (Wikimedia, 429) is left aside for
/// the next candidates.
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
            Err(e) => tracing::debug!("candidate discarded: {e}"),
        }
    }
    None
}

/// A search and its best candidates (a video not found is searched as a
/// photo; a music not found, as ambient music).
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

/// The block kind of a plan.
pub const fn block_kind(kind: PlanKind) -> BlockKind {
    match kind {
        PlanKind::Animation => BlockKind::Animation,
        PlanKind::Mixed => BlockKind::Mixed,
        PlanKind::Effect => BlockKind::Effect,
        PlanKind::Rush => BlockKind::Rush,
        PlanKind::Broll => BlockKind::Broll,
    }
}

/// The kind of a block, for the editor.
pub const fn plan_kind(kind: BlockKind) -> PlanKind {
    match kind {
        BlockKind::Animation => PlanKind::Animation,
        BlockKind::Mixed => PlanKind::Mixed,
        BlockKind::Effect => PlanKind::Effect,
        BlockKind::Rush => PlanKind::Rush,
        BlockKind::Broll => PlanKind::Broll,
    }
}

/// A block written by the author: its description is not empty. The
/// editor does not rewrite it ("Write the storyboard" completes, it does not replace).
pub fn authored(block: &Block) -> bool {
    !block.description.trim().is_empty()
}

/// The edits applying the plan to the project, to pass at once
/// (`Edit::Batch`). `keep_authored`: blocks already described keep kind
/// and description. Real shots not found leave the block as an
/// animation, with its description.
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
        // Moments: on a word of the block, without overwriting the author's.
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
        // Transition: not after the last block, not beyond the allowed number,
        // not instead of a transition chosen by the author.
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

/// Places the music and sound effects found. Sounds already placed by the
/// author stay; a music is only added if there is none.
/// Returns the number of placed sounds.
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

/// Word reduced to the essentials to compare (lowercase, no punctuation).
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Time of word `word` in the block (the first word of a phrase; failing
/// that, a word containing it).
pub fn word_time(transcript: &Transcript, block: &Block, word: &str) -> Option<Flicks> {
    let pieces: Vec<String> = word
        .split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    let first = pieces.first()?;
    let words: Vec<_> = transcript.words_in(block.range).collect();
    let norm: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
    // The whole phrase, then its first word, then a word containing it
    // ("l'Europe" for "Europe").
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

/// Text of the word starting at `at`.
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

/// The blocks as the editor sees them.
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
            transition: match &b.transition_after {
                Transition::Cut => None,
                Transition::Auto => Some("auto".to_owned()),
                Transition::Described(text) => Some(text.trim().to_owned()),
            },
        })
        .collect()
}

/// Crafted transitions allowed: about one per four blocks, four at most.
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

/// The library for the editor: images, SVG and videos, one line per file
/// (fonts and sounds do not make a shot).
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

/// Summary in a sentence (CLI and MCP; the app builds its own, translated).
pub fn summary(plan: &EditPlan, gathered: &Gathered, edits: usize, sounds: usize) -> String {
    let brolls = gathered.brolls.len();
    let mut out = format!(
        "Storyboard written: {} block(s) described",
        plan.blocks.len()
    );
    if brolls > 0 {
        out.push_str(&format!(", {brolls} real shot(s)"));
    }
    if sounds > 0 {
        out.push_str(&format!(", {sounds} sound(s) placed"));
    }
    if edits == 0 {
        out.push_str(" (nothing to change)");
    }
    out.push('.');
    if !gathered.missing.is_empty() {
        out.push_str(&format!(" Not found: {}.", gathered.missing.join(", ")));
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

    /// Three 3 s blocks, three words each.
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
                        kind: "described".into(),
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
        g.missing.push("shot of block 3 (“train departure”)".into());
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
            "a single transition allowed"
        );
        assert_eq!(b[2].kind, BlockKind::Animation, "shot not found: animation");
        assert_eq!(b[2].description, "le train démarre");
        assert_eq!(
            b[2].transition_after,
            Transition::Cut,
            "nothing after the last block"
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
        // Its moments and its transition still come.
        assert!(
            edits
                .iter()
                .any(|e| matches!(e, Edit::AddMoment { block, .. } if *block == id))
        );
        // In magic mode, everything is rewritten.
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
        assert_eq!(finish.music[0].gain_db, -30.0, "volume bounded");
        assert_eq!(finish.sfx.len(), 1);
        let sfx = &finish.sfx[0];
        assert_eq!(sfx.block, project.blocks()[1].id);
        // "fume." starts at 1.8 s in the block, the sound a little before.
        assert!((sfx.offset - 1.75).abs() < 1e-9, "{}", sfx.offset);
        // A second time: no duplicate, no second music.
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
        assert_eq!(word_time(&transcript, b, "gare"), None, "not in this block");
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
        assert_eq!(c.len(), 2, "awkward licences and off-topic discarded");
        assert_eq!(c[0].ext, "webm", "assez longue d'abord");
        let c = broll_candidates(&hits, Media::Video, 30.0, q);
        assert_eq!(c[0].origin.title, "Old train station", "most relevant");
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
        assert_eq!(c[0].ext, "ogg", "the matching title");
        assert_eq!(c.len(), 2, "too long: not an effect");
        assert_eq!(
            music_candidates(&sounds, "calm piano")[0].duration,
            Some(120.0)
        );
    }

    #[test]
    fn long_videos_are_written_in_parts() {
        assert_eq!(parts(5, 8), vec![0..5]);
        assert_eq!(parts(0, 8), vec![0..0]);
        assert_eq!(parts(17, 8), vec![0..6, 6..12, 12..17]);
        assert_eq!(parts(16, 8), vec![0..8, 8..16]);
    }

    /// Three parts with canned answers: each request holds only its blocks
    /// and neighbours, the transition of the previous part is passed on, an
    /// unreadable part is skipped.
    #[tokio::test]
    async fn parts_pass_their_context_on() {
        use enfait_ai::editor::EditorBlock;
        use std::sync::{Arc, Mutex};
        let blocks: Vec<EditorBlock> = (1..=6)
            .map(|i| EditorBlock {
                start: 0.0,
                end: 1.0,
                text: format!("Phrase {i}."),
                authored: None,
                transition: None,
            })
            .collect();
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
            max_transitions: 1,
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let provider = Provider::Canned(Arc::new(move |request: &enfait_ai::Request| {
            let enfait_ai::Part::Text(user) = &request.user[0] else {
                return Ok(String::new());
            };
            log.lock().unwrap().push(user.clone());
            Ok(if user.contains("blocks 1 to 2") {
                r#"```json
{"blocks":[{"index":1,"kind":"animation","description":"un"},
 {"index":2,"kind":"animation","description":"deux","transition":{"type":"described","description":"zoom"}},
 {"index":5,"kind":"animation","description":"hors partie"}],
 "music":{"query":"calm piano"}}
```"#
            } else if user.contains("blocks 3 to 4") {
                "pas de JSON"
            } else {
                r#"{"blocks":[{"index":5,"kind":"animation","description":"cinq"}]}"#
            }
            .to_owned())
        }));
        let work = tempfile::tempdir().unwrap();
        let ranges = parts(blocks.len(), 2);
        let steps = Mutex::new(Vec::new());
        let plan = write_parts(
            &provider,
            work.path(),
            &input,
            &ranges,
            "m",
            Effort::Medium,
            &|part, count| steps.lock().unwrap().push((part, count)),
        )
        .await
        .unwrap();
        assert_eq!(*steps.lock().unwrap(), vec![(1, 3), (2, 3), (3, 3)]);
        let indexes: Vec<usize> = plan.blocks.iter().map(|b| b.index).collect();
        assert_eq!(indexes, vec![1, 2, 5], "outside its part, unreadable: dropped");
        assert_eq!(plan.blocks[2].description, "cinq");
        assert_eq!(plan.music.unwrap().query, "calm piano");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1 + 2 + 1, "the unreadable part is asked twice");
        assert!(!seen[0].contains("Phrase 4."));
        assert!(seen[1].contains("transition into this part: “zoom”"));
        assert!(seen[1].contains("Crafted transitions left: 0"));
        assert!(seen[3].contains("Phrase 4.") && !seen[3].contains("transition into"));
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
        assert!(text.starts_with(
            "Storyboard written: 3 block(s) described, 1 real shot(s), 2 sound(s) placed."
        ));
        assert!(text.contains("Not found: shot of block 3"));
    }
}
