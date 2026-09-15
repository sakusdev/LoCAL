use local_core::{Config,Node};
use serde_json::{json,Value};
use std::{path::Path,time::Duration};

async fn start(path: &Path,name: &str) -> std::sync::Arc<Node> {
    Node::start(Config{data_dir:path.join("data"),receive_dir:path.join("received"),name:name.into(),bind:"127.0.0.1:0".parse().unwrap(),discovery:false}).await.unwrap()
}
async fn until(node: &Node, predicate: impl Fn(&Value)->bool) -> Value {
    for _ in 0..200 { let s=node.snapshot().unwrap(); if predicate(&s) {return s;} tokio::time::sleep(Duration::from_millis(25)).await; }
    panic!("condition timed out: {}",node.snapshot().unwrap())
}
async fn pair(a: &std::sync::Arc<Node>,b: &std::sync::Arc<Node>) {
    a.connect(&b.address().unwrap().to_string(),Some(&b.id)).await.unwrap();
    let av=until(a,|s|s["peers"][0]["code"].is_string()).await;
    let bv=until(b,|s|s["peers"][0]["code"].is_string()).await;
    let code=av["peers"][0]["code"].as_str().unwrap();
    assert_eq!(code,bv["peers"][0]["code"].as_str().unwrap());
    assert!(a.send_text(&b.id,"blocked".into(),"mesh.text".into()).await.is_err());
    assert!(a.confirm(&b.id,"wrong").await.is_err());
    a.confirm(&b.id,code).await.unwrap();
    assert!(a.send_text(&b.id,"still blocked".into(),"mesh.text".into()).await.is_err());
    b.confirm(&a.id,code).await.unwrap();
    until(a,|s|s["peers"][0]["ready"]==true).await;
    until(b,|s|s["peers"][0]["ready"]==true).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn two_real_quic_peers_pair_message_file_and_revoke() {
    let temp=tempfile::tempdir().unwrap();
    let a=start(&temp.path().join("a"),"Alice").await;
    let b=start(&temp.path().join("b"),"Bob").await;
    pair(&a,&b).await;
    let text="こんにちは 👋\nPrivate LAN message";
    a.send_text(&b.id,text.into(),"mesh.text".into()).await.unwrap();
    b.send_text(&a.id,"clipboard example".into(),"mesh.clipboard".into()).await.unwrap();
    assert_eq!(b.snapshot().unwrap()["messages"][0]["text"],text);
    let bytes:Vec<u8>=(0..2_500_007).map(|n|(n%251) as u8).collect();
    let source=temp.path().join("テスト.bin"); std::fs::write(&source,&bytes).unwrap();
    let send_id=a.send_file(&b.id,source.clone()).await.unwrap();
    let incoming=until(&b,|s|s["transfers"][0]["status"]=="offered").await;
    b.decide_file(incoming["transfers"][0]["id"].as_str().unwrap(),true).unwrap();
    let state=until(&b,|s|s["transfers"][0]["status"]=="completed").await;
    let path=state["transfers"][0]["path"].as_str().unwrap();
    assert_eq!(std::fs::read(path).unwrap(),bytes);
    until(&a,|s|s["transfers"].as_array().unwrap().iter().any(|t|t["id"]==send_id && t["status"]=="completed")).await;
    // Refusal must not create another visible received file.
    let rejected=a.send_file(&b.id,source).await.unwrap();
    let state=until(&b,|s|s["transfers"].as_array().unwrap().iter().any(|t|t["status"]=="offered")).await;
    let offer=state["transfers"].as_array().unwrap().iter().find(|t|t["status"]=="offered").unwrap();
    b.decide_file(offer["id"].as_str().unwrap(),false).unwrap();
    until(&a,|s|s["transfers"].as_array().unwrap().iter().any(|t|t["id"]==rejected && t["status"]=="failed")).await;
    assert_eq!(std::fs::read_dir(&b.receive_dir).unwrap().filter_map(Result::ok).filter(|e|e.path().is_file()).count(),1);
    b.forget(&a.id).unwrap();
    until(&a,|s|s["peers"][0]["connected"]==false).await;
    assert!(a.send_text(&b.id,"no".into(),"mesh.text".into()).await.is_err());
    assert!(b.snapshot().unwrap()["trusted"].as_array().unwrap().is_empty());
    a.stop();b.stop();
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn identity_pin_reconnect_and_restart_persistence() {
    let temp=tempfile::tempdir().unwrap();
    let a=start(&temp.path().join("a"),"Alice").await;
    let b=start(&temp.path().join("b"),"Bob").await;
    assert!(a.connect(&b.address().unwrap().to_string(),Some(&"0".repeat(64))).await.is_err());
    pair(&a,&b).await;
    a.send_text(&b.id,"persistent".into(),"mesh.text".into()).await.unwrap();
    a.disconnect(&b.id);
    until(&b,|s|s["peers"][0]["connected"]==false).await;
    a.connect(&b.address().unwrap().to_string(),Some(&b.id)).await.unwrap();
    until(&a,|s|s["peers"][0]["ready"]==true).await;
    until(&b,|s|s["peers"][0]["ready"]==true).await;
    let id=b.id.clone();
    a.stop(); b.stop(); drop(a); drop(b);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let b=start(&temp.path().join("b"),"Other name").await;
    assert_eq!(b.id,id);
    assert_eq!(b.snapshot().unwrap()["messages"][0]["text"],"persistent");
    assert_eq!(b.snapshot().unwrap()["trusted"].as_array().unwrap().len(),1);
    b.command(json!({"op":"clear_history"})).await.unwrap();
    b.stop();
}

#[test]
fn traversal_reserved_names_and_hashes_are_rejected() {
    use local_core::protocol::*;
    for name in ["../secret","a/b","a\\b","..","", "CON.txt","LPT1","NUL","trailing.","a\0b"] {assert!(validate_name(name).is_err(),"{name}");}
    for name in ["写真.png","hello world.txt",".gitignore"] {assert!(validate_name(name).is_ok(),"{name}");}
    assert!(!valid_hash("../../anything")); assert!(valid_hash(&"a".repeat(64)));
}

