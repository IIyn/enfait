//! The MCP server on a real session: protocol through standard input and
//! output (simulated), tools changing the files, generation requests
//! dropped for the app.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use enfait_core::{
    Asset, AssetKind, Block, BlockKind, Finish, Flicks, Look, Project, ProjectSettings, TimeRange,
    Transcript, Word,
};
use enfait_mcp::bridge::{RequestKind, take_requests};
use enfait_mcp::{Server, Tools};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

fn secs(s: f64) -> Flicks {
    #[allow(clippy::cast_possible_truncation)]
    Flicks::from_millis((s * 1000.0) as i64).unwrap()
}

fn range(a: f64, b: f64) -> TimeRange {
    TimeRange::new(secs(a), secs(b)).unwrap()
}

/// Session: two blocks, the voice "Bonjour à tous. Voici l'aiguillage.".
fn session() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let blocks = vec![
        Block::new(range(0.0, 2.0), BlockKind::Animation),
        Block::new(range(2.0, 5.0), BlockKind::Animation),
    ];
    let project = Project::with_blocks("essai", ProjectSettings::default(), blocks).unwrap();
    std::fs::write(d.join("project.json"), project.to_json().unwrap()).unwrap();
    let words = [
        ("Bonjour", 0.1, 0.6),
        ("à", 0.7, 0.8),
        ("tous.", 0.9, 1.4),
        ("Voici", 2.2, 2.6),
        ("l'aiguillage.", 2.7, 3.6),
    ];
    let transcript = Transcript {
        language: "fr".into(),
        media_duration: secs(5.0),
        words: words
            .iter()
            .map(|(t, a, b)| Word {
                text: (*t).into(),
                range: range(*a, *b),
                confidence: 1.0,
            })
            .collect(),
    };
    std::fs::write(
        d.join("montage.json"),
        serde_json::to_vec(&json!({ "key": "k", "transcript": transcript })).unwrap(),
    )
    .unwrap();
    let look = Look {
        assets: vec![Asset {
            file: "whoosh.wav".into(),
            kind: AssetKind::Audio,
            name: "Whoosh".into(),
            description: String::new(),
            origin: None,
        }],
        ..Look::default()
    };
    std::fs::write(d.join("look.json"), serde_json::to_vec(&look).unwrap()).unwrap();
    dir
}

async fn call(server: &mut Server, id: u64, name: &str, args: Value) -> (String, bool) {
    let reply = server
        .handle(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args },
        }))
        .await
        .unwrap();
    let result = &reply["result"];
    (
        result["content"][0]["text"].as_str().unwrap().to_owned(),
        result["isError"].as_bool().unwrap(),
    )
}

#[tokio::test]
async fn tools_edit_the_session() {
    let dir = session();
    let d = dir.path();
    let mut server = Server::new(Tools::new(d.to_path_buf()));

    let (text, err) = call(&mut server, 1, "storyboard", json!({})).await;
    assert!(!err);
    assert!(
        text.contains("## Block 2") && text.contains("“Voici l'aiguillage.”"),
        "{text}"
    );
    assert!(text.contains("closed"), "no app open");

    let (text, err) = call(
        &mut server,
        2,
        "edit_block",
        json!({ "block": 2, "description": "Un aiguillage en gravure", "transition": "auto" }),
    )
    .await;
    assert!(!err, "{text}");
    let (text, err) = call(
        &mut server,
        3,
        "add_moment",
        json!({ "block": "2", "word": "aiguillage", "note": "il bascule" }),
    )
    .await;
    assert!(!err, "{text}");
    assert!(text.contains("l'aiguillage."), "{text}");
    let project =
        Project::from_json(&std::fs::read_to_string(d.join("project.json")).unwrap()).unwrap();
    let b = &project.blocks()[1];
    assert_eq!(b.description, "Un aiguillage en gravure");
    assert_eq!(b.moments[0].note, "il bascule");
    assert_eq!(b.moments[0].at, secs(2.7));

    // Readable errors.
    let (text, err) = call(
        &mut server,
        4,
        "edit_block",
        json!({ "block": 9, "type": "rush" }),
    )
    .await;
    assert!(err && text.contains("no block 9"), "{text}");
    let (text, err) = call(
        &mut server,
        5,
        "add_moment",
        json!({ "block": 1, "word": "train", "note": "x" }),
    )
    .await;
    assert!(err && text.contains("Words: Bonjour à tous."), "{text}");

    // Finish: a whoosh just before block 2, ducking set.
    let (text, err) = call(
        &mut server,
        6,
        "add_sfx",
        json!({ "file": "whoosh.wav", "block": 2, "offset": -0.25 }),
    )
    .await;
    assert!(!err, "{text}");
    let (_, err) = call(&mut server, 7, "set_ducking", json!({ "depth_db": -18 })).await;
    assert!(!err);
    let finish: Finish =
        serde_json::from_slice(&std::fs::read(d.join("finish.json")).unwrap()).unwrap();
    assert_eq!(finish.sfx.len(), 1);
    assert_eq!(finish.ducking.depth_db, -18.0);
    let (text, _) = call(&mut server, 8, "finish", json!({})).await;
    assert!(text.contains("block 2 -0.25 s (0:01.8)"), "{text}");
    let (text, err) = call(&mut server, 9, "add_music", json!({ "file": "absent.mp3" })).await;
    assert!(err && text.contains("not in the library"), "{text}");

    // Generation: requests dropped for the app.
    let (text, err) = call(&mut server, 10, "generate", json!({ "all": true })).await;
    assert!(!err, "{text}");
    assert!(text.contains("block 1") && !text.contains("transition after block 2"));
    let requests = take_requests(d);
    assert_eq!(
        requests.len(),
        2,
        "two animations (block 2 has an auto transition, but it is the last one)"
    );
    assert!(requests.iter().all(|r| r.kind == RequestKind::Animation));
}

/// The whole protocol through streams, as on standard input and output.
#[tokio::test]
async fn speaks_mcp_over_streams() {
    let dir = session();
    let (client, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let path = dir.path().to_path_buf();
    let task = tokio::spawn(async move {
        Server::new(Tools::new(path))
            .serve(BufReader::new(server_read), server_write)
            .await
    });
    let (client_read, mut client_write) = tokio::io::split(client);
    let mut lines = BufReader::new(client_read).lines();
    let mut send = async |m: Value| {
        client_write
            .write_all(format!("{m}\n").as_bytes())
            .await
            .unwrap();
    };
    send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                 "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "essai", "version": "1" } } }))
        .await;
    send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await;
    send(json!({ "jsonrpc": "2.0", "id": 3, "method": "whatever" })).await;
    send(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "credits", "arguments": {} } })).await;
    // End of the input: the server stops after handling everything.
    client_write.shutdown().await.unwrap();

    let mut replies = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        replies.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    task.await.unwrap().unwrap();
    assert_eq!(replies.len(), 4, "no answer to a notification");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "enfait");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 15);
    assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
    assert_eq!(replies[2]["error"]["code"], -32601);
    assert!(
        replies[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no credits")
    );
}
