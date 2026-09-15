use crate::{now, protocol, Node, Session};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{broadcast, oneshot, Notify};

const MAX_SCREEN_SESSIONS: usize = 8;
const MAX_VISIBLE_SCREEN_SESSIONS: usize = 50;
const VIDEO_QUEUE_FRAMES: usize = 4;
const OFFER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize)]
pub struct ScreenSessionState {
    pub id: String,
    pub peer_id: String,
    pub direction: String,
    pub status: String,
    pub offer: protocol::ScreenOffer,
    pub error: String,
    pub timestamp: i64,
}

#[derive(Debug)]
pub struct ScreenVideoPacket {
    pub header: protocol::ScreenFrameHeader,
    pub data: Vec<u8>,
}

struct VideoQueue {
    frames: Mutex<VecDeque<ScreenVideoPacket>>,
    wake: Notify,
    closed: AtomicBool,
    claimed: AtomicBool,
}

impl VideoQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            frames: Mutex::new(VecDeque::with_capacity(VIDEO_QUEUE_FRAMES)),
            wake: Notify::new(),
            closed: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
        })
    }

    fn push(&self, packet: ScreenVideoPacket) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut frames = self.frames.lock().unwrap();
        while frames.len() >= VIDEO_QUEUE_FRAMES {
            frames.pop_front();
        }
        frames.push_back(packet);
        drop(frames);
        self.wake.notify_one();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
}

pub struct ScreenVideoReceiver {
    queue: Arc<VideoQueue>,
}

impl ScreenVideoReceiver {
    pub async fn recv(&mut self) -> Option<ScreenVideoPacket> {
        loop {
            let notified = self.queue.wake.notified();
            if let Some(packet) = self.queue.frames.lock().unwrap().pop_front() {
                return Some(packet);
            }
            if self.queue.closed.load(Ordering::SeqCst) {
                return None;
            }
            notified.await;
        }
    }
}

pub struct ScreenVideoSender {
    stream: quinn::SendStream,
    last_sequence: Option<u64>,
    last_timestamp_us: Option<u64>,
}

impl ScreenVideoSender {
    pub async fn send_frame(
        &mut self,
        header: protocol::ScreenFrameHeader,
        payload: &[u8],
    ) -> Result<()> {
        header.validate()?;
        if self
            .last_sequence
            .is_some_and(|last| header.sequence <= last)
        {
            bail!("Screen frame sequence must increase");
        }
        if self
            .last_timestamp_us
            .is_some_and(|last| header.timestamp_us < last)
        {
            bail!("Screen frame timestamp moved backwards");
        }
        protocol::write_screen_frame(&mut self.stream, &header, payload).await?;
        self.last_sequence = Some(header.sequence);
        self.last_timestamp_us = Some(header.timestamp_us);
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.stream.finish()?;
        Ok(())
    }
}

pub(crate) struct Runtime {
    capabilities: Mutex<Option<protocol::ScreenCapabilities>>,
    states: Mutex<HashMap<String, ScreenSessionState>>,
    decisions: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    signals: Mutex<HashMap<String, broadcast::Sender<protocol::ScreenSignal>>>,
    videos: Mutex<HashMap<String, Arc<VideoQueue>>>,
}

impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            capabilities: Mutex::new(None),
            states: Mutex::new(HashMap::new()),
            decisions: Mutex::new(HashMap::new()),
            signals: Mutex::new(HashMap::new()),
            videos: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn advertisement(&self) -> (Vec<String>, Option<protocol::ScreenCapabilities>) {
        let screen = self.capabilities.lock().unwrap().clone();
        (protocol::screen_capability_names(screen.as_ref()), screen)
    }

    fn local_capabilities(&self) -> Result<protocol::ScreenCapabilities> {
        self.capabilities
            .lock()
            .unwrap()
            .clone()
            .context("This device has no screen backend enabled")
    }

    fn insert_state(&self, state: ScreenSessionState) -> Result<()> {
        let mut removed = None;
        {
            let mut states = self.states.lock().unwrap();
            if states.contains_key(&state.id) {
                bail!("Screen session ID is already in use");
            }
            let active = states
                .values()
                .filter(|item| {
                    matches!(
                        item.status.as_str(),
                        "awaiting_acceptance" | "offered" | "active"
                    )
                })
                .count();
            if active >= MAX_SCREEN_SESSIONS {
                bail!("Too many active screen sessions");
            }
            if states.len() >= MAX_VISIBLE_SCREEN_SESSIONS {
                removed = states
                    .values()
                    .filter(|item| {
                        !matches!(
                            item.status.as_str(),
                            "awaiting_acceptance" | "offered" | "active"
                        )
                    })
                    .min_by_key(|item| item.timestamp)
                    .map(|item| item.id.clone());
                if let Some(id) = &removed {
                    states.remove(id);
                } else {
                    bail!("Screen session history is full");
                }
            }
            states.insert(state.id.clone(), state);
        }
        if let Some(id) = removed {
            self.signals.lock().unwrap().remove(&id);
            if let Some(queue) = self.videos.lock().unwrap().remove(&id) {
                queue.close();
            }
        }
        Ok(())
    }

    fn state(&self, id: &str) -> Result<ScreenSessionState> {
        self.states
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Unknown screen session")
    }

    fn set_status(&self, id: &str, status: &str) {
        if let Some(state) = self.states.lock().unwrap().get_mut(id) {
            state.status = status.into();
            if status != "failed" {
                state.error.clear();
            }
        }
    }

    fn fail(&self, id: &str, error: impl ToString) {
        if let Some(state) = self.states.lock().unwrap().get_mut(id) {
            state.status = "failed".into();
            state.error = error.to_string();
        }
        if let Some(queue) = self.videos.lock().unwrap().get(id) {
            queue.close();
        }
    }

    pub(crate) fn states(&self) -> Vec<ScreenSessionState> {
        let mut states: Vec<_> = self.states.lock().unwrap().values().cloned().collect();
        states.sort_by_key(|state| std::cmp::Reverse(state.timestamp));
        states
    }

    pub(crate) fn peer_closed(&self, peer_id: &str) {
        let ids: Vec<String> = self
            .states
            .lock()
            .unwrap()
            .values()
            .filter(|state| {
                state.peer_id == peer_id
                    && matches!(
                        state.status.as_str(),
                        "awaiting_acceptance" | "offered" | "active"
                    )
            })
            .map(|state| state.id.clone())
            .collect();
        for id in ids {
            if let Some(decision) = self.decisions.lock().unwrap().remove(&id) {
                let _ = decision.send(false);
            }
            if let Some(signal) = self.signals.lock().unwrap().get(&id) {
                let _ = signal.send(protocol::ScreenSignal::Stop);
            }
            if let Some(queue) = self.videos.lock().unwrap().get(&id) {
                queue.close();
            }
            self.set_status(&id, "stopped");
        }
    }

    pub(crate) fn stop_all(&self) {
        let ids: Vec<String> = self.states.lock().unwrap().keys().cloned().collect();
        for id in ids {
            if let Some(decision) = self.decisions.lock().unwrap().remove(&id) {
                let _ = decision.send(false);
            }
            if let Some(signal) = self.signals.lock().unwrap().get(&id) {
                let _ = signal.send(protocol::ScreenSignal::Stop);
            }
            if let Some(queue) = self.videos.lock().unwrap().get(&id) {
                queue.close();
            }
            self.set_status(&id, "stopped");
        }
    }
}

impl Node {
    pub fn set_screen_capabilities(
        &self,
        capabilities: Option<protocol::ScreenCapabilities>,
    ) -> Result<()> {
        if !self.sessions.lock().unwrap().is_empty() {
            bail!("Disconnect peers before changing screen capabilities");
        }
        let names = protocol::screen_capability_names(capabilities.as_ref());
        protocol::validate_capabilities(&names, capabilities.as_ref())?;
        *self.screen_runtime.capabilities.lock().unwrap() = capabilities;
        Ok(())
    }

