//! Les outils proposés aux agents. Chacun lit la session sur disque,
//! modifie ce qu'il faut et réécrit les fichiers (l'interface ouverte les
//! relit). Les réponses sont du texte en français, lisible par un modèle.

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

/// État des outils : la session, et les derniers résultats de recherche
/// (`ajouter_media` les désigne par leur numéro).
pub struct Tools {
    dir: PathBuf,
    found: Vec<Hit>,
}

/// Fichier accompagnant `project.json` : la transcription du montage.
#[derive(serde::Deserialize)]
struct Montage {
    transcript: Transcript,
}

fn kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Animation => "animation",
        BlockKind::Mixed => "mixte",
        BlockKind::Rush => "rush",
        BlockKind::Effect => "effet",
        BlockKind::Broll => "broll",
    }
}

fn parse_kind(text: &str) -> Result<BlockKind, String> {
    Ok(match text.trim().to_lowercase().as_str() {
        "animation" => BlockKind::Animation,
        "mixte" | "mixed" => BlockKind::Mixed,
        "rush" => BlockKind::Rush,
        "effet" | "effect" => BlockKind::Effect,
        "broll" | "b-roll" => BlockKind::Broll,
        other => {
            return Err(format!(
                "type inconnu : {other} (animation, mixte, rush, effet, broll)"
            ));
        }
    })
}

/// `M:SS.d`.
fn at(t: f64) -> String {
    let t = t.max(0.0);
    format!("{}:{:04.1}", (t / 60.0).floor(), t % 60.0)
}

