mod discovery;
mod files;
mod identity;
pub mod protocol;
mod storage;

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use protocol::{Hello, Reply, Request};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{oneshot, Notify};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub struct Config {
    pub data_dir: PathBuf,
    pub receive_dir: PathBuf,
    pub name: String,
    pub bind: SocketAddr,
    pub discovery: bool,
}
impl Default for Config {
    fn default() -> Self {
        let dirs = directories::ProjectDirs::from("org", "sakus", "LoCAL")
            .expect("OS data directory unavailable");
        let receive_dir = directories::UserDirs::new()
            .and_then(|d| d.download_dir().map(|p| p.join("LoCAL")))
            .unwrap_or_else(|| dirs.data_local_dir().join("received"));
        Self {
            data_dir: dirs.data_local_dir().into(),
            receive_dir,
            name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "My device".into()),
            bind: "0.0.0.0:53319".parse().unwrap(),
            discovery: true,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub address: String,
    pub last_seen: i64,
    pub connected: bool,
    pub trusted: bool,
    pub ready: bool,
    pub code: Option<String>,
    pub local_confirmed: bool,
}

#[derive(Clone, Serialize)]
pub struct Transfer {
    pub id: String,
    pub peer_id: String,
    pub name: String,
    pub size: u64,
    pub bytes: u64,
    pub direction: String,
    pub status: String,
    pub error: String,
    pub path: Option<String>,
    pub timestamp: i64,
}

struct Cancel {
    flag: AtomicBool,
    wake: Notify,
}
impl Cancel {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            flag: AtomicBool::new(false),
            wake: Notify::new(),
        })
    }
    fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
    async fn cancelled(&self) {
        loop {
            let wake = self.wake.notified();
            if self.flag.load(Ordering::SeqCst) {
                return;
            }
            wake.await;
        }
    }
}

struct Session {
    conn: quinn::Connection,
    peer: Hello,
    code: String,
    local_ok: AtomicBool,
    remote_ok: AtomicBool,
    created: Instant,
}
impl Session {
    fn ready(&self) -> bool {
        self.local_ok.load(Ordering::SeqCst)
            && self.remote_ok.load(Ordering::SeqCst)
            && self.conn.close_reason().is_none()
    }
}

pub struct Node {
    pub id: String,
    pub receive_dir: PathBuf,
    pub data_dir: PathBuf,
    name: Mutex<String>,
    endpoint: quinn::Endpoint,
    store: Mutex<storage::Store>,
    peers: Mutex<HashMap<String, Peer>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    transfers: Mutex<HashMap<String, Transfer>>,
    decisions: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    cancellations: Mutex<HashMap<String, Arc<Cancel>>>,
    warnings: Mutex<Vec<String>>,
    stopping: AtomicBool,
    shutdown: Notify,
    _lock: std::fs::File,
}

