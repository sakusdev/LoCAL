#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use local_core::{Config,Node};
use serde_json::Value;
use std::sync::Arc;
use tauri::{Manager,State};

struct AppState { node: Arc<Node> }

#[tauri::command]
async fn local_command(state: State<'_,AppState>, request: Value) -> Result<Value,String> {
    state.node.command(request).await.map_err(|e|e.to_string())
}

fn main() {
    let app=tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let node=tauri::async_runtime::block_on(Node::start(Config::default()))?;
            app.manage(AppState{node});
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![local_command])
        .build(tauri::generate_context!())
        .expect("Cannot start LoCAL. Check whether another instance is running.");
    app.run(|handle,event| { if let tauri::RunEvent::Exit=event { handle.state::<AppState>().node.stop(); } });
}