    pub(crate) fn screen_advertisement(
        &self,
    ) -> (Vec<String>, Option<protocol::ScreenCapabilities>) {
        self.screen_runtime.advertisement()
    }

    pub fn screen_sessions(&self) -> Vec<ScreenSessionState> {
        self.screen_runtime.states()
    }

    pub async fn offer_screen(&self, peer_id: &str, offer: protocol::ScreenOffer) -> Result<bool> {
        offer.validate()?;
        let session = self.session(peer_id)?;
        if !session.ready() {
            bail!("Confirm pairing on both devices first");
        }
        if !session.supports(protocol::CAP_SCREEN_VIEW) {
            bail!("Peer does not advertise screen viewing support");
        }
        let source = self.screen_runtime.local_capabilities()?;
        let viewer = session
            .peer
            .screen
            .as_ref()
            .context("Peer omitted screen capability metadata")?;
        protocol::validate_screen_offer(&source, viewer, &offer)?;
        self.screen_runtime.insert_state(ScreenSessionState {
            id: offer.id.clone(),
            peer_id: peer_id.into(),
            direction: "out".into(),
            status: "awaiting_acceptance".into(),
            offer: offer.clone(),
            error: String::new(),
            timestamp: now(),
        })?;
        let (signal, _) = broadcast::channel(8);
        self.screen_runtime
            .signals
            .lock()
            .unwrap()
            .insert(offer.id.clone(), signal);

        let result = async {
            let (mut send, mut recv) = session.conn.open_bi().await?;
            protocol::write(
                &mut send,
                &protocol::Request::ScreenOffer {
                    offer: offer.clone(),
                },
            )
            .await?;
            send.finish()?;
            let accepted = protocol::read::<protocol::Reply>(&mut recv)
                .await?
                .screen_decision()?;
            if accepted {
                self.screen_runtime.set_status(&offer.id, "active");
            } else {
                self.screen_runtime.set_status(&offer.id, "rejected");
            }
            Ok::<_, anyhow::Error>(accepted)
        }
        .await;
        if let Err(error) = &result {
            self.screen_runtime.fail(&offer.id, error);
        }
        result
    }

    pub fn decide_screen(&self, id: &str, accept: bool) -> Result<()> {
        let state = self.screen_runtime.state(id)?;
        if state.direction != "in" || state.status != "offered" {
            bail!("Screen offer is no longer awaiting a decision");
        }
        self.screen_runtime
            .decisions
            .lock()
            .unwrap()
            .remove(id)
            .context("Screen offer has expired")?
            .send(accept)
            .map_err(|_| anyhow::anyhow!("Screen source disconnected"))
    }

    pub async fn stop_screen(&self, id: &str) -> Result<()> {
        let state = self.screen_runtime.state(id)?;
        if state.direction == "in" && state.status == "offered" {
            self.decide_screen(id, false)?;
            self.screen_runtime.set_status(id, "rejected");
            return Ok(());
        }
        if !matches!(state.status.as_str(), "awaiting_acceptance" | "active") {
            bail!("Screen session is not active");
        }
        let _ = self
            .send_screen_signal(&state.peer_id, id, protocol::ScreenSignal::Stop)
            .await;
        self.screen_runtime.set_status(id, "stopped");
        if let Some(signal) = self.screen_runtime.signals.lock().unwrap().get(id) {
            let _ = signal.send(protocol::ScreenSignal::Stop);
        }
        if let Some(queue) = self.screen_runtime.videos.lock().unwrap().get(id) {
            queue.close();
        }
        Ok(())
    }

    pub async fn request_screen_keyframe(&self, id: &str) -> Result<()> {
        let state = self.screen_runtime.state(id)?;
        if state.direction != "in" || state.status != "active" {
            bail!("Only an active screen viewer can request a keyframe");
        }
        self.send_screen_signal(&state.peer_id, id, protocol::ScreenSignal::RequestKeyframe)
            .await
    }