fn millis(seconds: f64) -> Result<Flicks, String> {
    // Secondes raisonnables : la conversion en millisecondes ne déborde pas.
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

    // --- Lecture et écriture de la session. ---

    fn project(&self) -> Result<Project, String> {
        let json = std::fs::read_to_string(self.dir.join("project.json")).map_err(|_| {
            "pas encore de storyboard : dans l'interface, importe une prise (écran Voix) puis \
             ouvre le Storyboard"
                .to_owned()
        })?;
        Project::from_json(&json).map_err(|e| format!("project.json illisible : {e}"))
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

    /// Le bloc désigné par `value` : son numéro (à partir de 1) ou son
    /// identifiant.
    fn block<'p>(project: &'p Project, value: Option<&Value>) -> Result<&'p Block, String> {
        let blocks = project.blocks();
        match value {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| blocks.get(i))
                .ok_or_else(|| format!("pas de bloc {n} (il y en a {})", blocks.len())),
            Some(Value::String(s)) => {
                if let Ok(n) = s.trim().parse::<usize>() {
                    return Self::block(project, Some(&json!(n)));
                }
                blocks
                    .iter()
                    .find(|b| b.id.to_string() == s.trim())
                    .ok_or_else(|| format!("bloc introuvable : {s}"))
            }
            _ => Err("indique le bloc (`bloc` : son numéro, à partir de 1)".into()),
        }
    }

    fn number_of(project: &Project, id: BlockId) -> String {
        project
            .blocks()
            .iter()
            .position(|b| b.id == id)
            .map_or_else(|| "?".to_owned(), |i| (i.saturating_add(1)).to_string())
    }

    /// Dernier état connu des jobs d'un bloc, d'après `studio/jobs.json`.
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
                    "queued" => "en attente".to_owned(),
                    "running" => "en cours".to_owned(),
                    "failed" => format!(
                        "échec ({})",
                        j.pointer("/status/message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    ),
                    "cancelled" => "annulé".to_owned(),
                    _ => "fait".to_owned(),
                })
            })
    }

    fn block_state(&self, block: &Block) -> String {
        let clips = self.studio().join("clips");
        match block.kind {
            BlockKind::Rush => "prise montrée telle quelle".to_owned(),
            BlockKind::Broll => match &block.broll {
                None => "plan à choisir".to_owned(),
                Some(_) if clips.join(format!("broll-{}.mp4", block.id)).is_file() => {
                    "plan monté".to_owned()
                }
                Some(_) => "plan à monter (aperçu)".to_owned(),
            },
            _ => {
                let job = self.job_status(block.id, false);
                let ready = clips.join(format!("bloc-{}.mp4", block.id)).is_file();
                match (job.as_deref(), ready) {
                    (Some(j @ ("en attente" | "en cours")), _) => j.to_owned(),
                    (_, true) => "animée".to_owned(),
                    (Some(j), false) => j.to_owned(),
                    (None, false) => "à générer".to_owned(),
                }
            }
        }
    }

    // --- Les outils. ---

    /// Description des outils (`tools/list`).
    pub fn list() -> Vec<Value> {
        let bloc = json!({
            "description": "Numéro du bloc (1, 2…) tel que le donne `storyboard`.",
            "type": ["integer", "string"],
        });
        vec![
            json!({
                "name": "storyboard",
                "description": "Le storyboard de la vidéo : format, et pour chaque bloc son numéro, \
                    ses instants, son type, le texte dit par la voix, ce qu'on doit y voir, ses \
                    moments précis, la transition qui suit, et l'état de son animation.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "modifier_bloc",
                "description": "Modifie un bloc. Types : animation (motion design seul), mixte \
                    (face cam + animation), rush (prise telle quelle), effet (effet sur la prise), \
                    broll (vidéo ou image de la bibliothèque). Transition : « coupe », « auto » ou \
                    une description libre. Une animation déjà faite n'est pas refaite : utilise \
                    `generer` ensuite.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "bloc": bloc,
                        "type": { "type": "string", "enum": ["animation", "mixte", "rush", "effet", "broll"] },
                        "description": { "type": "string", "description": "Ce qu'on voit pendant le bloc." },
                        "transition": { "type": "string", "description": "Vers le bloc suivant." },
                        "broll": { "type": "string", "description": "Fichier de la bibliothèque montré par un bloc broll." },
                        "broll_depart": { "type": "number", "description": "Instant de la vidéo où commence le plan, en secondes." },
                        "detourage": { "type": "boolean", "description": "Bloc mixte : détourer la personne (l'animation passe derrière)." },
                    },
                    "required": ["bloc"],
                },
            }),
            json!({
                "name": "ajouter_moment",
                "description": "Pose un moment précis dans un bloc : ce qui doit se passer à \
                    l'écran quand la voix dit un mot.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "bloc": bloc,
                        "mot": { "type": "string", "description": "Mot du bloc (le premier qui correspond)." },
                        "note": { "type": "string", "description": "Ce qui se passe à cet instant." },
                    },
                    "required": ["bloc", "mot", "note"],
                },
            }),
            json!({
                "name": "retirer_moment",
                "description": "Retire le moment posé sur un mot d'un bloc.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "bloc": bloc, "mot": { "type": "string" } },
                    "required": ["bloc", "mot"],
                },
            }),
            json!({
                "name": "bibliotheque",
                "description": "Les fichiers de la bibliothèque (images, SVG, vidéos, polices, sons), \
                    avec leur usage et leur licence.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "chercher_medias",
                "description": "Cherche des images, vidéos ou sons libres de droits (Wikimedia \
                    Commons, Openverse ; Pexels, Pixabay, Freesound avec une clé). Mots en anglais \
                    de préférence. Les résultats sont numérotés pour `ajouter_media`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "requete": { "type": "string" },
                        "type": { "type": "string", "enum": ["image", "video", "son"] },
                    },
                    "required": ["requete"],
                },
            }),
            json!({
                "name": "ajouter_media",
                "description": "Télécharge un résultat de la dernière recherche dans la \
                    bibliothèque, avec son auteur et sa licence.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "resultat": { "type": "integer", "description": "Numéro du résultat." },
                        "description": { "type": "string", "description": "À quoi il sert, pour les animateurs." },
                    },
                    "required": ["resultat"],
                },
            }),
            json!({
                "name": "decrire_fichier",
                "description": "Dit à quoi sert un fichier de la bibliothèque (les animateurs le lisent).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "fichier": { "type": "string" }, "description": { "type": "string" } },
                    "required": ["fichier", "description"],
                },
            }),
            json!({
                "name": "finition",
                "description": "La finition sonore : musiques, effets sonores (SFX), ducking.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "ajouter_musique",
                "description": "Pose une musique de la bibliothèque du début d'un bloc à la fin \
                    d'un autre (par défaut toute la vidéo). Elle baisse sous la voix (ducking).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "fichier": { "type": "string" },
                        "du_bloc": bloc,
                        "au_bloc": bloc,
                        "volume_db": { "type": "number", "description": "−14 par défaut." },
                    },
                    "required": ["fichier"],
                },
            }),
            json!({
                "name": "ajouter_sfx",
                "description": "Pose un effet sonore de la bibliothèque dans un bloc, à un \
                    décalage depuis son début (négatif : avant le bloc, pour un whoosh sur une \
                    coupe), ou sur le mot donné.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "fichier": { "type": "string" },
                        "bloc": bloc,
                        "decalage": { "type": "number", "description": "Secondes depuis le début du bloc." },
                        "mot": { "type": "string", "description": "Ou : sur ce mot du bloc." },
                        "volume_db": { "type": "number" },
                    },
                    "required": ["fichier", "bloc"],
                },
            }),
            json!({
                "name": "retirer_son",
                "description": "Retire une musique ou un SFX de la finition (par son identifiant).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "integer" } },
                    "required": ["id"],
                },
            }),
            json!({
                "name": "regler_ducking",
                "description": "Règle la baisse de la musique sous la voix et le volume de la voix.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "actif": { "type": "boolean" },
                        "baisse_db": { "type": "number", "description": "Négatif, −10 par défaut." },
                        "voix_db": { "type": "number" },
                    },
                },
            }),
            json!({
                "name": "generer",
                "description": "Demande à l'interface d'enfait (qui doit être ouverte) de générer \
                    les animations de blocs, ou tout ce qui reste à générer, transitions comprises. \
                    Suis l'avancement avec `storyboard`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "blocs": { "type": "array", "items": { "type": "integer" } },
                        "tout": { "type": "boolean", "description": "Tout ce qui reste à générer." },
                        "transitions": { "type": "boolean", "description": "Avec les transitions des blocs demandés." },
                    },
                },
            }),
            json!({
                "name": "ecrire_storyboard",
                "description": "Demande à l'interface d'enfait (qui doit être ouverte) de faire \
                    écrire le storyboard par son monteur : type et description des blocs encore \
                    sans description, moments, transitions, plans réels cherchés et téléchargés \
                    (licences libres), effets sonores et musique posés. Les blocs déjà décrits \
                    sont gardés. Une seule étape d'annulation dans l'interface. Relis \
                    `storyboard` et `finition` ensuite (une à deux minutes).",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "mode_magique",
                "description": "Demande à l'interface d'enfait (qui doit être ouverte) le mode \
                    magique sur la prise de la session : nettoyage (dernières prises), storyboard \
                    écrit, plans et sons trouvés, animations, transitions, puis export MP4 à côté \
                    de la prise. Long (plusieurs minutes) et coûteux en modèles ; suis \
                    l'avancement avec `storyboard`.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "credits",
                "description": "Les crédits à publier avec la vidéo (fichiers trouvés en ligne).",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "format",
                "description": "Change le format de la vidéo : grand format 16:9 (YouTube), court vertical 9:16 (Shorts, Reels, TikTok) ou carré. Les animations sont rangées par format : celles d'un autre format reviennent quand on y retourne, les blocs à animer dans le nouveau format sont à regénérer (outil generer). Une prise d'un autre format est rognée pour remplir le cadre (cadrage « rogner », décalable) ou montrée entière avec des bandes (« bandes »).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "format": { "type": "string", "enum": ["grand", "vertical", "carre"] },
                        "cadrage": { "type": "string", "enum": ["rogner", "bandes"] },
                        "decalage": { "type": "integer", "minimum": -50, "maximum": 50, "description": "Décalage du cadre rogné, en % de la marge : -50 bord gauche (ou haut), 50 bord droit (ou bas)." },
                    },
                },
            }),
            json!({
                "name": "exporter",
                "description": "Exporte la vidéo finale (MP4 en pleine définition, post-FX, mixage de la finition, son à -14 LUFS) et/ou le montage pour Premiere Pro (XML FCP7, médias copiés à côté). Les blocs sans animation montrent la prise. Peut prendre plusieurs minutes. Chemins absolus.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "video": { "type": "string", "description": "Chemin du MP4 à écrire." },
                        "premiere": { "type": "string", "description": "Chemin du XML à écrire." },
                        "sfx": { "type": "boolean", "description": "Garder les effets sonores (vrai par défaut)." },
                    },
                },
            }),
            json!({
                "name": "script",
                "description": "Le script de la voix off (texte écrit par l'utilisateur).",
                "inputSchema": { "type": "object", "properties": {} },
            }),
        ]
    }

    pub async fn call(&mut self, name: &str, args: &Value) -> Outcome {
        match name {
            "storyboard" => self.storyboard(),
            "modifier_bloc" => self.set_block(args),
            "ajouter_moment" => self.add_moment(args),
            "retirer_moment" => self.remove_moment(args),
            "bibliotheque" => Ok(self.library()),
            "chercher_medias" => self.search(args).await,
            "ajouter_media" => self.add_media(args).await,
            "decrire_fichier" => self.describe_file(args),
            "finition" => self.finish_text(),
            "ajouter_musique" => self.add_music(args),
            "ajouter_sfx" => self.add_sfx(args),
            "retirer_son" => self.remove_cue(args),
            "regler_ducking" => self.set_ducking(args),
            "generer" => self.generate(args),
            "ecrire_storyboard" => self.project_request(RequestKind::Storyboard),
            "mode_magique" => self.project_request(RequestKind::Magic),
            "credits" => Ok(self.credits()),
            "format" => self.set_format(args),
            "exporter" => self.export(args).await,
            "script" => Ok(std::fs::read_to_string(self.dir.join("script.md"))
                .unwrap_or_else(|_| "(pas de script)".into())),
            other => Err(format!("outil inconnu : {other}")),
        }
    }

    fn storyboard(&self) -> Outcome {
        let project = self.project()?;
        let transcript = self.transcript();
        let settings = project.settings();
        let rate = settings.frame_rate;
        let mut out = format!(
            "Vidéo {} {}×{}, {}/{} i/s, {} blocs, {}. Interface {}.\n\n",
            settings.aspect.label().to_lowercase(),
            settings.resolution.width,
            settings.resolution.height,
            rate.num(),
            rate.den(),
            project.blocks().len(),
            at(project.duration().as_secs_f64()),
            if bridge::app_alive(&self.dir) {
                "ouverte"
            } else {
                "fermée (la génération attendra son ouverture)"
            }
        );
        for (i, b) in project.blocks().iter().enumerate() {
            let _ = writeln!(
                out,
                "## Bloc {} — {} → {} — {} — {}",
                i.saturating_add(1),
                at(b.range.start().as_secs_f64()),
                at(b.range.end().as_secs_f64()),
                kind_name(b.kind),
                self.block_state(b)
            );
            let _ = writeln!(out, "Voix : « {} »", transcript.text_in(b.range));
            if !b.description.trim().is_empty() {
                let _ = writeln!(out, "On voit : {}", b.description.trim());
            }
            for m in &b.moments {
                let _ = writeln!(
                    out,
                    "Moment sur « {} » ({}) : {}",
                    m.word,
                    at(m.at.as_secs_f64()),
                    m.note
                );
            }
            if let Some(broll) = &b.broll {
                let _ = writeln!(
                    out,
                    "Plan : {} à partir de {:.1} s",
                    broll.file,
                    broll.start.as_secs_f64()
                );
            }
            if b.cutout {
                out.push_str("Personne détourée.\n");
            }
            match &b.transition_after {
                Transition::Cut => {}
                Transition::Auto => {
                    let state = self
                        .job_status(b.id, true)
                        .unwrap_or_else(|| "à générer".into());
                    let _ = writeln!(out, "Transition vers le suivant : auto ({state})");
                }
                Transition::Described(t) => {
                    let state = self
                        .job_status(b.id, true)
                        .unwrap_or_else(|| "à générer".into());
                    let _ = writeln!(out, "Transition vers le suivant : {t} ({state})");
                }
            }
            out.push('\n');
        }
        Ok(out)
    }

    fn set_block(&self, args: &Value) -> Outcome {
        let mut project = self.project()?;
        let block = Self::block(&project, args.get("bloc"))?.clone();
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
                "coupe" | "cut" | "" => Transition::Cut,
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
                return Err(format!(
                    "{file} n'est pas dans la bibliothèque (voir `bibliotheque`)"
                ));
            }
            edits.push(Edit::SetBroll {
                block: block.id,
                broll: Some(Broll {
                    file: file.to_owned(),
                    start: millis(number_arg(args, "broll_depart").unwrap_or(0.0).max(0.0))?,
                }),
            });
        } else if let (Some(start), Some(current)) =
            (number_arg(args, "broll_depart"), block.broll.as_ref())
        {
            edits.push(Edit::SetBroll {
                block: block.id,
                broll: Some(Broll {
                    file: current.file.clone(),
                    start: millis(start.max(0.0))?,
                }),
            });
        }
        if let Some(cutout) = args.get("detourage").and_then(Value::as_bool) {
            edits.push(Edit::SetCutout {
                block: block.id,
                cutout,
            });
        }
        if edits.is_empty() {
            return Err("rien à modifier".into());
        }
        for edit in edits {
            edit.apply(&mut project).map_err(|e| e.to_string())?;
        }
        self.save_project(&project)?;
        Ok(format!(
            "Bloc {} modifié.",
            Self::number_of(&project, block.id)
        ))
    }

    /// Instant du premier mot du bloc qui correspond à `word`.
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
                format!(
                    "« {word} » n'est pas dans ce bloc. Mots : {}",
                    words.join(" ")
                )
            })
    }

    fn add_moment(&self, args: &Value) -> Outcome {
        let mut project = self.project()?;
        let transcript = self.transcript();
        let block = Self::block(&project, args.get("bloc"))?.clone();
        let word = text_arg(args, "mot").ok_or("indique le mot (`mot`)")?;
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
            "Moment posé sur « {text} » ({}).",
            at(at_time.as_secs_f64())
        ))
    }

    fn remove_moment(&self, args: &Value) -> Outcome {
        let mut project = self.project()?;
        let transcript = self.transcript();
        let block = Self::block(&project, args.get("bloc"))?.clone();
        let word = text_arg(args, "mot").ok_or("indique le mot (`mot`)")?;
        let (at_time, text) = Self::find_word(&transcript, &block, word)?;
        Edit::RemoveMoment {
            block: block.id,
            at: at_time,
        }
        .apply(&mut project)
        .map_err(|_| format!("pas de moment sur « {text} »"))?;
        self.save_project(&project)?;
        Ok(format!("Moment sur « {text} » retiré."))
    }

    fn library(&self) -> String {
        let look = self.look();
        if look.assets.is_empty() {
            return "Bibliothèque vide.".into();
        }
        let mut out = String::new();
        for a in &look.assets {
            let _ = write!(out, "- {} ({}) : {}", a.file, a.kind.label(), a.name);
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
                        let _ = write!(out, " ; ⚠ {w}");
                    }
                }
                None => out.push_str(" — fichier de l'utilisateur"),
            }
            out.push('\n');
        }
        out
    }

    async fn search(&mut self, args: &Value) -> Outcome {
        let query = text_arg(args, "requete").ok_or("indique la recherche (`requete`)")?;
        let media = match text_arg(args, "type").unwrap_or("image") {
            "video" | "vidéo" => Media::Video,
            "son" | "audio" | "sound" => Media::Audio,
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
            let _ = writeln!(out, "(banque indisponible : {e})");
        }
        if self.found.is_empty() {
            out.push_str("Aucun résultat.\n");
        }
        Ok(out)
    }

    async fn add_media(&mut self, args: &Value) -> Outcome {
        let n = args
            .get("resultat")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("indique le numéro du résultat (`resultat`)")?;
        let hit = n
            .checked_sub(1)
            .and_then(|i| self.found.get(i))
            .cloned()
            .ok_or("pas de tel résultat : relance `chercher_medias`")?;
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
            "Ajouté à la bibliothèque : {file} ({}, {}).",
            hit.origin.license.name,
            terms.summary()
        );
        for w in terms.warnings() {
            let _ = write!(out, " ⚠ {w}.");
        }
        Ok(out)
    }

    fn describe_file(&self, args: &Value) -> Outcome {
        let file = text_arg(args, "fichier").ok_or("indique le fichier")?;
        let description = text_arg(args, "description")
            .unwrap_or("")
            .trim()
            .to_owned();
        let mut look = self.look();
        let asset = look
            .assets
            .iter_mut()
            .find(|a| a.file == file)
            .ok_or_else(|| format!("{file} n'est pas dans la bibliothèque"))?;
        asset.description = description;
        self.save_look(&look)?;
        Ok(format!("{file} décrit."))
    }

    fn finish_text(&self) -> Outcome {
        let project = self.project()?;
        let finish = self.finish();
        let placed: Placed = finish.resolve(&project);
        let mut out = String::new();
        let d = finish.ducking;
        let _ = writeln!(
            out,
            "Ducking : {} ({:+.0} dB sous la voix). Voix : {:+.1} dB.",
            if d.enabled { "actif" } else { "coupé" },
            d.depth_db,
            finish.voice_gain_db
        );
        for m in &finish.music {
            let _ = write!(out, "- Musique {} : {}", m.id, m.file);
            match placed.music.iter().find(|p| p.id == m.id) {
                Some(p) => {
                    let _ = write!(
                        out,
                        ", blocs {} à {} ({} → {}), {:+.0} dB",
                        Self::number_of(&project, m.from),
                        Self::number_of(&project, m.to),
                        at(p.start),
                        at(p.end),
                        m.gain_db
                    );
                }
                None => out.push_str(", son bloc n'existe plus"),
            }
            out.push('\n');
        }
        for s in &finish.sfx {
            let _ = write!(out, "- SFX {} : {}", s.id, s.file);
            match placed.sfx.iter().find(|p| p.id == s.id) {
                Some(p) => {
                    let _ = write!(
                        out,
                        ", bloc {} {:+.2} s ({}), {:+.0} dB",
                        Self::number_of(&project, s.block),
                        s.offset,
                        at(p.at),
                        s.gain_db
                    );
                }
                None => out.push_str(", son bloc n'existe plus"),
            }
            out.push('\n');
        }
        if finish.is_empty() {
            out.push_str("Ni musique ni SFX pour l'instant.\n");
        }
        Ok(out)
    }

    fn sound_file(&self, args: &Value) -> Result<String, String> {
        let file = text_arg(args, "fichier").ok_or("indique le fichier (`fichier`)")?;
        let look = self.look();
        let asset = look.assets.iter().find(|a| a.file == file).ok_or_else(|| {
            format!("{file} n'est pas dans la bibliothèque (voir `bibliotheque`)")
        })?;
        if asset.kind != enfait_core::AssetKind::Audio {
            return Err(format!("{file} n'est pas un son"));
        }
        Ok(file.to_owned())
    }

    fn add_music(&self, args: &Value) -> Outcome {
        let file = self.sound_file(args)?;
        let project = self.project()?;
        let blocks = project.blocks();
        let from = match args.get("du_bloc") {
            Some(v) => Self::block(&project, Some(v))?.id,
            None => blocks.first().ok_or("storyboard vide")?.id,
        };
        let to = match args.get("au_bloc") {
            Some(v) => Self::block(&project, Some(v))?.id,
            None => blocks.last().ok_or("storyboard vide")?.id,
        };
        let mut finish = self.finish();
        let id = finish.add_music(file.clone(), from, to);
        if let (Some(gain), Some(m)) = (number_arg(args, "volume_db"), finish.music_mut(id)) {
            // Gain en dB : petite valeur, la conversion ne perd rien d'utile.
            #[allow(clippy::cast_possible_truncation)]
            {
                m.gain_db = gain as f32;
            }
        }
        self.save_finish(&finish)?;
        Ok(format!(
            "Musique {id} posée : {file}, blocs {} à {}.",
            Self::number_of(&project, from),
            Self::number_of(&project, to)
        ))
    }

    fn add_sfx(&self, args: &Value) -> Outcome {
        let file = self.sound_file(args)?;
        let project = self.project()?;
        let block = Self::block(&project, args.get("bloc"))?;
        let offset = match (text_arg(args, "mot"), number_arg(args, "decalage")) {
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
            "SFX {id} posé : {file}, bloc {} {offset:+.2} s.",
            Self::number_of(&project, block.id)
        ))
    }

    fn remove_cue(&self, args: &Value) -> Outcome {
        let id = args
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("indique `id`")?;
        let mut finish = self.finish();
        let before = finish.music.len().saturating_add(finish.sfx.len());
        finish.remove(id);
        if finish.music.len().saturating_add(finish.sfx.len()) == before {
            return Err(format!("pas de son {id} (voir `finition`)"));
        }
        self.save_finish(&finish)?;
        Ok(format!("Son {id} retiré."))
    }

    fn set_ducking(&self, args: &Value) -> Outcome {
        let mut finish = self.finish();
        if let Some(on) = args.get("actif").and_then(Value::as_bool) {
            finish.ducking.enabled = on;
        }
        // Valeurs en dB, bornées : la conversion ne perd rien d'utile.
        #[allow(clippy::cast_possible_truncation)]
        {
            if let Some(depth) = number_arg(args, "baisse_db") {
                finish.ducking.depth_db = depth.clamp(-40.0, 0.0) as f32;
            }
            if let Some(voice) = number_arg(args, "voix_db") {
                finish.voice_gain_db = voice.clamp(-24.0, 24.0) as f32;
            }
        }
        self.save_finish(&finish)?;
        self.finish_text()
    }

    /// Demande sur tout le projet, exécutée par l'interface.
    fn project_request(&self, kind: RequestKind) -> Outcome {
        if !bridge::app_alive(&self.dir) {
            return Err(
                "l'interface d'enfait n'est pas ouverte sur cette session : \
                        lance `enfait`, puis redemande."
                    .into(),
            );
        }
        bridge::send_request(&self.dir, &Request { block: None, kind })
            .map_err(|e| e.to_string())?;
        Ok(match kind {
            RequestKind::Magic => "Mode magique demandé : l'interface s'en charge. Suis \
                l'avancement avec `storyboard` ; la vidéo sera exportée à côté de la prise."
                .into(),
            _ => "Demande envoyée : le monteur écrit le storyboard dans l'interface. Relis \
                `storyboard` dans une à deux minutes."
                .into(),
        })
    }

    fn generate(&self, args: &Value) -> Outcome {
        let project = self.project()?;
        let blocks = project.blocks();
        let all = args.get("tout").and_then(Value::as_bool).unwrap_or(false);
        let with_transitions = args
            .get("transitions")
            .and_then(Value::as_bool)
            .unwrap_or(all);
        let chosen: Vec<&Block> = if all {
            blocks
                .iter()
                .filter(|b| {
                    matches!(self.block_state(b).as_str(), "à générer")
                        || b.kind == BlockKind::Broll
                })
                .collect()
        } else {
            let numbers = args
                .get("blocs")
                .and_then(Value::as_array)
                .ok_or("indique `blocs` (numéros) ou `tout`")?;
            numbers
                .iter()
                .map(|n| Self::block(&project, Some(n)))
                .collect::<Result<_, _>>()?
        };
        let mut sent = Vec::new();
        for b in &chosen {
            if matches!(b.kind, BlockKind::Rush | BlockKind::Broll) {
                // Rien à générer : la prise telle quelle, ou un plan monté
                // automatiquement par l'aperçu.
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
            sent.push(format!("bloc {}", Self::number_of(&project, b.id)));
        }
        if with_transitions {
            for pair in blocks.windows(2) {
                let [b, _] = pair else { continue };
                let wanted = all || chosen.iter().any(|c| c.id == b.id);
                let done = self.job_status(b.id, true).as_deref() == Some("fait");
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
                        "transition après le bloc {}",
                        Self::number_of(&project, b.id)
                    ));
                }
            }
        }
        if sent.is_empty() {
            return Ok("Rien à générer.".into());
        }
        let mut out = format!("Demandé : {}.", sent.join(", "));
        if bridge::app_alive(&self.dir) {
            out.push_str(" L'interface les met en file ; suis l'avancement avec `storyboard`.");
        } else {
            out.push_str(
                " L'interface d'enfait est fermée : les demandes seront traitées à son ouverture.",
            );
        }
        Ok(out)
    }

    fn set_format(&self, args: &Value) -> Outcome {
        use enfait_core::{AspectRatio, Framing};
        let mut project = self.project()?;
        let mut settings = *project.settings();
        if let Some(format) = text_arg(args, "format") {
            let aspect = match format.trim().to_lowercase().as_str() {
                "grand" | "16:9" | "paysage" => AspectRatio::Landscape16x9,
                "vertical" | "9:16" | "court" => AspectRatio::Portrait9x16,
                "carre" | "carré" | "1:1" => AspectRatio::Square1x1,
                other => return Err(format!("format inconnu : {other} (grand, vertical, carre)")),
            };
            if aspect != settings.aspect {
                settings = settings.with_aspect(aspect);
                // Une prise d'un autre format remplit le cadre.
                let rush = self.rush_aspect();
                settings.framing = if rush.is_some_and(|r| r != aspect) {
                    Framing::Fill
                } else {
                    Framing::Fit
                };
                settings.pan = 0;
            }
        }
        match text_arg(args, "cadrage").map(str::trim) {
            Some("rogner") => settings.framing = Framing::Fill,
            Some("bandes") => settings.framing = Framing::Fit,
            Some(other) => return Err(format!("cadrage inconnu : {other} (rogner, bandes)")),
            None => {}
        }
        if let Some(pan) = args.get("decalage").and_then(Value::as_i64) {
            settings.pan = i8::try_from(pan.clamp(-50, 50)).unwrap_or(0);
        }
        Edit::SetSettings { settings }
            .apply(&mut project)
            .map_err(|e| e.to_string())?;
        self.save_project(&project)?;
        Ok(format!(
            "Format : {} ({}×{}), prise {}.",
            settings.aspect.label(),
            settings.resolution.width,
            settings.resolution.height,
            match settings.framing {
                Framing::Fill => format!("rognée (décalage {})", settings.pan()),
                Framing::Fit => "entière".to_owned(),
            }
        ))
    }

    /// Format des images de la prise, d'après la session.
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
            return Err("indique `video` (chemin du MP4) et/ou `premiere` (chemin du XML)".into());
        }
        for path in video.iter().chain(&premiere) {
            if !path.is_absolute() {
                return Err(format!("chemin absolu attendu : {}", path.display()));
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
                    " Blocs sans animation (la prise les couvre) : {}.",
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
                "Montage Premiere : {} (médias dans {}).{}",
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
                "Vidéo : {} ({}).{}",
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
            "Aucun fichier trouvé en ligne : pas de crédits à publier.".into()
        } else {
            text
        }
    }
}

/// Mot ramené à l'essentiel pour comparer (minuscules, sans ponctuation).
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '’' || *c == '-')
        .flat_map(char::to_lowercase)
        .collect()
}
