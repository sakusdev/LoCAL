use local_core::{
    protocol::{
        ScreenCapabilities, ScreenCodec, ScreenFrameHeader, ScreenMediaCapabilities, ScreenOffer,
        ScreenSignal,
    },
    Config, Node,
};
use serde_json::Value;
use std::{path::Path, sync::Arc, time::Duration};

async fn start(path: &Path, name: &str) -> Arc<Node> {
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
    for _ in 0..240 {
        let state = node.snapshot().unwrap();
        if predicate(&state) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition timed out: {}", node.snapshot().unwrap());
}

async fn pair(a: &Arc<Node>, b: &Arc<Node>) {
    a.connect(&b.address().unwrap().to_string(), Some(&b.id))
        .await
        .unwrap();
    let av = until(a, |s| s["peers"][0]["code"].is_string()).await;
    let bv = until(b, |s| s["peers"][0]["code"].is_string()).await;
    let code = av["peers"][0]["code"].as_str().unwrap();
    assert_eq!(code, bv["peers"][0]["code"].as_str().unwrap());
    a.confirm(&b.id, code).await.unwrap();
    b.confirm(&a.id, code).await.unwrap();
    until(a, |s| s["peers"][0]["ready"] == true).await;
    until(b, |s| s["peers"][0]["ready"] == true).await;
}

fn media(codecs: Vec<ScreenCodec>) -> ScreenMediaCapabilities {
    ScreenMediaCapabilities {
        codecs,
        max_width: 1920,
        max_height: 1080,
        max_fps: 60,
    }
}

fn source_capabilities() -> ScreenCapabilities {
    ScreenCapabilities {
        encode: Some(media(vec![ScreenCodec::H264, ScreenCodec::Av1])),
        decode: None,
        control_target: true,
        system_audio_capture: true,
        system_audio_playback: false,
    }
}

fn viewer_capabilities() -> ScreenCapabilities {
    ScreenCapabilities {
        encode: None,
        decode: Some(media(vec![ScreenCodec::H264, ScreenCodec::Vp9])),
        control_target: false,
        system_audio_capture: false,
        system_audio_playback: true,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn screen_offer_video_keyframe_and_stop_use_real_quic() {
    let temp = tempfile::tempdir().unwrap();
    let source = start(&temp.path().join("source"), "Source").await;
    let viewer = start(&temp.path().join("viewer"), "Viewer").await;
    source
        .set_screen_capabilities(Some(source_capabilities()))
        .unwrap();
    viewer
        .set_screen_capabilities(Some(viewer_capabilities()))
        .unwrap();
    pair(&source, &viewer).await;

    let source_state = source.snapshot().unwrap();
    assert!(source_state["peers"][0]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "screen.view"));
    let viewer_state = viewer.snapshot().unwrap();
    assert!(viewer_state["peers"][0]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "screen.share"));

    let offer = ScreenOffer {
        id: uuid::Uuid::new_v4().to_string(),
        resource: "screen/display/display-0".into(),
        codec: ScreenCodec::H264,
        width: 1280,
        height: 720,
        fps: 60,
        system_audio: true,
        control: true,
    };
    let offer_id = offer.id.clone();
    let source_for_offer = source.clone();
    let viewer_id = viewer.id.clone();
    let offer_task = tokio::spawn(async move {
        source_for_offer
            .offer_screen(&viewer_id, offer)
            .await
            .unwrap()
    });

    until(&viewer, |s| {
        s["screen_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == offer_id && session["status"] == "offered")
    })
    .await;
    viewer.decide_screen(&offer_id, true).unwrap();
    assert!(offer_task.await.unwrap());
    until(&source, |s| {
        s["screen_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == offer_id && session["status"] == "active")
    })
    .await;

    let mut signals = source.subscribe_screen_signals(&offer_id).unwrap();
    let mut video = viewer.take_screen_video_receiver(&offer_id).unwrap();
    let mut sender = source.open_screen_video(&offer_id).await.unwrap();
    let payload = b"encoded-h264-access-unit".to_vec();
    sender
        .send_frame(
            ScreenFrameHeader {
                sequence: 1,
                timestamp_us: 10_000,
                keyframe: true,
                payload_len: payload.len() as u32,
            },
            &payload,
        )
        .await
        .unwrap();
    let packet = tokio::time::timeout(Duration::from_secs(3), video.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(packet.header.sequence, 1);
    assert!(packet.header.keyframe);
    assert_eq!(packet.data, payload);

    viewer.request_screen_keyframe(&offer_id).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), signals.recv())
            .await
            .unwrap()
            .unwrap(),
        ScreenSignal::RequestKeyframe
    );

    viewer.stop_screen(&offer_id).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), signals.recv())
            .await
            .unwrap()
            .unwrap(),
        ScreenSignal::Stop
    );
    sender.finish().unwrap();
    until(&source, |s| {
        s["screen_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == offer_id && session["status"] == "stopped")
    })
    .await;
    until(&viewer, |s| {
        s["screen_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == offer_id && session["status"] == "stopped")
    })
    .await;

    source.stop();
    viewer.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_screen_offer_never_becomes_active() {
    let temp = tempfile::tempdir().unwrap();
    let source = start(&temp.path().join("source"), "Source").await;
    let viewer = start(&temp.path().join("viewer"), "Viewer").await;
    source
        .set_screen_capabilities(Some(source_capabilities()))
        .unwrap();
    viewer
        .set_screen_capabilities(Some(viewer_capabilities()))
        .unwrap();
    pair(&source, &viewer).await;

    let offer = ScreenOffer {
        id: uuid::Uuid::new_v4().to_string(),
        resource: "screen/display/display-0".into(),
        codec: ScreenCodec::H264,
        width: 1280,
        height: 720,
        fps: 30,
        system_audio: false,
        control: false,
    };
    let offer_id = offer.id.clone();
    let source_for_offer = source.clone();
    let viewer_id = viewer.id.clone();
    let offer_task = tokio::spawn(async move {
        source_for_offer
            .offer_screen(&viewer_id, offer)
            .await
            .unwrap()
    });
    until(&viewer, |s| {
        s["screen_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == offer_id && session["status"] == "offered")
    })
    .await;
    viewer.decide_screen(&offer_id, false).unwrap();
    assert!(!offer_task.await.unwrap());
    assert!(source.open_screen_video(&offer_id).await.is_err());

    source.stop();
    viewer.stop();
}
