use base64::{engine::general_purpose::STANDARD, Engine as _};
use jni::{
    objects::{JByteArray, JClass, JString},
    sys::{jboolean, jint, jlong, jstring, JNI_FALSE, JNI_TRUE},
    JNIEnv,
};
use local_core::{
    protocol::{
        ScreenCapabilities, ScreenCodec, ScreenFrameHeader, ScreenMediaCapabilities, ScreenOffer,
        ScreenSignal,
    },
    Config, Node, ScreenVideoReceiver, ScreenVideoSender,
};
use local_screen::{negotiate, ScreenRequest};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use tokio::sync::{broadcast, mpsc};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static NODE: Mutex<Option<Arc<Node>>> = Mutex::new(None);
static SCREEN_RECEIVERS: OnceLock<Mutex<HashMap<String, ScreenVideoReceiver>>> = OnceLock::new();
static OUTGOING_SCREENS: OnceLock<Mutex<HashMap<String, AndroidScreenPipe>>> = OnceLock::new();

const SCREEN_CONTROL_KEYFRAME: u8 = 1;
const SCREEN_CONTROL_STOP: u8 = 2;

struct AndroidFrame {
    header: ScreenFrameHeader,
    data: Vec<u8>,
}

struct AndroidScreenPipe {
    frames: mpsc::Sender<AndroidFrame>,
    control: Arc<AtomicU8>,
}

fn screen_receivers() -> &'static Mutex<HashMap<String, ScreenVideoReceiver>> {
    SCREEN_RECEIVERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn outgoing_screens() -> &'static Mutex<HashMap<String, AndroidScreenPipe>> {
    OUTGOING_SCREENS.get_or_init(|| Mutex::new(HashMap::new()))
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

fn android_screen_capabilities() -> ScreenCapabilities {
    ScreenCapabilities {
        encode: Some(ScreenMediaCapabilities {
            codecs: vec![ScreenCodec::H264],
            max_width: 1920,
            max_height: 1080,
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
    }
}

fn positive_u32(value: &Value, name: &str, default: u32) -> Result<u32, String> {
    let raw = value
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(default));
    let value = u32::try_from(raw).map_err(|_| format!("{name} is too large"))?;
    if value == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(value)
}

fn fit_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let (fitted_width, fitted_height) =
        if u64::from(width) * u64::from(max_height) > u64::from(height) * u64::from(max_width) {
            (
                max_width,
                (u64::from(height) * u64::from(max_width) / u64::from(width)) as u32,
            )
        } else {
            (
                (u64::from(width) * u64::from(max_height) / u64::from(height)) as u32,
                max_height,
            )
        };
    ((fitted_width.max(2) & !1), (fitted_height.max(2) & !1))
}

