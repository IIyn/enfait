//! The tools offered to agents. Each one reads the session from disk,
//! changes what it must and writes the files back (the open interface
//! reloads them). Answers are plain English text, readable by a model.

use crate::bridge::{self, Request, RequestKind};
use enfait_assets::search::{Hit, Media};
use enfait_core::finish::Placed;
use enfait_core::{
    Block, BlockId, BlockKind, Broll, Edit, Finish, Flicks, Look, Moment, Project, Transcript,
    Transition,
};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::PathBuf;

type Outcome = Result<String, String>;

/// State of a block that still has its animation to make.
const TO_GENERATE: &str = "to generate";
const QUEUED: &str = "queued";
const RUNNING: &str = "running";
const DONE: &str = "done";

/// State of the tools: the session, and the last search results
/// (`add_media` refers to them by number).
pub struct Tools {
    dir: PathBuf,
    found: Vec<Hit>,
}

/// File next to `project.json`: the transcript of the edit.
#[derive(serde::Deserialize)]
struct Montage {
    transcript: Transcript,
}

fn kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Animation => "animation",
        BlockKind::Mixed => "mixed",
        BlockKind::Rush => "rush",
        BlockKind::Effect => "effect",
        BlockKind::Broll => "broll",
    }
}

fn parse_kind(text: &str) -> Result<BlockKind, String> {
    Ok(match text.trim().to_lowercase().as_str() {
        "animation" => BlockKind::Animation,
        "mixed" | "mixte" => BlockKind::Mixed,
        "rush" => BlockKind::Rush,
        "effect" | "effet" => BlockKind::Effect,
        "broll" | "b-roll" => BlockKind::Broll,
        other => {
            return Err(format!(
                "unknown type: {other} (animation, mixed, rush, effect, broll)"
            ));
        }
    })
}

/// Tool names of the first versions, still answered.
fn current_name(name: &str) -> &str {
    match name {
        "modifier_bloc" => "edit_block",
        "ajouter_moment" => "add_moment",
        "retirer_moment" => "remove_moment",
        "bibliotheque" => "library",
        "chercher_medias" => "search_media",
        "ajouter_media" => "add_media",
        "decrire_fichier" => "describe_file",
        "finition" => "finish",
        "ajouter_musique" => "add_music",
        "ajouter_sfx" => "add_sfx",
        "retirer_son" => "remove_sound",
        "regler_ducking" => "set_ducking",
        "generer" => "generate",
        "ecrire_storyboard" => "write_storyboard",
        "mode_magique" => "magic_mode",
        "exporter" => "export",
        other => other,
    }
}

/// `M:SS.d`.
fn at(t: f64) -> String {
    let t = t.max(0.0);
    format!("{}:{:04.1}", (t / 60.0).floor(), t % 60.0)
}

fn millis(seconds: f64) -> Result<Flicks, String> {
    // Reasonable seconds: the conversion to milliseconds does not overflow.
    #[allow(clippy::cast_possible_truncation)]
    let ms = (seconds * 1000.0).round() as i64;
    Flicks::from_millis(ms).map_err(|e| e.to_string())
}