    async fn send_screen_signal(
        &self,
        peer_id: &str,
        id: &str,
        signal: protocol::ScreenSignal,
    ) -> Result<()> {
        let session = self.session(peer_id)?;
        if !session.ready() {
            bail!("Screen peer is disconnected");
        }
        let (mut send, mut recv) = session.conn.open_bi().await?;
        protocol::write(
            &mut send,
            &protocol::Request::ScreenSignal {
                id: id.into(),
                signal,
            },
        )
        .await?;
        send.finish()?;
        protocol::read::<protocol::Reply>(&mut recv)
            .await?
            .check()?;
        Ok(())
    }

    pub fn subscribe_screen_signals(
        &self,
        id: &str,
    ) -> Result<broadcast::Receiver<protocol::ScreenSignal>> {
        let state = self.screen_runtime.state(id)?;
        if state.direction != "out" {
            bail!("Only the screen source receives encoder signals");
        }
        Ok(self
            .screen_runtime
            .signals
            .lock()
            .unwrap()
            .get(id)
            .context("Screen signal channel is unavailable")?
            .subscribe())
    }

    pub async fn open_screen_video(&self, id: &str) -> Result<ScreenVideoSender> {
        let state = self.screen_runtime.state(id)?;
        if state.direction != "out" || state.status != "active" {
            bail!("Screen session is not an active outgoing share");
        }
        let session = self.session(&state.peer_id)?;
        if !session.ready() {
            bail!("Screen peer is disconnected");
        }
        let mut stream = session.conn.open_uni().await?;
        protocol::write(&mut stream, &state.offer.stream_init()).await?;
        Ok(ScreenVideoSender {
            stream,
            last_sequence: None,
            last_timestamp_us: None,
        })
    }