fn spawn_android_screen_sender(
    node: Arc<Node>,
    id: String,
    mut sender: ScreenVideoSender,
    mut signals: broadcast::Receiver<ScreenSignal>,
    mut frames: mpsc::Receiver<AndroidFrame>,
    control: Arc<AtomicU8>,
) {
    tokio::spawn(async move {
        let mut remote_stopped = false;
        loop {
            tokio::select! {
                signal = signals.recv() => match signal {
                    Ok(ScreenSignal::Stop) => {
                        remote_stopped = true;
                        control.fetch_or(SCREEN_CONTROL_STOP, Ordering::Release);
                        break;
                    }
                    Ok(ScreenSignal::RequestKeyframe) => {
                        control.fetch_or(SCREEN_CONTROL_KEYFRAME, Ordering::Release);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                frame = frames.recv() => {
                    let Some(frame) = frame else {
                        break;
                    };
                    if sender.send_frame(frame.header, &frame.data).await.is_err() {
                        break;
                    }
                }
            }
        }
        control.fetch_or(SCREEN_CONTROL_STOP, Ordering::Release);
        let _ = sender.finish();
        if let Ok(mut screens) = outgoing_screens().lock() {
            screens.remove(&id);
        }
        if !remote_stopped {
            let _ = node.stop_screen(&id).await;
        }
    });
}

async fn prepare_android_screen(node: Arc<Node>, value: &Value) -> Result<Value, String> {
    let peer_id = value
        .get("peer_id")
        .and_then(Value::as_str)
        .ok_or("Missing peer_id")?;
    if !outgoing_screens()
        .lock()
        .map_err(|e| e.to_string())?
        .is_empty()
    {
        return Err("Android can share one screen at a time".into());
    }
    let source_width = positive_u32(value, "source_width", 1080)?;
    let source_height = positive_u32(value, "source_height", 1920)?;
    let requested_width = positive_u32(value, "max_width", 1280)?.min(1920);
    let requested_height = positive_u32(value, "max_height", 720)?.min(1080);
    let (max_width, max_height) = fit_dimensions(
        source_width,
        source_height,
        requested_width,
        requested_height,
    );
    let max_fps = u16::try_from(positive_u32(value, "max_fps", 15)?.min(30))
        .map_err(|error| error.to_string())?;
    let source = android_screen_capabilities();
    let viewer = node
        .peer_screen_capabilities(peer_id)
        .map_err(|error| error.to_string())?;
    let profile = negotiate(
        &source,
        &viewer,
        &ScreenRequest {
            max_width,
            max_height,
            max_fps,
            system_audio: false,
            control: false,
        },
    )
    .map_err(|error| error.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let offer = ScreenOffer {
        id: id.clone(),
        resource: "screen/display/display-0".into(),
        codec: profile.codec,
        width: profile.width,
        height: profile.height,
        fps: profile.fps,
        system_audio: false,
        control: false,
    };
    let accepted = node
        .offer_screen(peer_id, offer)
        .await
        .map_err(|error| error.to_string())?;
    if !accepted {
        return Ok(json!({"id":id,"accepted":false,"profile":profile}));
    }
    let signals = node
        .subscribe_screen_signals(&id)
        .map_err(|error| error.to_string())?;
    let sender = node
        .open_screen_video(&id)
        .await
        .map_err(|error| error.to_string())?;
    let (frame_tx, frame_rx) = mpsc::channel(2);
    let control = Arc::new(AtomicU8::new(0));
    outgoing_screens()
        .lock()
        .map_err(|e| e.to_string())?
        .insert(
            id.clone(),
            AndroidScreenPipe {
                frames: frame_tx,
                control: control.clone(),
            },
        );
    spawn_android_screen_sender(node, id.clone(), sender, signals, frame_rx, control);
    Ok(json!({"id":id,"accepted":true,"profile":profile}))
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
                node.set_screen_capabilities(Some(android_screen_capabilities()))
                    .map_err(|e| e.to_string())?;
                Ok(json!({"enabled":true,"sharing":true}))
            }
            Some("share_screen") => runtime().block_on(prepare_android_screen(node, &value)),
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
            Some("stop_screen") => {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    screen_receivers()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .remove(id);
                    if let Some(screen) = outgoing_screens()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .get(id)
                    {
                        screen
                            .control
                            .fetch_or(SCREEN_CONTROL_STOP, Ordering::Release);
                    }
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
    data: JByteArray,
    sequence: jlong,
    timestamp_us: jlong,
    keyframe: jboolean,
) -> jboolean {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if sequence < 0 || timestamp_us < 0 {
            return false;
        }
        let Ok(id) = text(&mut env, id) else {
            return false;
        };
        let Ok(data) = env.convert_byte_array(&data) else {
            return false;
        };
        if data.is_empty() || data.len() > local_core::protocol::MAX_SCREEN_FRAME {
            return false;
        }
        let Ok(screens) = outgoing_screens().lock() else {
            return false;
        };
        let Some(screen) = screens.get(&id) else {
            return false;
        };
        screen
            .frames
            .try_send(AndroidFrame {
                header: ScreenFrameHeader {
                    sequence: sequence as u64,
                    timestamp_us: timestamp_us as u64,
                    keyframe: keyframe != JNI_FALSE,
                    payload_len: data.len() as u32,
                },
                data,
            })
            .is_ok()
    }));
    if result.unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_pollScreenControl(
    mut env: JNIEnv,
    _: JClass,
    id: JString,
) -> jint {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(id) = text(&mut env, id) else {
            return i32::from(SCREEN_CONTROL_STOP);
        };
        let Ok(screens) = outgoing_screens().lock() else {
            return i32::from(SCREEN_CONTROL_STOP);
        };
        let Some(screen) = screens.get(&id) else {
            return i32::from(SCREEN_CONTROL_STOP);
        };
        let value = screen.control.swap(0, Ordering::AcqRel);
        if value & SCREEN_CONTROL_STOP != 0 {
            screen.control.store(SCREEN_CONTROL_STOP, Ordering::Release);
        }
        i32::from(value)
    }))
    .unwrap_or(i32::from(SCREEN_CONTROL_STOP))
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_stop(_: JNIEnv, _: JClass) {
    if let Ok(mut receivers) = screen_receivers().lock() {
        receivers.clear();
    }
    if let Ok(mut screens) = outgoing_screens().lock() {
        for screen in screens.values() {
            screen
                .control
                .fetch_or(SCREEN_CONTROL_STOP, Ordering::Release);
        }
        screens.clear();
    }
    if let Ok(mut node) = NODE.lock() {
        if let Some(node) = node.take() {
            node.stop();
        }
    }
}
