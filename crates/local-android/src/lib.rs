use base64::{engine::general_purpose::STANDARD, Engine as _};
use jni::{
    objects::{JByteArray, JClass, JString},
    sys::{jboolean, jlong, jstring, JNI_FALSE, JNI_TRUE},
    JNIEnv,
};
use local_core::{
    protocol::{ScreenCapabilities, ScreenCodec, ScreenFrameHeader, ScreenMediaCapabilities, ScreenOffer},
    Config, Node, ScreenVideoReceiver, ScreenVideoSender,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static NODE: Mutex<Option<Arc<Node>>> = Mutex::new(None);
static SCREEN_RECEIVERS: OnceLock<Mutex<HashMap<String, ScreenVideoReceiver>>> = OnceLock::new();
static SCREEN_SENDERS: OnceLock<Mutex<HashMap<String, ScreenVideoSender>>> = OnceLock::new();
fn screen_receivers() -> &'static Mutex<HashMap<String, ScreenVideoReceiver>> {
    SCREEN_RECEIVERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn screen_senders() -> &'static Mutex<HashMap<String, ScreenVideoSender>> {
    SCREEN_SENDERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("Tokio runtime"))
}
fn output(env: &mut JNIEnv<'_>, value: Value) -> jstring {
    env.new_string(value.to_string())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
fn text(env: &mut JNIEnv<'_>, value: JString<'_>) -> Result<String, String> {
    env.get_string(&value)
        .map(|v| v.into())
        .map_err(|e| e.to_string())
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_start(
    mut env: JNIEnv,
    _: JClass,
    data: JString,
    received: JString,
    name: JString,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut node = NODE.lock().map_err(|e| e.to_string())?;
        if node.is_some() {
            return Ok(json!({}));
        }
        let config = Config {
            data_dir: PathBuf::from(text(&mut env, data)?),
            receive_dir: PathBuf::from(text(&mut env, received)?),
            name: text(&mut env, name)?,
            bind: "0.0.0.0:53319".parse().unwrap(),
            discovery: true,
        };
        let started = runtime()
            .block_on(Node::start(config))
            .map_err(|e| e.to_string())?;
        *node = Some(started);
        Ok::<Value, String>(json!({}))
    }));
    output(
        &mut env,
        match result {
            Ok(Ok(data)) => json!({"ok":true,"data":data}),
            Ok(Err(e)) => json!({"ok":false,"error":e}),
            Err(_) => json!({"ok":false,"error":"Native startup failed"}),
        },
    )
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_command(
    mut env: JNIEnv,
    _: JClass,
    request: JString,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let value: Value =
            serde_json::from_str(&text(&mut env, request)?).map_err(|e| e.to_string())?;
        let node = NODE
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or("LoCAL is starting")?;
        match value.get("op").and_then(Value::as_str) {
            Some("enable_screen_view") => {
                node.set_screen_capabilities(Some(ScreenCapabilities {
                    encode: Some(ScreenMediaCapabilities {
                        codecs: vec![ScreenCodec::H264],
                        max_width: 1280,
                        max_height: 720,
                        max_fps: 30,
                    }),
                    decode: Some(ScreenMediaCapabilities {
                        codecs: vec![ScreenCodec::H264],
                        max_width: 1920,
                        max_height: 1080,
                        max_fps: 30,
                    }),
                    control_target: false,
                    system_audio_capture: false,
                    system_audio_playback: false,
                }))
                .map_err(|e| e.to_string())?;
                Ok(json!({"enabled":true}))
            }
            Some("watch_screen") => {
                let id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Missing screen session id")?;
                let receiver = node
                    .take_screen_video_receiver(id)
                    .map_err(|e| e.to_string())?;
                screen_receivers()
                    .lock()
                    .map_err(|e| e.to_string())?
                    .insert(id.into(), receiver);
                Ok(json!({"watching":true}))
            }
            Some("poll_screen_frame") => {
                let id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Missing screen session id")?;
                let mut receiver = screen_receivers()
                    .lock()
                    .map_err(|e| e.to_string())?
                    .remove(id)
                    .ok_or("Screen viewer is not active")?;
                let packet = runtime().block_on(async {
                    tokio::time::timeout(std::time::Duration::from_millis(120), receiver.recv())
                        .await
                });
                match packet {
                    Ok(Some(packet)) => {
                        screen_receivers()
                            .lock()
                            .map_err(|e| e.to_string())?
                            .insert(id.into(), receiver);
                        Ok(json!({
                            "frame":{
                                "sequence":packet.header.sequence,
                                "timestamp_us":packet.header.timestamp_us,
                                "keyframe":packet.header.keyframe,
                                "data":STANDARD.encode(packet.data)
                            }
                        }))
                    }
                    Err(_) => {
                        screen_receivers()
                            .lock()
                            .map_err(|e| e.to_string())?
                            .insert(id.into(), receiver);
                        Ok(json!({"frame":null}))
                    }
                    Ok(None) => Ok(json!({"ended":true,"frame":null})),
                }
            }
            Some("android_share_screen") => {
                let peer_id = value.get("peer_id").and_then(Value::as_str).ok_or("Missing peer_id")?;
                let width = value.get("width").and_then(Value::as_u64).ok_or("Missing width")? as u32;
                let height = value.get("height").and_then(Value::as_u64).ok_or("Missing height")? as u32;
                let fps = value.get("fps").and_then(Value::as_u64).unwrap_or(15).clamp(1, 30) as u16;
                let id = value.get("id").and_then(Value::as_str).ok_or("Missing screen session id")?.to_owned();
                let offer = ScreenOffer {
                    id: id.clone(),
                    resource: "screen/display/display-0".into(),
                    codec: ScreenCodec::H264,
                    width,
                    height,
                    fps,
                    system_audio: false,
                    control: false,
                };
                let accepted = runtime().block_on(node.offer_screen(peer_id, offer)).map_err(|e| e.to_string())?;
                if accepted {
                    let sender = runtime().block_on(node.open_screen_video(&id)).map_err(|e| e.to_string())?;
                    screen_senders().lock().map_err(|e| e.to_string())?.insert(id.clone(), sender);
                }
                Ok(json!({"id":id,"accepted":accepted,"width":width,"height":height,"fps":fps}))
            }
            Some("stop_screen") => {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    screen_receivers()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .remove(id);
                    screen_senders()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .remove(id);
                }
                runtime()
                    .block_on(node.command(value))
                    .map_err(|e| e.to_string())
            }
            _ => runtime()
                .block_on(node.command(value))
                .map_err(|e| e.to_string()),
        }
    }));
    output(
        &mut env,
        match result {
            Ok(Ok(data)) => json!({"ok":true,"data":data}),
            Ok(Err(e)) => json!({"ok":false,"error":e}),
            Err(_) => json!({"ok":false,"error":"Native operation failed"}),
        },
    )
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_pushScreenFrame(
    mut env: JNIEnv,
    _: JClass,
    id: JString,
    sequence: jlong,
    timestamp_us: jlong,
    keyframe: jboolean,
    data: JByteArray,
) -> jboolean {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = text(&mut env, id)?;
        let bytes = env.convert_byte_array(data).map_err(|e| e.to_string())?;
        let node = NODE.lock().map_err(|e| e.to_string())?.clone().ok_or("LoCAL is starting")?;
        let active = node.screen_sessions().into_iter().any(|s| s.id == id && s.direction == "out" && s.status == "active");
        if !active { return Ok::<bool, String>(false); }
        let mut sender = screen_senders().lock().map_err(|e| e.to_string())?.remove(&id).ok_or("Screen sender is not active")?;
        let header = ScreenFrameHeader {
            sequence: sequence.max(0) as u64,
            timestamp_us: timestamp_us.max(0) as u64,
            keyframe: keyframe != JNI_FALSE,
            payload_len: bytes.len() as u32,
        };
        let sent = runtime().block_on(sender.send_frame(header, &bytes));
        if sent.is_ok() {
            screen_senders().lock().map_err(|e| e.to_string())?.insert(id, sender);
            Ok(true)
        } else {
            Ok(false)
        }
    }));
    match result {
        Ok(Ok(true)) => JNI_TRUE,
        _ => JNI_FALSE,
    }
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_stop(_: JNIEnv, _: JClass) {
    if let Ok(mut receivers) = screen_receivers().lock() {
        receivers.clear();
    }
    if let Ok(mut senders) = screen_senders().lock() {
        senders.clear();
    }
    if let Ok(mut node) = NODE.lock() {
        if let Some(node) = node.take() {
            node.stop();
        }
    }
}