    pub fn take_screen_video_receiver(&self, id: &str) -> Result<ScreenVideoReceiver> {
        let state = self.screen_runtime.state(id)?;
        if state.direction != "in" || state.status != "active" {
            bail!("Screen session is not an active incoming view");
        }
        let queue = self
            .screen_runtime
            .videos
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Screen video queue is unavailable")?;
        queue
            .claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| anyhow::anyhow!("Screen video receiver is already taken"))?;
        Ok(ScreenVideoReceiver { queue })
    }

    pub(crate) async fn receive_screen_offer(
        &self,
        session: &Session,
        offer: protocol::ScreenOffer,
    ) -> Result<bool> {
        if !session.ready() {
            bail!("Confirm pairing on both devices first");
        }
        if !session.supports(protocol::CAP_SCREEN_SHARE) {
            bail!("Peer did not advertise screen sharing support");
        }
        let source = session
            .peer
            .screen
            .as_ref()
            .context("Peer omitted screen capability metadata")?;
        let viewer = self.screen_runtime.local_capabilities()?;
        protocol::validate_screen_offer(source, &viewer, &offer)?;
        let (tx, rx) = oneshot::channel();
        self.screen_runtime.insert_state(ScreenSessionState {
            id: offer.id.clone(),
            peer_id: session.peer.id.clone(),
            direction: "in".into(),
            status: "offered".into(),
            offer: offer.clone(),
            error: String::new(),
            timestamp: now(),
        })?;
        self.screen_runtime
            .decisions
            .lock()
            .unwrap()
            .insert(offer.id.clone(), tx);
        let accepted = tokio::select! {
            result = tokio::time::timeout(OFFER_TIMEOUT, rx) => {
                result.context("Screen offer expired")?.context("Screen offer cancelled")?
            }
            _ = session.conn.closed() => bail!("Screen source disconnected")
        };
        self.screen_runtime
            .decisions
            .lock()
            .unwrap()
            .remove(&offer.id);
        if accepted {
            if !session.ready() {
                bail!("Pairing is no longer trusted");
            }
            self.screen_runtime.set_status(&offer.id, "active");
            self.screen_runtime
                .videos
                .lock()
                .unwrap()
                .insert(offer.id.clone(), VideoQueue::new());
        } else {
            self.screen_runtime.set_status(&offer.id, "rejected");
        }
        Ok(accepted)
    }

    pub(crate) fn receive_screen_signal(
        &self,
        session: &Session,
        id: &str,
        signal: protocol::ScreenSignal,
    ) -> Result<()> {
        if !session.ready() || uuid::Uuid::parse_str(id).is_err() {
            bail!("Invalid screen signal");
        }
        let state = self.screen_runtime.state(id)?;
        if state.peer_id != session.peer.id {
            bail!("Screen session belongs to another peer");
        }
        match signal {
            protocol::ScreenSignal::Stop => {
                if state.direction == "in" && state.status == "offered" {
                    if let Some(decision) = self.screen_runtime.decisions.lock().unwrap().remove(id)
                    {
                        let _ = decision.send(false);
                    }
                }
                if let Some(bus) = self.screen_runtime.signals.lock().unwrap().get(id) {
                    let _ = bus.send(protocol::ScreenSignal::Stop);
                }
                if let Some(queue) = self.screen_runtime.videos.lock().unwrap().get(id) {
                    queue.close();
                }
                self.screen_runtime.set_status(id, "stopped");
            }
            protocol::ScreenSignal::RequestKeyframe => {
                if state.direction != "out" || state.status != "active" {
                    bail!("Keyframe request does not target an active screen source");
                }
                self.screen_runtime
                    .signals
                    .lock()
                    .unwrap()
                    .get(id)
                    .context("Screen signal channel is unavailable")?
                    .send(protocol::ScreenSignal::RequestKeyframe)
                    .map_err(|_| anyhow::anyhow!("Screen encoder is not listening"))?;
            }
        }
        Ok(())
    }

    pub(crate) async fn serve_screen_streams(self: Arc<Self>, session: Arc<Session>) {
        while let Ok(mut recv) = session.conn.accept_uni().await {
            let node = self.clone();
            let peer = session.clone();
            tokio::spawn(async move {
                if let Err(error) = node.receive_screen_stream(&peer, &mut recv).await {
                    node.warn(format!("Screen stream: {error}"));
                }
            });
        }
    }

    async fn receive_screen_stream(
        &self,
        session: &Session,
        recv: &mut quinn::RecvStream,
    ) -> Result<()> {
        if !session.ready() {
            bail!("Screen stream arrived before pairing was confirmed");
        }
        let init: protocol::ScreenStreamInit = protocol::read(recv).await?;
        init.validate()?;
        let state = self.screen_runtime.state(&init.id)?;
        if state.peer_id != session.peer.id
            || state.direction != "in"
            || state.status != "active"
            || init != state.offer.stream_init()
        {
            bail!("Screen stream does not match an accepted offer");
        }
        let queue = self
            .screen_runtime
            .videos
            .lock()
            .unwrap()
            .get(&init.id)
            .cloned()
            .context("Screen video queue is unavailable")?;
        let mut last_sequence = None;
        let mut last_timestamp_us = None;
        let result = async {
            while let Some((header, data)) = protocol::read_screen_frame(recv).await? {
                if last_sequence.is_some_and(|last| header.sequence <= last) {
                    bail!("Screen frame sequence moved backwards");
                }
                if last_timestamp_us.is_some_and(|last| header.timestamp_us < last) {
                    bail!("Screen frame timestamp moved backwards");
                }
                last_sequence = Some(header.sequence);
                last_timestamp_us = Some(header.timestamp_us);
                queue.push(ScreenVideoPacket { header, data });
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        queue.close();
        match &result {
            Ok(()) => self.screen_runtime.set_status(&init.id, "stopped"),
            Err(error) => self.screen_runtime.fail(&init.id, error),
        }
        result
    }
}
