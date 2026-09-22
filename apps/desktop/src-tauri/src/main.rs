#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use local_core::{
    protocol::{
        ScreenCapabilities, ScreenCodec, ScreenMediaCapabilities, ScreenOffer, ScreenSignal,
    },
    Config, Node, ScreenVideoSender,
};
#[cfg(target_os = "windows")]
use local_screen::windows::WindowsCaptureBackend;
use local_screen::{
    negotiate, CapturePoll, EncodedCaptureBackend, EncodedCaptureSession, EncodedVideoFrame,
    ScreenRequest,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, State};
use tokio::sync::{broadcast, watch};

struct AppState {
    node: Arc<Node>,
    screen: Arc<ScreenRuntime>,
}

struct ScreenRuntime {
    backend: Option<Arc<dyn EncodedCaptureBackend>>,
    view_enabled: AtomicBool,
}

fn source_capabilities(backend: &dyn EncodedCaptureBackend) -> ScreenCapabilities {
    ScreenCapabilities {
        encode: Some(backend.encode_capabilities()),
        decode: None,
        control_target: false,
        system_audio_capture: backend.system_audio_capture(),
        system_audio_playback: false,
    }
}

fn optional_u32(request: &Value, name: &str, default: u32) -> Result<u32, String> {
    let Some(value) = request.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .ok_or_else(|| format!("{name} must be a positive integer"))?;
    u32::try_from(value).map_err(|_| format!("{name} is too large"))
}

fn optional_u16(request: &Value, name: &str, default: u16) -> Result<u16, String> {
    let Some(value) = request.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .ok_or_else(|| format!("{name} must be a positive integer"))?;
    u16::try_from(value).map_err(|_| format!("{name} is too large"))
}

impl ScreenRuntime {
    fn new(node: &Arc<Node>) -> Self {
        #[cfg(target_os = "windows")]
        let backend: Option<Arc<dyn EncodedCaptureBackend>> = WindowsCaptureBackend::new()
            .ok()
            .map(|backend| Arc::new(backend) as Arc<dyn EncodedCaptureBackend>);

        #[cfg(not(target_os = "windows"))]
        let backend: Option<Arc<dyn EncodedCaptureBackend>> = None;

        let backend = backend.and_then(|backend| {
            node.set_screen_capabilities(Some(source_capabilities(backend.as_ref())))
                .ok()
                .map(|_| backend)
        });
        Self {
            backend,
            view_enabled: AtomicBool::new(false),
        }
    }

    fn enable_view(&self, node: &Node) -> Result<Value, String> {
        if !cfg!(target_os = "windows") {
            return Err("Screen viewing is currently supported on Windows only".into());
        }
        if self.view_enabled.load(Ordering::Acquire) {
            return Ok(json!({"enabled":true}));
        }
        let mut capabilities = self.backend.as_ref().map_or(
            ScreenCapabilities {
                encode: None,
                decode: None,
                control_target: false,
                system_audio_capture: false,
                system_audio_playback: false,
            },
            |backend| source_capabilities(backend.as_ref()),
        );
        capabilities.decode = Some(ScreenMediaCapabilities {
            codecs: vec![ScreenCodec::H264],
            max_width: 1920,
            max_height: 1080,
            max_fps: 30,
        });
        node.set_screen_capabilities(Some(capabilities))
            .map_err(|error| error.to_string())?;
        self.view_enabled.store(true, Ordering::Release);
        Ok(json!({"enabled":true}))
    }