impl Node {
    pub async fn start(config: Config) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(&config.receive_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.data_dir.join("instance.lock"))?;
        lock.try_lock_exclusive()
            .context("LoCAL is already running with this data directory")?;
        let identity = identity::Identity::load(&config.data_dir)?;
        let id = identity::device_id(&identity.cert)?;
        let (server, client) = identity.configs()?;
        let mut endpoint = quinn::Endpoint::server(server, config.bind).context(
            "Cannot listen. Another LoCAL instance may be using this port; use --port with the CLI",
        )?;
        endpoint.set_default_client_config(client);
        let store = storage::Store::open(&config.data_dir)?;
        let name = store.name().unwrap_or(config.name);
        let node = Arc::new(Self {
            id,
            receive_dir: config.receive_dir,
            data_dir: config.data_dir,
            name: Mutex::new(name),
            endpoint,
            store: Mutex::new(store),
            peers: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            decisions: Mutex::new(HashMap::new()),
            cancellations: Mutex::new(HashMap::new()),
            warnings: Mutex::new(vec![]),
            stopping: AtomicBool::new(false),
            shutdown: Notify::new(),
            _lock: lock,
        });
        let n = node.clone();
        tokio::spawn(async move {
            while let Some(incoming) = n.endpoint.accept().await {
                if n.sessions.lock().unwrap().len() >= 16 {
                    incoming.refuse();
                    continue;
                }
                let n = n.clone();
                tokio::spawn(async move {
                    let result = async {
                        let conn =
                            tokio::time::timeout(Duration::from_secs(10), incoming).await??;
                        let result = n.handshake(conn.clone(), false, None).await;
                        if result.is_err() {
                            conn.close(1u32.into(), b"handshake rejected");
                        }
                        result
                    }
                    .await;
                    if let Err(e) = result {
                        n.warn(format!("Connection: {e}"));
                    }
                });
            }
        });
        if config.discovery {
            discovery::start(node.clone());
        }
        Ok(node)
    }

    pub fn address(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }
    fn hello(&self) -> Hello {
        Hello {
            version: protocol::VERSION,
            id: self.id.clone(),
            name: self.name.lock().unwrap().clone(),
            port: self.endpoint.local_addr().unwrap().port(),
        }
    }
    fn warn(&self, text: String) {
        let mut warnings = self.warnings.lock().unwrap();
        if warnings.last() != Some(&text) {
            warnings.push(text);
            if warnings.len() > 5 {
                warnings.remove(0);
            }
        }
    }

    pub async fn connect(
        self: &Arc<Self>,
        address: &str,
        expected_id: Option<&str>,
    ) -> Result<String> {
        let address: SocketAddr = address
            .trim()
            .parse()
            .context("Enter an IPv4 address and port, for example 192.168.1.20:53319")?;
        if !address.is_ipv4()
            || address.port() == 0
            || address.ip().is_unspecified()
            || address.ip().is_multicast()
        {
            bail!("Use a unicast IPv4 address and nonzero port");
        }
        let conn = tokio::time::timeout(
            Duration::from_secs(10),
            self.endpoint.connect(address, "localmesh.local")?,
        )
        .await
        .context("Connection timed out; check Wi-Fi and firewall")??;
        let result = self.handshake(conn.clone(), true, expected_id).await;
        if result.is_err() {
            conn.close(1u32.into(), b"handshake rejected");
        }
        result
    }

    async fn handshake(
        self: &Arc<Self>,
        conn: quinn::Connection,
        outgoing: bool,
        expected_id: Option<&str>,
    ) -> Result<String> {
        let certs = conn
            .peer_identity()
            .context("Missing TLS device identity")?
            .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
            .map_err(|_| anyhow::anyhow!("Unexpected TLS identity"))?;
        let cert = certs.first().context("No peer certificate")?;
        let id = identity::device_id(cert.as_ref())?;
        if id == self.id {
            bail!("Cannot connect to this device itself");
        }
        if let Some(expected) = expected_id {
            if expected != id {
                bail!("Device identity changed. Pair again only after checking the other device");
            }
        }
        let peer: Hello = tokio::time::timeout(Duration::from_secs(10), async {
            let (mut send, mut recv) = if outgoing {
                conn.open_bi().await?
            } else {
                conn.accept_bi().await?
            };
            if outgoing {
                protocol::write(&mut send, &self.hello()).await?;
            }
            let peer: Hello = protocol::read(&mut recv).await?;
            if !outgoing {
                protocol::write(&mut send, &self.hello()).await?;
            }
            send.finish()?;
            Ok::<_, anyhow::Error>(peer)
        })
        .await??;
        if peer.version != protocol::VERSION
            || peer.id != id
            || peer.name.is_empty()
            || peer.name.len() > 80
            || peer.name.chars().any(char::is_control)
            || peer.port == 0
        {
            bail!("Invalid peer hello");
        }
        let mut material = [0; 32];
        conn.export_keying_material(&mut material, b"LoCAL pairing v1", b"")
            .map_err(|_| anyhow::anyhow!("Pairing exporter failed"))?;
        let code = format!(
            "{:06}",
            u32::from_be_bytes(material[..4].try_into()?) % 1_000_000
        );
        let trusted = self.store.lock().unwrap().trusted(&id);
        let session = Arc::new(Session {
            conn: conn.clone(),
            peer: peer.clone(),
            code,
            local_ok: AtomicBool::new(trusted),
            remote_ok: AtomicBool::new(false),
            created: Instant::now(),
        });
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(old) = sessions.get(&id) {
                if old.conn.close_reason().is_none() {
                    bail!("This device is already connected");
                }
            }
            if sessions.len() >= 16 {
                bail!("Too many peers; disconnect one first");
            }
            sessions.insert(id.clone(), session.clone());
        }
        self.peers.lock().unwrap().insert(
            id.clone(),
            Peer {
                id: id.clone(),
                name: peer.name,
                address: SocketAddr::new(conn.remote_address().ip(), peer.port).to_string(),
                last_seen: now(),
                connected: true,
                trusted,
                ready: false,
                code: None,
                local_confirmed: trusted,
            },
        );
        let n = self.clone();
        let s = session.clone();
        tokio::spawn(async move {
            n.serve(s).await;
        });
        let n = self.clone();
        let s = session.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(120)) => { if !s.ready() { s.conn.close(2u32.into(), b"pairing expired"); } },
                _ = s.conn.closed() => {}
            }
            let mut sessions = n.sessions.lock().unwrap();
            if let Some(current) = sessions.get(&s.peer.id) {
                if current.conn.stable_id() == s.conn.stable_id() && s.conn.close_reason().is_some()
                {
                    sessions.remove(&s.peer.id);
                }
            }
        });
        if trusted {
            self.send_confirmation(session).await?;
        }
        Ok(id)
    }

    async fn serve(self: Arc<Self>, session: Arc<Session>) {
        while let Ok((mut send, mut recv)) = session.conn.accept_bi().await {
            let n = self.clone();
            let s = session.clone();
            tokio::spawn(async move {
                let result = async {
                    let request: Request = protocol::read(&mut recv).await?;
                    match request {
                        Request::Confirm => {
                            if s.created.elapsed() > Duration::from_secs(120) && !s.ready() {
                                bail!("Pairing expired");
                            }
                            s.remote_ok.store(true, Ordering::SeqCst);
                            n.persist_trust(&s)?;
                            protocol::write(&mut send, &Reply::ok(0)).await?;
                        }
                        Request::Message { id, channel, text } => {
                            if !s.ready() {
                                bail!("Confirm pairing on both devices first");
                            }
                            if uuid::Uuid::parse_str(&id).is_err()
                                || text.is_empty()
                                || text.len() > protocol::MAX_TEXT
                                || !["mesh.text", "mesh.clipboard"].contains(&channel.as_str())
                            {
                                bail!("Invalid message");
                            }
                            n.store.lock().unwrap().insert_message(&storage::Message {
                                id,
                                peer_id: s.peer.id.clone(),
                                direction: "in".into(),
                                channel,
                                text,
                                timestamp: now(),
                            })?;
                            protocol::write(&mut send, &Reply::ok(0)).await?;
                        }
                        Request::File {
                            id,
                            name,
                            size,
                            hash,
                        } => {
                            if !s.ready() {
                                bail!("Confirm pairing on both devices first");
                            }
                            n.receive_file(&s, &mut send, &mut recv, id, name, size, hash)
                                .await?;
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                if let Err(error) = result {
                    let _ = protocol::write(&mut send, &Reply::error(error)).await;
                }
                let _ = send.finish();
            });
        }
        let mut sessions = self.sessions.lock().unwrap();
        if sessions
            .get(&session.peer.id)
            .is_some_and(|s| s.conn.stable_id() == session.conn.stable_id())
        {
            sessions.remove(&session.peer.id);
        }
    }

    fn persist_trust(&self, s: &Session) -> Result<()> {
        if s.ready() {
            self.store.lock().unwrap().trust(&s.peer.id, &s.peer.name)?;
        }
        Ok(())
    }
    fn session(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Device is disconnected. Connect again")
    }
    async fn send_confirmation(&self, s: Arc<Session>) -> Result<()> {
        let (mut send, mut recv) = s.conn.open_bi().await?;
        protocol::write(&mut send, &Request::Confirm).await?;
        send.finish()?;
        protocol::read::<Reply>(&mut recv).await?.check()?;
        self.persist_trust(&s)?;
        Ok(())
    }
    pub async fn confirm(&self, id: &str, code: &str) -> Result<()> {
        let s = self.session(id)?;
        if s.created.elapsed() > Duration::from_secs(120) && !s.ready() {
            bail!("Pairing expired. Reconnect");
        }
        if s.code != code {
            bail!("Pairing code does not match");
        }
        s.local_ok.store(true, Ordering::SeqCst);
        self.send_confirmation(s).await
    }
    pub fn disconnect(&self, id: &str) {
        if let Some(s) = self.sessions.lock().unwrap().remove(id) {
            s.conn.close(0u32.into(), b"disconnected locally");
        }
    }
    pub fn forget(&self, id: &str) -> Result<()> {
        self.disconnect(id);
        self.store.lock().unwrap().forget(id)?;
        Ok(())
    }
    pub async fn send_text(&self, peer_id: &str, text: String, channel: String) -> Result<String> {
        if text.is_empty()
            || text.len() > protocol::MAX_TEXT
            || !["mesh.text", "mesh.clipboard"].contains(&channel.as_str())
        {
            bail!("Text must be 1–16384 UTF-8 bytes and use a supported channel");
        }
        let s = self.session(peer_id)?;
        if !s.ready() {
            bail!("Confirm pairing on both devices first");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (mut send, mut recv) = s.conn.open_bi().await?;
        protocol::write(
            &mut send,
            &Request::Message {
                id: id.clone(),
                text: text.clone(),
                channel: channel.clone(),
            },
        )
        .await?;
        send.finish()?;
        protocol::read::<Reply>(&mut recv).await?.check()?;
        self.store
            .lock()
            .unwrap()
            .insert_message(&storage::Message {
                id: id.clone(),
                peer_id: peer_id.into(),
                direction: "out".into(),
                channel,
                text,
                timestamp: now(),
            })?;
        Ok(id)
    }
    pub fn snapshot(&self) -> Result<Value> {
        let store = self.store.lock().unwrap();
        let sessions = self.sessions.lock().unwrap();
        let mut peers: Vec<Peer> = self
            .peers
            .lock()
            .unwrap()
            .values()
            .filter(|p| {
                now() - p.last_seen < 20_000 || sessions.contains_key(&p.id) || store.trusted(&p.id)
            })
            .cloned()
            .collect();
        for p in &mut peers {
            let session = sessions
                .get(&p.id)
                .filter(|s| s.conn.close_reason().is_none());
            p.connected = session.is_some();
            p.trusted = store.trusted(&p.id);
            p.ready = session.is_some_and(|s| s.ready());
            p.code = session.filter(|s| !s.ready()).map(|s| s.code.clone());
            p.local_confirmed = session.is_some_and(|s| s.local_ok.load(Ordering::SeqCst));
        }
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        let mut transfers: Vec<_> = self.transfers.lock().unwrap().values().cloned().collect();
        transfers.sort_by_key(|t| std::cmp::Reverse(t.timestamp));
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"device":{"id":self.id,"name":*self.name.lock().unwrap(),"port":self.address()?.port(),"addresses":discovery::addresses(self.address()?.port())},"peers":peers,"trusted":store.peers()?,"messages":store.messages()?,"transfers":transfers,"receive_dir":self.receive_dir.to_string_lossy(),"warnings":*self.warnings.lock().unwrap()}),
        )
    }
    pub async fn command(self: &Arc<Self>, value: Value) -> Result<Value> {
        let field = |name: &str| -> Result<&str> {
            value
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("Missing {name}"))
        };
        match field("op")? {
            "snapshot" => self.snapshot(),
            "connect" => Ok(
                json!({"id":self.connect(field("address")?,value.get("peer_id").and_then(Value::as_str)).await?}),
            ),
            "confirm" => {
                self.confirm(field("peer_id")?, field("code")?).await?;
                Ok(json!({}))
            }
            "disconnect" => {
                self.disconnect(field("peer_id")?);
                Ok(json!({}))
            }
            "forget" => {
                self.forget(field("peer_id")?)?;
                Ok(json!({}))
            }
            "send_text" => Ok(
                json!({"id":self.send_text(field("peer_id")?,field("text")?.into(),value.get("channel").and_then(Value::as_str).unwrap_or("mesh.text").into()).await?}),
            ),
            "send_file" => Ok(
                json!({"id":self.send_file(field("peer_id")?,PathBuf::from(field("path")?)).await?}),
            ),
            "accept_file" => {
                self.decide_file(
                    field("id")?,
                    value
                        .get("accept")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )?;
                Ok(json!({}))
            }
            "cancel_transfer" => {
                self.cancel_transfer(field("id")?)?;
                Ok(json!({}))
            }
            "set_name" => {
                let name = field("name")?.trim();
                if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
                    bail!("Device name must be 1–80 bytes without control characters");
                }
                self.store.lock().unwrap().set_name(name)?;
                *self.name.lock().unwrap() = name.into();
                Ok(json!({}))
            }
            "clear_history" => {
                self.store.lock().unwrap().clear_history()?;
                Ok(json!({}))
            }
            _ => bail!("Unknown command"),
        }
    }
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.shutdown.notify_waiters();
        self.endpoint.close(0u32.into(), b"LoCAL closed");
        for cancel in self.cancellations.lock().unwrap().values() {
            cancel.cancel();
        }
    }
}
