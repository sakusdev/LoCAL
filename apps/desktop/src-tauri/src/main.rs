#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use local_core::{
    protocol::{ScreenCapabilities, ScreenOffer, ScreenSignal},
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
    sync::{mpsc, Arc},
    time::Duration,
};
use tauri::{Manager, State};
use tokio::sync::{broadcast, watch};

struct AppState {
    node: Arc<Node>,
    screen: Arc<ScreenRuntime>,
}

struct ScreenRuntime {
    backend: Option<Arc<dyn EncodedCaptureBackend>>,
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

        if let Some(backend) = &backend {
            let _ = node.set_screen_capabilities(Some(source_capabilities(backend.as_ref())));
        }
        Self { backend }
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

        let sender = node
            .open_screen_video(&id)
            .await
            .map_err(|error| error.to_string())?;
        let signals = node
            .subscribe_screen_signals(&id)
            .map_err(|error| error.to_string())?;
        let capture = backend
            .start(&source.id, &profile)
            .map_err(|error| error.to_string())?;
        spawn_screen_pipeline(node, id.clone(), sender, signals, capture);

        Ok(json!({
            "id": id,
            "accepted": true,
            "profile": profile,
        }))
    }
}

fn run_capture_loop(
    mut capture: Box<dyn EncodedCaptureSession>,
    control_rx: mpsc::Receiver<ScreenSignal>,
    latest_tx: watch::Sender<Option<EncodedVideoFrame>>,
) -> Result<(), String> {
    loop {
        loop {
            match control_rx.try_recv() {
                Ok(ScreenSignal::Stop) => {
                    capture.stop().map_err(|error| error.to_string())?;
                    return Ok(());
                }
                Ok(ScreenSignal::RequestKeyframe) => capture
                    .request_keyframe()
                    .map_err(|error| error.to_string())?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = capture.stop();
                    return Ok(());
                }
            }
        }

        match capture
            .poll_frame(Duration::from_millis(100))
            .map_err(|error| error.to_string())?
        {
            CapturePoll::Frame(frame) => {
                latest_tx.send_replace(Some(frame));
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
    let (latest_tx, mut latest_rx) = watch::channel::<Option<EncodedVideoFrame>>(None);
    let (control_tx, control_rx) = mpsc::channel::<ScreenSignal>();
    let mut capture_task =
        tokio::task::spawn_blocking(move || run_capture_loop(capture, control_rx, latest_tx));

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
                            let _ = control_tx.send(ScreenSignal::Stop);
                            break;
                        }
                        Ok(ScreenSignal::RequestKeyframe) => {
                            if control_tx.send(ScreenSignal::RequestKeyframe).is_err() {
                                break;
                            }
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

        let _ = control_tx.send(ScreenSignal::Stop);
        let _ = sender.finish();
        if !remote_stopped {
            let _ = node.stop_screen(&id).await;
        }
    });
}

#[tauri::command]
async fn local_command(state: State<'_, AppState>, request: Value) -> Result<Value, String> {
    match request.get("op").and_then(Value::as_str) {
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