    fn watch(&self, node: Arc<Node>, app: tauri::AppHandle, id: &str) -> Result<Value, String> {
        if !self.view_enabled.load(Ordering::Acquire) {
            return Err("Screen decoder is not enabled".into());
        }
        let mut receiver = node
            .take_screen_video_receiver(id)
            .map_err(|error| error.to_string())?;
        let id = id.to_owned();
        tokio::spawn(async move {
            let mut waiting_for_keyframe = true;
            let mut last_sequence: Option<u64> = None;
            let mut last_request: Option<Instant> = None;
            while let Some(packet) = receiver.recv().await {
                if last_sequence.is_some_and(|last| packet.header.sequence != last.saturating_add(1)) {
                    waiting_for_keyframe = true;
                }
                last_sequence = Some(packet.header.sequence);
                if packet.header.keyframe {
                    waiting_for_keyframe = false;
                } else if waiting_for_keyframe {
                    if last_request.is_none_or(|time| time.elapsed() >= Duration::from_secs(1)) {
                        let _ = node.request_screen_keyframe(&id).await;
                        last_request = Some(Instant::now());
                    }
                    continue;
                }
                if app
                    .emit(
                        "local-screen-frame",
                        json!({
                            "id": id,
                            "sequence": packet.header.sequence,
                            "timestamp_us": packet.header.timestamp_us,
                            "keyframe": packet.header.keyframe,
                            "data": STANDARD.encode(&packet.data),
                        }),
                    )
                    .is_err()
                {
                    let _ = node.stop_screen(&id).await;
                    break;
                }
            }
            let _ = app.emit("local-screen-ended", json!({"id":id}));
        });
        Ok(json!({"watching":true}))
    }

    fn backend(&self) -> Result<Arc<dyn EncodedCaptureBackend>, String> {
        self.backend
            .clone()
            .ok_or_else(|| "Screen capture is not available on this desktop".into())
    }

    fn sources(&self) -> Result<Value, String> {
        let backend = self.backend()?;
        let sources = backend.sources().map_err(|error| error.to_string())?;
        Ok(json!({
            "backend": backend.backend_name(),
            "sources": sources,
        }))
    }

    async fn share(&self, node: Arc<Node>, request: &Value) -> Result<Value, String> {
        let peer_id = request
            .get("peer_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing peer_id".to_owned())?;
        let source_id = request
            .get("source_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing source_id".to_owned())?;
        let backend = self.backend()?;
        let source = backend
            .sources()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| "Capture source is no longer available".to_owned())?;
        source.validate().map_err(|error| error.to_string())?;

        let defaults = ScreenRequest::default();
        let screen_request = ScreenRequest {
            max_width: optional_u32(request, "max_width", defaults.max_width)?.min(source.width),
            max_height: optional_u32(request, "max_height", defaults.max_height)?
                .min(source.height),
            max_fps: optional_u16(request, "max_fps", defaults.max_fps)?,
            system_audio: request
                .get("system_audio")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            control: request
                .get("control")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let viewer = node
            .peer_screen_capabilities(peer_id)
            .map_err(|error| error.to_string())?;
        let profile = negotiate(
            &source_capabilities(backend.as_ref()),
            &viewer,
            &screen_request,
        )
        .map_err(|error| error.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let offer = ScreenOffer {
            id: id.clone(),
            resource: source.resource_path(),
            codec: profile.codec,
            width: profile.width,
            height: profile.height,
            fps: profile.fps,
            system_audio: profile.system_audio,
            control: profile.control,
        };

        let accepted = node
            .offer_screen(peer_id, offer)
            .await
            .map_err(|error| error.to_string())?;
        if !accepted {
            return Ok(json!({
                "id": id,
                "accepted": false,
                "profile": profile,
            }));
        }

        let signals = match node.subscribe_screen_signals(&id) {
            Ok(signals) => signals,
            Err(error) => {
                let _ = node.stop_screen(&id).await;
                return Err(error.to_string());
            }
        };
        let mut capture = match backend.start(&source.id, &profile) {
            Ok(capture) => capture,
            Err(error) => {
                let _ = node.stop_screen(&id).await;
                return Err(error.to_string());
            }
        };
        let sender = match node.open_screen_video(&id).await {
            Ok(sender) => sender,
            Err(error) => {
                let _ = capture.stop();
                let _ = node.stop_screen(&id).await;
                return Err(error.to_string());
            }
        };
        spawn_screen_pipeline(node, id.clone(), sender, signals, capture);

        Ok(json!({
            "id": id,
            "accepted": true,
            "profile": profile,
        }))
    }
}

struct CaptureControl {
    stop: AtomicBool,
    keyframe: AtomicBool,
}

impl CaptureControl {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            keyframe: AtomicBool::new(false),
        }
    }
}