fn number_arg(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

impl Tools {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            found: Vec::new(),
        }
    }

    // --- Reading and writing the session. ---

    fn project(&self) -> Result<Project, String> {
        let json = std::fs::read_to_string(self.dir.join("project.json")).map_err(|_| {
            "no storyboard yet: in the app, import a take (Voice screen) then open the \
             Storyboard"
                .to_owned()
        })?;
        Project::from_json(&json).map_err(|e| format!("unreadable project.json: {e}"))
    }

    fn save_project(&self, project: &Project) -> Result<(), String> {
        let json = project.to_json().map_err(|e| e.to_string())?;
        bridge::write_atomic(&self.dir.join("project.json"), json.as_bytes())
            .map_err(|e| e.to_string())
    }

    fn transcript(&self) -> Transcript {
        std::fs::read(self.dir.join("montage.json"))
            .ok()
            .and_then(|d| serde_json::from_slice::<Montage>(&d).ok())
            .map(|m| m.transcript)
            .unwrap_or_default()
    }

    fn look(&self) -> Look {
        std::fs::read(self.dir.join("look.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    fn save_look(&self, look: &Look) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(look).map_err(|e| e.to_string())?;
        bridge::write_atomic(&self.dir.join("look.json"), &json).map_err(|e| e.to_string())
    }

    fn finish(&self) -> Finish {
        std::fs::read(self.dir.join("finish.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    fn save_finish(&self, finish: &Finish) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(finish).map_err(|e| e.to_string())?;
        bridge::write_atomic(&self.dir.join("finish.json"), &json).map_err(|e| e.to_string())
    }

    fn studio(&self) -> PathBuf {
        self.dir.join("studio")
    }

    /// The block pointed to by `value`: its number (from 1) or its id.
    fn block<'p>(project: &'p Project, value: Option<&Value>) -> Result<&'p Block, String> {
        let blocks = project.blocks();
        match value {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| blocks.get(i))
                .ok_or_else(|| format!("no block {n} (there are {})", blocks.len())),
            Some(Value::String(s)) => {
                if let Ok(n) = s.trim().parse::<usize>() {
                    return Self::block(project, Some(&json!(n)));
                }
                blocks
                    .iter()
                    .find(|b| b.id.to_string() == s.trim())
                    .ok_or_else(|| format!("block not found: {s}"))
            }
            _ => Err("give the block (`block`: its number, from 1)".into()),
        }
    }

    fn number_of(project: &Project, id: BlockId) -> String {
        project
            .blocks()
            .iter()
            .position(|b| b.id == id)
            .map_or_else(|| "?".to_owned(), |i| (i.saturating_add(1)).to_string())
    }

    /// Last known state of the jobs of a block, from `studio/jobs.json`.
    fn job_status(&self, id: BlockId, transition: bool) -> Option<String> {
        let jobs: Value =
            serde_json::from_slice(&std::fs::read(self.studio().join("jobs.json")).ok()?).ok()?;
        let id = id.to_string();
        jobs.get("jobs")?
            .as_array()?
            .iter()
            .rev()
            .find(|j| {
                j.get("block").and_then(Value::as_str) == Some(id.as_str())
                    && (j.pointer("/kind/type").and_then(Value::as_str) == Some("transition"))
                        == transition
            })
            .and_then(|j| {
                let status = j.pointer("/status/status").and_then(Value::as_str)?;
                Some(match status {
                    "queued" => QUEUED.to_owned(),
                    "running" => RUNNING.to_owned(),
                    "failed" => format!(
                        "failed ({})",
                        j.pointer("/status/message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    ),
                    "cancelled" => "cancelled".to_owned(),
                    _ => DONE.to_owned(),
                })
            })
    }

    fn block_state(&self, block: &Block) -> String {
        let clips = self.studio().join("clips");
        match block.kind {
            BlockKind::Rush => "take shown as is".to_owned(),
            BlockKind::Broll => match &block.broll {
                None => "shot to choose".to_owned(),
                Some(_) if clips.join(format!("broll-{}.mp4", block.id)).is_file() => {
                    "shot edited".to_owned()
                }
                Some(_) => "shot to edit (preview)".to_owned(),
            },
            _ => {
                let job = self.job_status(block.id, false);
                let ready = clips.join(format!("bloc-{}.mp4", block.id)).is_file();
                match (job.as_deref(), ready) {
                    (Some(j @ (QUEUED | RUNNING)), _) => j.to_owned(),
                    (_, true) => "animated".to_owned(),
                    (Some(j), false) => j.to_owned(),
                    (None, false) => TO_GENERATE.to_owned(),
                }
            }
        }
    }

    // --- The tools. ---

    /// Description of the tools (`tools/list`).
    pub fn list() -> Vec<Value> {
        let block = json!({
            "description": "Number of the block (1, 2…) as given by `storyboard`.",
            "type": ["integer", "string"],
        });
        vec![
            json!({
                "name": "storyboard",
                "description": "The storyboard of the video: format, and for each block its number, \
                    its times, its type, the text spoken by the voice, what must be seen, its \
                    precise moments, the transition after it, and the state of its animation.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "edit_block",
                "description": "Edits a block. Types: animation (motion design only), mixed \
                    (face cam + animation), rush (take as is), effect (effect on the take), \
                    broll (video or image of the library). Transition: \"cut\", \"auto\" or a \
                    free description. An animation already made is not redone: use `generate` \
                    afterwards.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "block": block,
                        "type": { "type": "string", "enum": ["animation", "mixed", "rush", "effect", "broll"] },
                        "description": { "type": "string", "description": "What is seen during the block." },
                        "transition": { "type": "string", "description": "To the next block." },
                        "broll": { "type": "string", "description": "Library file shown by a broll block." },
                        "broll_start": { "type": "number", "description": "Time of the video where the shot starts, in seconds." },
                        "cutout": { "type": "boolean", "description": "Mixed block: cut the person out (the animation passes behind)." },
                    },
                    "required": ["block"],
                },
            }),
            json!({
                "name": "add_moment",
                "description": "Sets a precise moment in a block: what must happen on screen \
                    when the voice says a word.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "block": block,
                        "word": { "type": "string", "description": "Word of the block (the first one matching)." },
                        "note": { "type": "string", "description": "What happens at that time." },
                    },
                    "required": ["block", "word", "note"],
                },
            }),
            json!({
                "name": "remove_moment",
                "description": "Removes the moment set on a word of a block.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "block": block, "word": { "type": "string" } },
                    "required": ["block", "word"],
                },
            }),
            json!({
                "name": "library",
                "description": "The files of the library (images, SVG, videos, fonts, sounds), \
                    with their use and their licence.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "search_media",
                "description": "Searches royalty-free images, videos or sounds (Wikimedia \
                    Commons, Openverse; Pexels, Pixabay, Freesound with a key). English words \
                    preferably. Results are numbered for `add_media`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "media": { "type": "string", "enum": ["image", "video", "sound"] },
                    },
                    "required": ["query"],
                },
            }),
            json!({
                "name": "add_media",
                "description": "Downloads a result of the last search into the library, with \
                    its author and licence.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "result": { "type": "integer", "description": "Number of the result." },
                        "description": { "type": "string", "description": "What it is for, for the animators." },
                    },
                    "required": ["result"],
                },
            }),
            json!({
                "name": "describe_file",
                "description": "Says what a library file is for (the animators read it).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "file": { "type": "string" }, "description": { "type": "string" } },
                    "required": ["file", "description"],
                },
            }),
            json!({
                "name": "finish",
                "description": "The sound finish: music, sound effects (SFX), ducking.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "add_music",
                "description": "Places a library music from the start of a block to the end \
                    of another (by default the whole video). It dips under the voice (ducking).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string" },
                        "from_block": block,
                        "to_block": block,
                        "volume_db": { "type": "number", "description": "−14 by default." },
                    },
                    "required": ["file"],
                },
            }),
            json!({
                "name": "add_sfx",
                "description": "Places a library sound effect in a block, at an offset from \
                    its start (negative: before the block, for a whoosh on a cut), or on the \
                    given word.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string" },
                        "block": block,
                        "offset": { "type": "number", "description": "Seconds from the start of the block." },
                        "word": { "type": "string", "description": "Or: on this word of the block." },
                        "volume_db": { "type": "number" },
                    },
                    "required": ["file", "block"],
                },
            }),
            json!({
                "name": "remove_sound",
                "description": "Removes a music or a SFX from the finish (by its id).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "integer" } },
                    "required": ["id"],
                },
            }),
            json!({
                "name": "set_ducking",
                "description": "Sets how much the music dips under the voice, and the voice volume.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "enabled": { "type": "boolean" },
                        "depth_db": { "type": "number", "description": "Negative, −10 by default." },
                        "voice_db": { "type": "number" },
                    },
                },
            }),
            json!({
                "name": "generate",
                "description": "Asks the enfait app (which must be open) to generate the \
                    animations of blocks, or everything left to generate, transitions \
                    included. Follow the progress with `storyboard`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "blocks": { "type": "array", "items": { "type": "integer" } },
                        "all": { "type": "boolean", "description": "Everything left to generate." },
                        "transitions": { "type": "boolean", "description": "With the transitions of the asked blocks." },
                    },
                },
            }),
            json!({
                "name": "write_storyboard",
                "description": "Asks the enfait app (which must be open) to have its editor \
                    write the storyboard: type and description of the blocks still without a \
                    description, moments, transitions, real shots searched and downloaded \
                    (free licences), sound effects and music placed. Blocks already described \
                    are kept. A single undo step in the app. Read `storyboard` and `finish` \
                    afterwards (one to two minutes).",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "magic_mode",
                "description": "Asks the enfait app (which must be open) for magic mode on the \
                    take of the session: cleanup (last takes), written storyboard, shots and \
                    sounds found, animations, transitions, then an MP4 export next to the take. \
                    Long (several minutes) and costly in models; follow the progress with \
                    `storyboard`.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "credits",
                "description": "The credits to publish with the video (files found online).",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "format",
                "description": "Changes the format of the video: landscape 16:9 (YouTube), vertical 9:16 (Shorts, Reels, TikTok) or square. Animations are stored per format: those of another format come back when returning to it, the blocks to animate in the new format must be generated again (`generate` tool). A take of another format is cropped to fill the frame (framing \"crop\", can be shifted) or shown whole with bars (\"bars\").",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "format": { "type": "string", "enum": ["landscape", "vertical", "square"] },
                        "framing": { "type": "string", "enum": ["crop", "bars"] },
                        "pan": { "type": "integer", "minimum": -50, "maximum": 50, "description": "Shift of the cropped frame, in % of the margin: -50 left (or top) edge, 50 right (or bottom) edge." },
                    },
                },
            }),
            json!({
                "name": "export",
                "description": "Exports the final video (full-definition MP4, post-FX, mix of the finish, sound at -14 LUFS) and/or the edit for Premiere Pro (FCP7 XML, media copied next to it). Blocks without animation show the take. May take several minutes. Absolute paths.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "video": { "type": "string", "description": "Path of the MP4 to write." },
                        "premiere": { "type": "string", "description": "Path of the XML to write." },
                        "sfx": { "type": "boolean", "description": "Keep the sound effects (true by default)." },
                    },
                },
            }),
            json!({
                "name": "script",
                "description": "The script of the voice-over (text written by the user).",
                "inputSchema": { "type": "object", "properties": {} },
            }),
        ]
    }

    pub async fn call(&mut self, name: &str, args: &Value) -> Outcome {
        match current_name(name) {
            "storyboard" => self.storyboard(),
            "edit_block" => self.set_block(args),
            "add_moment" => self.add_moment(args),
            "remove_moment" => self.remove_moment(args),
            "library" => Ok(self.library()),
            "search_media" => self.search(args).await,
            "add_media" => self.add_media(args).await,
            "describe_file" => self.describe_file(args),
            "finish" => self.finish_text(),
            "add_music" => self.add_music(args),
            "add_sfx" => self.add_sfx(args),
            "remove_sound" => self.remove_cue(args),
            "set_ducking" => self.set_ducking(args),
            "generate" => self.generate(args),
            "write_storyboard" => self.project_request(RequestKind::Storyboard),
            "magic_mode" => self.project_request(RequestKind::Magic),
            "credits" => Ok(self.credits()),
            "format" => self.set_format(args),
            "export" => self.export(args).await,
            "script" => Ok(std::fs::read_to_string(self.dir.join("script.md"))
                .unwrap_or_else(|_| "(no script)".into())),
            other => Err(format!("unknown tool: {other}")),
        }
    }

    fn storyboard(&self) -> Outcome {
        let project = self.project()?;
        let transcript = self.transcript();
        let settings = project.settings();
        let rate = settings.frame_rate;
        let mut out = format!(
            "{} video {}×{}, {}/{} fps, {} blocks, {}. App {}.\n\n",
            settings.aspect.label(),
            settings.resolution.width,
            settings.resolution.height,
            rate.num(),
            rate.den(),
            project.blocks().len(),
            at(project.duration().as_secs_f64()),
            if bridge::app_alive(&self.dir) {
                "open"
            } else {
                "closed (generation will wait until it opens)"
            }
        );
        for (i, b) in project.blocks().iter().enumerate() {
            let _ = writeln!(
                out,
                "## Block {} — {} → {} — {} — {}",
                i.saturating_add(1),
                at(b.range.start().as_secs_f64()),
                at(b.range.end().as_secs_f64()),
                kind_name(b.kind),
                self.block_state(b)
            );
            let _ = writeln!(out, "Voice: “{}”", transcript.text_in(b.range));
            if !b.description.trim().is_empty() {
                let _ = writeln!(out, "On screen: {}", b.description.trim());
            }
            for m in &b.moments {
                let _ = writeln!(
                    out,
                    "Moment on “{}” ({}): {}",
                    m.word,
                    at(m.at.as_secs_f64()),
                    m.note
                );
            }
            if let Some(broll) = &b.broll {
                let _ = writeln!(
                    out,
                    "Shot: {} from {:.1} s",
                    broll.file,
                    broll.start.as_secs_f64()
                );
            }
            if b.cutout {
                out.push_str("Person cut out.\n");
            }
            match &b.transition_after {
                Transition::Cut => {}
                Transition::Auto => {
                    let state = self
                        .job_status(b.id, true)
                        .unwrap_or_else(|| TO_GENERATE.into());
                    let _ = writeln!(out, "Transition to the next one: auto ({state})");
                }
                Transition::Described(t) => {
                    let state = self
                        .job_status(b.id, true)
                        .unwrap_or_else(|| TO_GENERATE.into());
                    let _ = writeln!(out, "Transition to the next one: {t} ({state})");
                }
            }
            out.push('\n');
        }
        Ok(out)
    }

    fn set_block(&self, args: &Value) -> Outcome {
        let mut project = self.project()?;
        let block = Self::block(&project, args.get("block"))?.clone();
        let mut edits = Vec::new();
        if let Some(kind) = text_arg(args, "type") {
            edits.push(Edit::SetKind {
                block: block.id,
                kind: parse_kind(kind)?,
            });
        }
        if let Some(text) = text_arg(args, "description") {
            edits.push(Edit::SetDescription {
                block: block.id,
                text: text.trim().to_owned(),
            });
        }
        if let Some(t) = text_arg(args, "transition") {
            let transition = match t.trim().to_lowercase().as_str() {
                "cut" | "coupe" | "" => Transition::Cut,
                "auto" => Transition::Auto,
                _ => Transition::Described(t.trim().to_owned()),
            };
            edits.push(Edit::SetTransition {
                block: block.id,
                transition,
            });
        }
        if let Some(file) = text_arg(args, "broll") {
            let look = self.look();
            if !look.assets.iter().any(|a| a.file == file) {
                return Err(format!("{file} is not in the library (see `library`)"));
            }
            edits.push(Edit::SetBroll {
                block: block.id,
                broll: Some(Broll {
                    file: file.to_owned(),
                    start: millis(number_arg(args, "broll_start").unwrap_or(0.0).max(0.0))?,
                }),
            });
        } else if let (Some(start), Some(current)) =
            (number_arg(args, "broll_start"), block.broll.as_ref())
        {
            edits.push(Edit::SetBroll {
                block: block.id,
                broll: Some(Broll {
                    file: current.file.clone(),
                    start: millis(start.max(0.0))?,
                }),
            });
        }
        if let Some(cutout) = args.get("cutout").and_then(Value::as_bool) {
            edits.push(Edit::SetCutout {
                block: block.id,
                cutout,
            });
        }
        if edits.is_empty() {
            return Err("nothing to change".into());
        }
        for edit in edits {
            edit.apply(&mut project).map_err(|e| e.to_string())?;
        }
        self.save_project(&project)?;
        Ok(format!(
            "Block {} updated.",
            Self::number_of(&project, block.id)
        ))
    }

    /// Time of the first word of the block matching `word`.
    fn find_word(
        transcript: &Transcript,
        block: &Block,
        word: &str,
    ) -> Result<(Flicks, String), String> {
        let wanted = normalize(word);
        transcript
            .words_in(block.range)
            .find(|w| normalize(&w.text) == wanted)
            .or_else(|| {
                transcript
                    .words_in(block.range)
                    .find(|w| normalize(&w.text).contains(&wanted))
            })
            .map(|w| (w.range.start(), w.text.clone()))
            .ok_or_else(|| {
                let words: Vec<String> = transcript
                    .words_in(block.range)
                    .map(|w| w.text.clone())
                    .collect();
                format!("“{word}” is not in this block. Words: {}", words.join(" "))
            })
    }

    fn add_moment(&self, args: &Value) -> Outcome {
        let mut project = self.project()?;
        let transcript = self.transcript();
        let block = Self::block(&project, args.get("block"))?.clone();
        let word = text_arg(args, "word").ok_or("give the word (`word`)")?;
        let note = text_arg(args, "note").unwrap_or("").trim().to_owned();
        let (at_time, text) = Self::find_word(&transcript, &block, word)?;
        if block.moments.iter().any(|m| m.at == at_time) {
            Edit::SetMomentNote {
                block: block.id,
                at: at_time,
                note,
            }
            .apply(&mut project)
            .map_err(|e| e.to_string())?;
        } else {
            Edit::AddMoment {
                block: block.id,
                moment: Moment {
                    at: at_time,
                    word: text.clone(),
                    note,
                },
            }
            .apply(&mut project)
            .map_err(|e| e.to_string())?;
        }
        self.save_project(&project)?;
        Ok(format!(
            "Moment set on “{text}” ({}).",
            at(at_time.as_secs_f64())
        ))
    }

    fn remove_moment(&self, args: &Value) -> Outcome {
        let mut project = self.project()?;
        let transcript = self.transcript();
        let block = Self::block(&project, args.get("block"))?.clone();
        let word = text_arg(args, "word").ok_or("give the word (`word`)")?;
        let (at_time, text) = Self::find_word(&transcript, &block, word)?;
        Edit::RemoveMoment {
            block: block.id,
            at: at_time,
        }
        .apply(&mut project)
        .map_err(|_| format!("no moment on “{text}”"))?;
        self.save_project(&project)?;
        Ok(format!("Moment on “{text}” removed."))
    }

    fn library(&self) -> String {
        let look = self.look();
        if look.assets.is_empty() {
            return "Empty library.".into();
        }
        let mut out = String::new();
        for a in &look.assets {
            let _ = write!(out, "- {} ({}): {}", a.file, a.kind.label(), a.name);
            if !a.description.is_empty() {
                let _ = write!(out, " — {}", a.description);
            }
            match &a.origin {
                Some(o) => {
                    let terms = o.license.terms();
                    let _ = write!(
                        out,
                        " — {} ({}), {}",
                        o.license.name,
                        terms.summary(),
                        o.source
                    );
                    for w in terms.warnings() {
                        let _ = write!(out, "; ⚠ {w}");
                    }
                }
                None => out.push_str(" — user's file"),
            }
            out.push('\n');
        }
        out
    }

    async fn search(&mut self, args: &Value) -> Outcome {
        let query = text_arg(args, "query").ok_or("give the search (`query`)")?;
        let media = match text_arg(args, "media").unwrap_or("image") {
            "video" | "vidéo" => Media::Video,
            "sound" | "audio" | "son" => Media::Audio,
            _ => Media::Image,
        };
        let client = enfait_assets::search::client().map_err(|e| e.to_string())?;
        let results = enfait_assets::search::search(&client, query, media).await;
        self.found = results.hits;
        let mut out = String::new();
        for (i, h) in self.found.iter().enumerate() {
            let terms = h.origin.license.terms();
            let _ = write!(
                out,
                "{}. [{}] {} — {}",
                i.saturating_add(1),
                h.kind.label(),
                h.origin.title,
                h.origin.license.name
            );
            if h.width > 0 {
                let _ = write!(out, " — {}×{}", h.width, h.height);
            }
            if let Some(d) = h.duration {
                let _ = write!(out, " — {d:.1} s");
            }
            let _ = write!(out, " — {} ({})", h.source.name(), terms.summary());
            if terms.restrictive() {
                out.push_str(" ⚠ restrictions");
            }
            out.push('\n');
        }
        for e in &results.errors {
            let _ = writeln!(out, "(bank unavailable: {e})");
        }
        if self.found.is_empty() {
            out.push_str("No result.\n");
        }
        Ok(out)
    }

    async fn add_media(&mut self, args: &Value) -> Outcome {
        let n = args
            .get("result")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("give the number of the result (`result`)")?;
        let hit = n
            .checked_sub(1)
            .and_then(|i| self.found.get(i))
            .cloned()
            .ok_or("no such result: run `search_media` again")?;
        let client = enfait_assets::search::client().map_err(|e| e.to_string())?;
        let tmp = self.dir.join("studio").join("downloads").join(format!(
            "mcp-{}.{}",
            std::process::id(),
            hit.ext
        ));
        enfait_assets::search::download(&client, &hit.file, &tmp)
            .await
            .map_err(|e| e.to_string())?;
        let added = enfait_assets::library::add_file(
            &self.dir.join("library"),
            &tmp,
            &hit.origin.title,
            Some(&hit.origin.title),
            Some(hit.origin.clone()),
        );
        let _ = std::fs::remove_file(&tmp);
        let mut asset = added?;
        if let Some(d) = text_arg(args, "description") {
            asset.description = d.trim().to_owned();
        }
        let file = asset.file.clone();
        let mut look = self.look();
        look.assets.push(asset);
        self.save_look(&look)?;
        let terms = hit.origin.license.terms();
        let mut out = format!(
            "Added to the library: {file} ({}, {}).",
            hit.origin.license.name,
            terms.summary()
        );
        for w in terms.warnings() {
            let _ = write!(out, " ⚠ {w}.");
        }
        Ok(out)
    }

    fn describe_file(&self, args: &Value) -> Outcome {
        let file = text_arg(args, "file").ok_or("give the file (`file`)")?;
        let description = text_arg(args, "description")
            .unwrap_or("")
            .trim()
            .to_owned();
        let mut look = self.look();
        let asset = look
            .assets
            .iter_mut()
            .find(|a| a.file == file)
            .ok_or_else(|| format!("{file} is not in the library"))?;
        asset.description = description;
        self.save_look(&look)?;
        Ok(format!("{file} described."))
    }

    fn finish_text(&self) -> Outcome {
        let project = self.project()?;
        let finish = self.finish();
        let placed: Placed = finish.resolve(&project);
        let mut out = String::new();
        let d = finish.ducking;
        let _ = writeln!(
            out,
            "Ducking: {} ({:+.0} dB under the voice). Voice: {:+.1} dB.",
            if d.enabled { "on" } else { "off" },
            d.depth_db,
            finish.voice_gain_db
        );
        for m in &finish.music {
            let _ = write!(out, "- Music {}: {}", m.id, m.file);
            match placed.music.iter().find(|p| p.id == m.id) {
                Some(p) => {
                    let _ = write!(
                        out,
                        ", blocks {} to {} ({} → {}), {:+.0} dB",
                        Self::number_of(&project, m.from),
                        Self::number_of(&project, m.to),
                        at(p.start),
                        at(p.end),
                        m.gain_db
                    );
                }
                None => out.push_str(", its block no longer exists"),
            }
            out.push('\n');
        }
        for s in &finish.sfx {
            let _ = write!(out, "- SFX {}: {}", s.id, s.file);
            match placed.sfx.iter().find(|p| p.id == s.id) {
                Some(p) => {
                    let _ = write!(
                        out,
                        ", block {} {:+.2} s ({}), {:+.0} dB",
                        Self::number_of(&project, s.block),
                        s.offset,
                        at(p.at),
                        s.gain_db
                    );
                }
                None => out.push_str(", its block no longer exists"),
            }
            out.push('\n');
        }
        if finish.is_empty() {
            out.push_str("No music or SFX yet.\n");
        }
        Ok(out)
    }

    fn sound_file(&self, args: &Value) -> Result<String, String> {
        let file = text_arg(args, "file").ok_or("give the file (`file`)")?;
        let look = self.look();
        let asset = look
            .assets
            .iter()
            .find(|a| a.file == file)
            .ok_or_else(|| format!("{file} is not in the library (see `library`)"))?;
        if asset.kind != enfait_core::AssetKind::Audio {
            return Err(format!("{file} is not a sound"));
        }
        Ok(file.to_owned())
    }

    fn add_music(&self, args: &Value) -> Outcome {
        let file = self.sound_file(args)?;
        let project = self.project()?;
        let blocks = project.blocks();
        let from = match args.get("from_block") {
            Some(v) => Self::block(&project, Some(v))?.id,
            None => blocks.first().ok_or("empty storyboard")?.id,
        };
        let to = match args.get("to_block") {
            Some(v) => Self::block(&project, Some(v))?.id,
            None => blocks.last().ok_or("empty storyboard")?.id,
        };
        let mut finish = self.finish();
        let id = finish.add_music(file.clone(), from, to);
        if let (Some(gain), Some(m)) = (number_arg(args, "volume_db"), finish.music_mut(id)) {
            // Gain in dB: small value, the conversion loses nothing useful.
            #[allow(clippy::cast_possible_truncation)]
            {
                m.gain_db = gain as f32;
            }
        }
        self.save_finish(&finish)?;
        Ok(format!(
            "Music {id} placed: {file}, blocks {} to {}.",
            Self::number_of(&project, from),
            Self::number_of(&project, to)
        ))
    }

    fn add_sfx(&self, args: &Value) -> Outcome {
        let file = self.sound_file(args)?;
        let project = self.project()?;
        let block = Self::block(&project, args.get("block"))?;
        let offset = match (text_arg(args, "word"), number_arg(args, "offset")) {
            (Some(word), _) => {
                let (t, _) = Self::find_word(&self.transcript(), block, word)?;
                t.as_secs_f64() - block.range.start().as_secs_f64()
            }
            (None, offset) => offset.unwrap_or(0.0),
        };
        let mut finish = self.finish();
        let id = finish.add_sfx(file.clone(), block.id, offset);
        if let (Some(gain), Some(s)) = (number_arg(args, "volume_db"), finish.sfx_mut(id)) {
            #[allow(clippy::cast_possible_truncation)]
            {
                s.gain_db = gain as f32;
            }
        }
        self.save_finish(&finish)?;
        Ok(format!(
            "SFX {id} placed: {file}, block {} {offset:+.2} s.",
            Self::number_of(&project, block.id)
        ))
    }

    fn remove_cue(&self, args: &Value) -> Outcome {
        let id = args.get("id").and_then(Value::as_u64).ok_or("give `id`")?;
        let mut finish = self.finish();
        let before = finish.music.len().saturating_add(finish.sfx.len());
        finish.remove(id);
        if finish.music.len().saturating_add(finish.sfx.len()) == before {
            return Err(format!("no sound {id} (see `finish`)"));
        }
        self.save_finish(&finish)?;
        Ok(format!("Sound {id} removed."))
    }

    fn set_ducking(&self, args: &Value) -> Outcome {
        let mut finish = self.finish();
        if let Some(on) = args.get("enabled").and_then(Value::as_bool) {
            finish.ducking.enabled = on;
        }
        // Values in dB, bounded: the conversion loses nothing useful.
        #[allow(clippy::cast_possible_truncation)]
        {
            if let Some(depth) = number_arg(args, "depth_db") {
                finish.ducking.depth_db = depth.clamp(-40.0, 0.0) as f32;
            }
            if let Some(voice) = number_arg(args, "voice_db") {
                finish.voice_gain_db = voice.clamp(-24.0, 24.0) as f32;
            }
        }
        self.save_finish(&finish)?;
        self.finish_text()
    }

    /// Request on the whole project, run by the app.
    fn project_request(&self, kind: RequestKind) -> Outcome {
        if !bridge::app_alive(&self.dir) {
            return Err(
                "the enfait app is not open on this session: start `enfait`, then ask again."
                    .into(),
            );
        }
        bridge::send_request(&self.dir, &Request { block: None, kind })
            .map_err(|e| e.to_string())?;
        Ok(match kind {
            RequestKind::Magic => "Magic mode requested: the app takes care of it. Follow the \
                progress with `storyboard`; the video will be exported next to the take."
                .into(),
            _ => "Request sent: the editor writes the storyboard in the app. Read \
                `storyboard` again in one to two minutes."
                .into(),
        })
    }

    fn generate(&self, args: &Value) -> Outcome {
        let project = self.project()?;
        let blocks = project.blocks();
        let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
        let with_transitions = args
            .get("transitions")
            .and_then(Value::as_bool)
            .unwrap_or(all);
        let chosen: Vec<&Block> = if all {
            blocks
                .iter()
                .filter(|b| self.block_state(b) == TO_GENERATE || b.kind == BlockKind::Broll)
                .collect()
        } else {
            let numbers = args
                .get("blocks")
                .and_then(Value::as_array)
                .ok_or("give `blocks` (numbers) or `all`")?;
            numbers
                .iter()
                .map(|n| Self::block(&project, Some(n)))
                .collect::<Result<_, _>>()?
        };
        let mut sent = Vec::new();
        for b in &chosen {
            if matches!(b.kind, BlockKind::Rush | BlockKind::Broll) {
                // Nothing to generate: the take as is, or a shot edited
                // automatically by the preview.
                continue;
            }
            bridge::send_request(
                &self.dir,
                &Request {
                    block: Some(b.id),
                    kind: RequestKind::Animation,
                },
            )
            .map_err(|e| e.to_string())?;
            sent.push(format!("block {}", Self::number_of(&project, b.id)));
        }
        if with_transitions {
            for pair in blocks.windows(2) {
                let [b, _] = pair else { continue };
                let wanted = all || chosen.iter().any(|c| c.id == b.id);
                let done = self.job_status(b.id, true).as_deref() == Some(DONE);
                if wanted && b.transition_after != Transition::Cut && !(all && done) {
                    bridge::send_request(
                        &self.dir,
                        &Request {
                            block: Some(b.id),
                            kind: RequestKind::Transition,
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    sent.push(format!(
                        "transition after block {}",
                        Self::number_of(&project, b.id)
                    ));
                }
            }
        }
        if sent.is_empty() {
            return Ok("Nothing to generate.".into());
        }
        let mut out = format!("Requested: {}.", sent.join(", "));
        if bridge::app_alive(&self.dir) {
            out.push_str(" The app queues them; follow the progress with `storyboard`.");
        } else {
            out.push_str(" The enfait app is closed: the requests will run when it opens.");
        }
        Ok(out)
    }

    fn set_format(&self, args: &Value) -> Outcome {
        use enfait_core::{AspectRatio, Framing};
        let mut project = self.project()?;
        let mut settings = *project.settings();
        if let Some(format) = text_arg(args, "format") {
            let aspect = match format.trim().to_lowercase().as_str() {
                "landscape" | "16:9" | "grand" | "paysage" => AspectRatio::Landscape16x9,
                "vertical" | "9:16" | "court" => AspectRatio::Portrait9x16,
                "square" | "1:1" | "carre" | "carré" => AspectRatio::Square1x1,
                other => {
                    return Err(format!(
                        "unknown format: {other} (landscape, vertical, square)"
                    ));
                }
            };
            if aspect != settings.aspect {
                settings = settings.with_aspect(aspect);
                // A take of another format fills the frame.
                let rush = self.rush_aspect();
                settings.framing = if rush.is_some_and(|r| r != aspect) {
                    Framing::Fill
                } else {
                    Framing::Fit
                };
                settings.pan = 0;
            }
        }
        match text_arg(args, "framing").map(str::trim) {
            Some("crop" | "rogner") => settings.framing = Framing::Fill,
            Some("bars" | "bandes") => settings.framing = Framing::Fit,
            Some(other) => return Err(format!("unknown framing: {other} (crop, bars)")),
            None => {}
        }
        if let Some(pan) = args.get("pan").and_then(Value::as_i64) {
            settings.pan = i8::try_from(pan.clamp(-50, 50)).unwrap_or(0);
        }
        Edit::SetSettings { settings }
            .apply(&mut project)
            .map_err(|e| e.to_string())?;
        self.save_project(&project)?;
        Ok(format!(
            "Format: {} ({}×{}), take {}.",
            settings.aspect.label(),
            settings.resolution.width,
            settings.resolution.height,
            match settings.framing {
                Framing::Fill => format!("cropped (shift {})", settings.pan()),
                Framing::Fit => "whole".to_owned(),
            }
        ))
    }

    /// Format of the frames of the take, from the session.
    fn rush_aspect(&self) -> Option<enfait_core::AspectRatio> {
        let data: Value =
            serde_json::from_slice(&std::fs::read(self.dir.join("session.json")).ok()?).ok()?;
        let rush = data.get("rush")?.as_str()?;
        let info = enfait_studio::probe_video(std::path::Path::new(rush))?;
        Some(enfait_core::AspectRatio::nearest(info.0, info.1))
    }

    async fn export(&self, args: &Value) -> Outcome {
        let video = text_arg(args, "video").map(PathBuf::from);
        let premiere = text_arg(args, "premiere").map(PathBuf::from);
        if video.is_none() && premiere.is_none() {
            return Err(
                "give `video` (path of the MP4) and/or `premiere` (path of the XML)".into(),
            );
        }
        for path in video.iter().chain(&premiere) {
            if !path.is_absolute() {
                return Err(format!("absolute path expected: {}", path.display()));
            }
        }
        let sfx = args.get("sfx").and_then(Value::as_bool).unwrap_or(true);
        let session =
            enfait_studio::session::SessionFiles::load(&self.dir).map_err(|e| format!("{e:#}"))?;
        let sources = session.sources();
        let mut out = String::new();
        let missing = |list: &[usize]| {
            if list.is_empty() {
                String::new()
            } else {
                let l: Vec<String> = list.iter().map(ToString::to_string).collect();
                format!(
                    " Blocks without animation (the take covers them): {}.",
                    l.join(", ")
                )
            }
        };
        if let Some(xml) = &premiere {
            let report = enfait_studio::premiere::export(
                &sources,
                enfait_studio::premiere::PremiereOptions { sfx },
                xml,
            )
            .map_err(|e| format!("{e:#}"))?;
            let _ = writeln!(
                out,
                "Premiere edit: {} (media in {}).{}",
                report.xml.display(),
                report.media.display(),
                missing(&report.missing)
            );
        }
        if let Some(mp4) = &video {
            let report = enfait_studio::export::video(
                &sources,
                enfait_studio::export::ExportOptions {
                    sfx,
                    ..Default::default()
                },
                mp4,
                &|_| {},
            )
            .await
            .map_err(|e| format!("{e:#}"))?;
            let _ = writeln!(
                out,
                "Video: {} ({}).{}",
                report.path.display(),
                at(report.duration),
                missing(&report.missing)
            );
        }
        Ok(out)
    }

    fn credits(&self) -> String {
        let text = enfait_core::license::credits(&self.look().assets);
        if text.is_empty() {
            "No file found online: no credits to publish.".into()
        } else {
            text
        }
    }
}

/// Word reduced to the essentials to compare (lowercase, no punctuation).
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '’' || *c == '-')
        .flat_map(char::to_lowercase)
        .collect()
}
