//! « Écrire le storyboard » sans modèle : le monteur répond d'abord
//! n'importe quoi, puis un storyboard ; le plan lu s'applique au projet en
//! une étape d'annulation.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use enfait_ai::editor::EditorInput;
use enfait_ai::{AiError, Provider, Quality, Request};
use enfait_core::{
    Block, BlockKind, Edit, Flicks, History, Project, ProjectSettings, TimeRange, Transcript, Word,
};
use enfait_studio::magic;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn secs(t: f64) -> Flicks {
    #[allow(clippy::cast_possible_truncation)]
    Flicks::from_millis((t * 1000.0).round() as i64).unwrap()
}

fn project() -> (Project, Transcript) {
    let mut words = Vec::new();
    let mut blocks = Vec::new();
    for (b, text) in [["Le", "train", "part."], ["Fin", "du", "voyage."]]
        .iter()
        .enumerate()
    {
        #[allow(clippy::cast_precision_loss)]
        let t0 = 3.0 * b as f64;
        for (k, w) in text.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
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
    let project = Project::with_blocks("essai", ProjectSettings::default(), blocks).unwrap();
    let transcript = Transcript {
        language: "fr".into(),
        words,
        media_duration: secs(6.0),
    };
    (project, transcript)
}

#[tokio::test]
async fn storyboard_written_without_a_model() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let provider = Provider::Canned(Arc::new(
        move |request: &Request| -> Result<String, AiError> {
            assert!(request.system.contains("monteur"));
            assert_eq!(request.model, "claude-sonnet-5-5", "Rapide : Sonnet");
            Ok(if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                "Je réfléchis, sans JSON.".into()
            } else {
                r#"Le propos : un départ.
```json
{"blocks":[
 {"index":1,"kind":"animation","description":"Un train en traits blancs démarre sur « part ».",
  "moments":[{"word":"part","note":"la locomotive s'élance"}],
  "transition":{"type":"auto"}},
 {"index":2,"kind":"rush","description":"face caméra"}
],"music":null}
```"#
                    .into()
            })
        },
    ));
    let (mut project, transcript) = project();
    let before = project.clone();
    let blocks = magic::editor_blocks(&project, &transcript, true);
    assert_eq!(blocks[0].text, "Le train part.");
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
        max_transitions: magic::max_transitions(blocks.len()),
    };
    let work = tempfile::tempdir().unwrap();
    let plan = magic::write_plan(&provider, work.path(), &input, Quality::Fast)
        .await
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "une réponse illisible, un nouvel essai"
    );
    assert!(work.path().join("editor.json").is_file());
    assert!(work.path().join("editor.notes.md").is_file());
    assert_eq!(
        plan.blocks[1].kind, "animation",
        "voix seule : pas de bloc filmé"
    );

    // Rien à chercher : pas de réseau.
    let library = work.path().join("library");
    let gathered = magic::gather(
        &plan,
        &project,
        &enfait_core::Look::default(),
        &library,
        &work.path().join("downloads"),
        &|_| {},
    )
    .await
    .unwrap();
    assert!(gathered.assets.is_empty() && gathered.missing.is_empty());

    let edits = magic::edits(&project, &transcript, &plan, &gathered, true, 1);
    let mut history = History::new();
    history.apply(&mut project, Edit::Batch { edits }).unwrap();
    let b = project.blocks();
    assert!(b[0].description.contains("train"));
    assert_eq!(b[0].moments[0].word, "part");
    assert_eq!(b[0].transition_after, enfait_core::Transition::Auto);
    assert_eq!(b[1].description, "face caméra");
    history.undo(&mut project).unwrap();
    assert_eq!(project, before);
}