fn run_capture_loop(
    mut capture: Box<dyn EncodedCaptureSession>,
    control: Arc<CaptureControl>,
    latest_tx: watch::Sender<Option<Arc<EncodedVideoFrame>>>,
) -> Result<(), String> {
    loop {
        if control.stop.load(Ordering::Acquire) {
            capture.stop().map_err(|error| error.to_string())?;
            return Ok(());
        }
        if control.keyframe.swap(false, Ordering::AcqRel) {
            capture
                .request_keyframe()
                .map_err(|error| error.to_string())?;
        }

        match capture
            .poll_frame(Duration::from_millis(100))
            .map_err(|error| error.to_string())?
        {
            CapturePoll::Frame(frame) => {
                latest_tx.send_replace(Some(Arc::new(frame)));
            }
            CapturePoll::Pending => {}
            CapturePoll::Ended => {
                let _ = capture.stop();
                return Ok(());
            }
        }
    }
}

fn spawn_screen_pipeline(
    node: Arc<Node>,
    id: String,
    mut sender: ScreenVideoSender,
    mut signals: broadcast::Receiver<ScreenSignal>,
    capture: Box<dyn EncodedCaptureSession>,
) {
    let (latest_tx, mut latest_rx) = watch::channel::<Option<Arc<EncodedVideoFrame>>>(None);
    let control = Arc::new(CaptureControl::new());
    let capture_control = control.clone();
    let mut capture_task =
        tokio::task::spawn_blocking(move || run_capture_loop(capture, capture_control, latest_tx));

    tokio::spawn(async move {
        let mut remote_stopped = false;
        loop {
            tokio::select! {
                capture_result = &mut capture_task => {
                    let _ = capture_result;
                    break;
                }
                signal = signals.recv() => {
                    match signal {
                        Ok(ScreenSignal::Stop) => {
                            remote_stopped = true;
                            control.stop.store(true, Ordering::Release);
                            break;
                        }
                        Ok(ScreenSignal::RequestKeyframe) => {
                            control.keyframe.store(true, Ordering::Release);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                changed = latest_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let frame = latest_rx.borrow_and_update().clone();
                    let Some(frame) = frame else {
                        continue;
                    };
                    let header = match frame.header() {
                        Ok(header) => header,
                        Err(_) => break,
                    };
                    if sender.send_frame(header, &frame.data).await.is_err() {
                        break;
                    }
                }
            }
        }

        control.stop.store(true, Ordering::Release);
        let _ = sender.finish();
        if !remote_stopped {
            let _ = node.stop_screen(&id).await;
        }
    });
}

#[tauri::command]
async fn local_command(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    request: Value,
) -> Result<Value, String> {
    match request.get("op").and_then(Value::as_str) {
        Some("enable_screen_view") => state.screen.enable_view(&state.node),
        Some("watch_screen") => state.screen.watch(
            state.node.clone(),
            app,
            request
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Missing screen session id")?,
        ),
        Some("screen_sources") => state.screen.sources(),
        Some("share_screen") => state.screen.share(state.node.clone(), &request).await,
        _ => state.node.command(request).await.map_err(|e| e.to_string()),
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let node = tauri::async_runtime::block_on(Node::start(Config::default()))?;
            let screen = Arc::new(ScreenRuntime::new(&node));
            app.manage(AppState { node, screen });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![local_command])
        .build(tauri::generate_context!())
        .expect("Cannot start LoCAL. Check whether another instance is running.");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle.state::<AppState>().node.stop();
        }
    });
}
