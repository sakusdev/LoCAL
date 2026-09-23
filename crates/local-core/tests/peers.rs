use local_core::{Config, Node};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

async fn start(path: &Path, name: &str) -> std::sync::Arc<Node> {
    Node::start(Config {
        data_dir: path.join("data"),
        receive_dir: path.join("received"),
        name: name.into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        discovery: false,
    })
    .await
    .unwrap()
}
async fn until(node: &Node, predicate: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..200 {
        let s = node.snapshot().unwrap();
        if predicate(&s) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition timed out: {}", node.snapshot().unwrap())
}
async fn pair(a: &std::sync::Arc<Node>, b: &std::sync::Arc<Node>) {
    a.connect(&b.address().unwrap().to_string(), Some(&b.id))
        .await
        .unwrap();
    let av = until(a, |s| s["peers"][0]["code"].is_string()).await;
    let bv = until(b, |s| s["peers"][0]["code"].is_string()).await;
    let code = av["peers"][0]["code"].as_str().unwrap();
    assert_eq!(code, bv["peers"][0]["code"].as_str().unwrap());
    assert!(a
        .send_text(&b.id, "blocked".into(), "mesh.text".into())
        .await
        .is_err());
    assert!(a.confirm(&b.id, "wrong").await.is_err());
    a.confirm(&b.id, code).await.unwrap();
    assert!(a
        .send_text(&b.id, "still blocked".into(), "mesh.text".into())
        .await
        .is_err());
    b.confirm(&a.id, code).await.unwrap();
    until(a, |s| s["peers"][0]["ready"] == true).await;
    until(b, |s| s["peers"][0]["ready"] == true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_real_quic_peers_pair_message_file_and_revoke() {
    let temp = tempfile::tempdir().unwrap();
    let a = start(&temp.path().join("a"), "Alice").await;
    let b = start(&temp.path().join("b"), "Bob").await;
    pair(&a, &b).await;
    let text = "こんにちは 👋\nPrivate LAN message";
    a.send_text(&b.id, text.into(), "mesh.text".into())
        .await
        .unwrap();
    b.send_text(&a.id, "clipboard example".into(), "mesh.clipboard".into())
        .await
        .unwrap();
    assert_eq!(b.snapshot().unwrap()["messages"][0]["text"], text);
    let bytes: Vec<u8> = (0..2_500_007).map(|n| (n % 251) as u8).collect();
    let source = temp.path().join("テスト.bin");
    std::fs::write(&source, &bytes).unwrap();
    let send_id = a.send_file(&b.id, source.clone()).await.unwrap();
    let incoming = until(&b, |s| s["transfers"][0]["status"] == "offered").await;
    b.decide_file(incoming["transfers"][0]["id"].as_str().unwrap(), true)
        .unwrap();
    assert_ne!(b.snapshot().unwrap()["transfers"][0]["status"], "offered");
    let state = until(&b, |s| s["transfers"][0]["status"] == "completed").await;
    let path = state["transfers"][0]["path"].as_str().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    let saved = b.command(json!({"op":"received_files"})).await.unwrap();
    assert_eq!(saved["total"], 1);
    assert_eq!(saved["files"][0]["name"], "テスト.bin");
    assert_eq!(
        saved["files"][0]["path"],
        std::fs::canonicalize(path)
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    until(&a, |s| {
        s["transfers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == send_id && t["status"] == "completed")
    })
    .await;
    // Refusal must not create another visible received file.
    let rejected = a.send_file(&b.id, source).await.unwrap();
    let state = until(&b, |s| {
        s["transfers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["status"] == "offered")
    })
    .await;
    let offer = state["transfers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["status"] == "offered")
        .unwrap();
    b.decide_file(offer["id"].as_str().unwrap(), false).unwrap();
    until(&a, |s| {
        s["transfers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == rejected && t["status"] == "failed")
    })
    .await;
    assert_eq!(
        std::fs::read_dir(&b.receive_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .count(),
        1
    );
    b.forget(&a.id).unwrap();
    until(&a, |s| s["peers"][0]["connected"] == false).await;
    assert!(a
        .send_text(&b.id, "no".into(), "mesh.text".into())
        .await
        .is_err());
    assert!(b.snapshot().unwrap()["trusted"]
        .as_array()
        .unwrap()
        .is_empty());
    a.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identity_pin_reconnect_and_restart_persistence() {
    let temp = tempfile::tempdir().unwrap();
    let a = start(&temp.path().join("a"), "Alice").await;
    let b = start(&temp.path().join("b"), "Bob").await;
    assert!(a
        .connect(&b.address().unwrap().to_string(), Some(&"0".repeat(64)))
        .await
        .is_err());
    pair(&a, &b).await;
    a.send_text(&b.id, "persistent".into(), "mesh.text".into())
        .await
        .unwrap();
    a.disconnect(&b.id);
    until(&b, |s| s["peers"][0]["connected"] == false).await;
    a.connect(&b.address().unwrap().to_string(), Some(&b.id))
        .await
        .unwrap();
    until(&a, |s| s["peers"][0]["ready"] == true).await;
    until(&b, |s| s["peers"][0]["ready"] == true).await;
    let id = b.id.clone();
    a.stop();
    b.stop();
    drop(a);
    drop(b);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let b = start(&temp.path().join("b"), "Other name").await;
    assert_eq!(b.id, id);
    assert_eq!(b.snapshot().unwrap()["messages"][0]["text"], "persistent");
    assert_eq!(
        b.snapshot().unwrap()["trusted"].as_array().unwrap().len(),
        1
    );
    b.command(json!({"op":"clear_history"})).await.unwrap();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paired_peers_exchange_private_audio_call_signals() {
    let temp = tempfile::tempdir().unwrap();
    let caller = start(&temp.path().join("caller"), "Caller").await;
    let receiver = start(&temp.path().join("receiver"), "Receiver").await;
    caller.set_audio_enabled(true).unwrap();
    receiver.set_audio_enabled(true).unwrap();
    pair(&caller, &receiver).await;
    let call_id = uuid::Uuid::new_v4().to_string();
    caller
        .send_audio_signal(
            &receiver.id,
            &call_id,
            "offer",
            r#"{"type":"offer","sdp":"test-sdp"}"#,
        )
        .await
        .unwrap();
    let events = receiver
        .command(json!({"op":"audio_signals","after":0}))
        .await
        .unwrap();
    assert_eq!(events["events"][0]["call_id"], call_id);
    assert_eq!(events["events"][0]["peer_id"], caller.id);
    assert_eq!(events["events"][0]["kind"], "offer");
    let sequence = events["latest"].as_u64().unwrap();
    assert!(receiver
        .command(json!({"op":"audio_signals","after":sequence}))
        .await
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(caller
        .send_audio_signal(&receiver.id, "not-a-uuid", "offer", "{}")
        .await
        .is_err());
    assert!(caller
        .send_audio_signal(&receiver.id, &call_id, "offer", "not-json")
        .await
        .is_err());
    caller.stop();
    receiver.stop();
}

#[test]
fn traversal_reserved_names_and_hashes_are_rejected() {
    use local_core::protocol::*;
    for name in [
        "../secret",
        "a/b",
        "a\\b",
        "..",
        "",
        "CON.txt",
        "LPT1",
        "NUL",
        "trailing.",
        "a\0b",
    ] {
        assert!(validate_name(name).is_err(), "{name}");
    }
    for name in ["写真.png", "hello world.txt", ".gitignore"] {
        assert!(validate_name(name).is_ok(), "{name}");
    }
    assert!(!valid_hash("../../anything"));
    assert!(valid_hash(&"a".repeat(64)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_persisted_bytes_and_rejects_corrupt_partial() {
    let temp = tempfile::tempdir().unwrap();
    let a = start(&temp.path().join("a"), "Sender").await;
    let b = start(&temp.path().join("b"), "Receiver").await;
    pair(&a, &b).await;
    let bytes: Vec<u8> = (0..3_100_017).map(|n| (n % 251) as u8).collect();
    let source = temp.path().join("resume.bin");
    std::fs::write(&source, &bytes).unwrap();
    let dir = b.receive_dir.join(".partial");
    std::fs::create_dir_all(&dir).unwrap();
    let partial = dir.join(format!("{}-{}.part", a.id, blake3::hash(&bytes).to_hex()));
    for corrupt in [true, false] {
        let mut prefix = bytes[..1_048_576].to_vec();
        if corrupt {
            prefix[777] ^= 1;
        }
        std::fs::write(&partial, prefix).unwrap();
        let id = a.send_file(&b.id, source.clone()).await.unwrap();
        let state = until(&b, |s| {
            s["transfers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["status"] == "offered")
        })
        .await;
        let offer = state["transfers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["status"] == "offered")
            .unwrap();
        b.decide_file(offer["id"].as_str().unwrap(), true).unwrap();
        let state = until(&a, |s| {
            s["transfers"].as_array().unwrap().iter().any(|t| {
                t["id"] == id
                    && ["completed", "failed"].contains(&t["status"].as_str().unwrap_or(""))
            })
        })
        .await;
        let transfer = state["transfers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .unwrap();
        if corrupt {
            assert_eq!(transfer["status"], "failed");
            assert_eq!(std::fs::metadata(&partial).unwrap().len(), 0);
            assert_eq!(
                std::fs::read_dir(&b.receive_dir)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|e| e.path().is_file())
                    .count(),
                0
            );
        } else {
            assert_eq!(transfer["status"], "completed");
            let received = b.snapshot().unwrap()["transfers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["status"] == "completed")
                .unwrap()["path"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(std::fs::read(received).unwrap(), bytes);
            assert!(!partial.exists());
        }
    }
    a.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn received_files_survive_restart_with_bounded_pages() {
    let temp = tempfile::tempdir().unwrap();
    let node = start(temp.path(), "Receiver").await;
    for index in 0..53 {
        std::fs::write(
            node.receive_dir
                .join(format!("{}_写真-{index}.txt", uuid::Uuid::new_v4())),
            b"saved",
        )
        .unwrap();
    }
    std::fs::create_dir(node.receive_dir.join(".partial")).unwrap();
    std::fs::write(
        node.receive_dir.join(".partial/incomplete.part"),
        b"partial",
    )
    .unwrap();
    std::fs::write(node.receive_dir.join(".hidden"), b"hidden").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        temp.path().join("data/identity.json"),
        node.receive_dir.join("link.txt"),
    )
    .unwrap();
    let first = node.command(json!({"op":"received_files"})).await.unwrap();
    assert_eq!(first["total"], 53);
    assert_eq!(first["files"].as_array().unwrap().len(), 50);
    assert!(first["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| file["name"].as_str().unwrap().starts_with("写真-")));
    let last = node
        .command(json!({"op":"received_files","offset":50}))
        .await
        .unwrap();
    assert_eq!(last["files"].as_array().unwrap().len(), 3);
    let overflow = node
        .command(json!({"op":"received_files","offset":u64::MAX}))
        .await
        .unwrap();
    assert_eq!(overflow, last);
    node.stop();
    drop(node);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let node = start(temp.path(), "Receiver").await;
    assert!(node.snapshot().unwrap()["transfers"]
        .as_array()
        .unwrap()
        .is_empty());
    let restored = node.command(json!({"op":"received_files"})).await.unwrap();
    assert_eq!(restored, first);
    assert_eq!(
        std::fs::read(restored["files"][0]["path"].as_str().unwrap()).unwrap(),
        b"saved"
    );
    node.stop();
}
