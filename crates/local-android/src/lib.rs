use jni::{JNIEnv,objects::{JClass,JString},sys::jstring};
use local_core::{Config,Node};
use serde_json::{json,Value};
use std::{sync::{Arc,Mutex,OnceLock},path::PathBuf};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static NODE: Mutex<Option<Arc<Node>>> = Mutex::new(None);
fn runtime() -> &'static tokio::runtime::Runtime { RUNTIME.get_or_init(||tokio::runtime::Runtime::new().expect("Tokio runtime")) }
fn output(env: &mut JNIEnv<'_>, value: Value) -> jstring { env.new_string(value.to_string()).map(|s|s.into_raw()).unwrap_or(std::ptr::null_mut()) }
fn text(env: &mut JNIEnv<'_>, value: JString<'_>) -> Result<String,String> { env.get_string(&value).map(|v|v.into()).map_err(|e|e.to_string()) }

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_start(mut env: JNIEnv, _: JClass, data: JString, received: JString, name: JString) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut node = NODE.lock().map_err(|e|e.to_string())?;
        if node.is_some() { return Ok(json!({})); }
        let config=Config {data_dir:PathBuf::from(text(&mut env,data)?),receive_dir:PathBuf::from(text(&mut env,received)?),name:text(&mut env,name)?,bind:"0.0.0.0:53319".parse().unwrap(),discovery:true};
        *node=Some(runtime().block_on(Node::start(config)).map_err(|e|e.to_string())?);
        Ok::<Value,String>(json!({}))
    }));
    output(&mut env,match result {Ok(Ok(data))=>json!({"ok":true,"data":data}),Ok(Err(e))=>json!({"ok":false,"error":e}),Err(_)=>json!({"ok":false,"error":"Native startup failed"})})
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_command(mut env: JNIEnv, _: JClass, request: JString) -> jstring {
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let value:Value=serde_json::from_str(&text(&mut env,request)?).map_err(|e|e.to_string())?;
        let node=NODE.lock().map_err(|e|e.to_string())?.clone().ok_or("LoCAL is starting")?;
        runtime().block_on(node.command(value)).map_err(|e|e.to_string())
    }));
    output(&mut env,match result {Ok(Ok(data))=>json!({"ok":true,"data":data}),Ok(Err(e))=>json!({"ok":false,"error":e}),Err(_)=>json!({"ok":false,"error":"Native operation failed"})})
}

#[no_mangle]
pub extern "system" fn Java_org_sakus_local_Native_stop(_: JNIEnv, _: JClass) {
    if let Ok(mut node)=NODE.lock() { if let Some(node)=node.take() { node.stop(); } }
}

