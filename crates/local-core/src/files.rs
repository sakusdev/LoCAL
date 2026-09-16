use crate::{
    now,
    protocol::{self, Reply, Request},
    Cancel, Node, Session, Transfer,
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::oneshot,
};

#[derive(serde::Serialize)]
struct ReceivedFile {
    name: String,
    path: String,
    size: u64,
    timestamp: u128,
}

async fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = tokio::fs::File::open(path).await?;
    hash_open_file(&mut file).await
}

async fn hash_open_file(file: &mut tokio::fs::File) -> Result<(String, u64)> {
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() || metadata.len() > protocol::MAX_FILE {
        bail!("Select a regular file up to 20 GiB");
    }
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0u8; protocol::CHUNK];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > protocol::MAX_FILE {
            bail!("File grew past size limit");
        }
        hash.update(&buffer[..read]);
    }
    if size != metadata.len() {
        bail!("File changed while reading. Try again when the file is no longer being edited");
    }
    Ok((hash.finalize().to_hex().to_string(), size))
}

impl Node {
    pub(crate) async fn received_files(&self, offset: usize) -> Result<serde_json::Value> {
        let directory = self.receive_dir.clone();
        tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
            let directory = std::fs::canonicalize(directory)?;
            let mut files = Vec::new();
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                // Never expose partial downloads, subdirectories or symbolic links.
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let filename = entry.file_name().to_string_lossy().into_owned();
                if filename.starts_with('.') {
                    continue;
                }
                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                let name = filename
                    .split_once('_')
                    .map_or(filename.as_str(), |(id, name)| {
                        if uuid::Uuid::parse_str(id).is_ok() {
                            name
                        } else {
                            &filename
                        }
                    });
                files.push(ReceivedFile {
                    name: name.into(),
                    path: entry.path().to_string_lossy().into_owned(),
                    size: metadata.len(),
                    timestamp: metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |duration| duration.as_millis()),
                });
            }
            files.sort_by(|a, b| {
                b.timestamp
                    .cmp(&a.timestamp)
                    .then_with(|| a.path.cmp(&b.path))
            });
            let total = files.len();
            let offset = offset.min(total.saturating_sub(1) / 50 * 50);
            let files: Vec<_> = files.into_iter().skip(offset).take(50).collect();
            Ok(serde_json::json!({"files":files,"total":total,"offset":offset,"limit":50}))
        })
        .await?
    }

    fn begin_transfer(&self, transfer: Transfer) -> Result<Arc<Cancel>> {
        let mut cancellations = self.cancellations.lock().unwrap();
        if cancellations.len() >= 8 || cancellations.contains_key(&transfer.id) {
            bail!("Too many active transfers");
        }
        let cancel = Cancel::new();
        cancellations.insert(transfer.id.clone(), cancel.clone());
        let mut transfers = self.transfers.lock().unwrap();
        if transfers.len() >= 100 {
            let oldest = transfers
                .values()
                .filter(|t| !cancellations.contains_key(&t.id))
                .min_by_key(|t| t.timestamp)
                .map(|t| t.id.clone());
            if let Some(id) = oldest {
                transfers.remove(&id);
            }
        }
        transfers.insert(transfer.id.clone(), transfer);
        Ok(cancel)
    }
    fn progress(&self, id: &str, status: &str, bytes: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(id) {
            t.status = status.into();
            t.bytes = bytes;
        }
    }
    fn finish_transfer(&self, id: &str, result: &Result<()>) {
        self.cancellations.lock().unwrap().remove(id);
        self.decisions.lock().unwrap().remove(id);
        if let Some(t) = self.transfers.lock().unwrap().get_mut(id) {
            match result {
                Ok(()) => {
                    t.status = "completed".into();
                    t.bytes = t.size;
                }
                Err(e) => {
                    t.error = e.to_string();
                    t.status = if t.error.contains("cancelled") {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                }
            }
        }
    }
    pub fn decide_file(&self, id: &str, accept: bool) -> Result<()> {
        self.decisions
            .lock()
            .unwrap()
            .remove(id)
            .context("This offer has expired")?
            .send(accept)
            .map_err(|_| anyhow::anyhow!("Sender disconnected"))
    }
    pub fn cancel_transfer(&self, id: &str) -> Result<()> {
        let cancel = self
            .cancellations
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Transfer is already finished")?;
        cancel.cancel();
        Ok(())
    }
    pub async fn send_file(self: &Arc<Self>, peer_id: &str, path: PathBuf) -> Result<String> {
        let session = self.session(peer_id)?;
        if !session.ready() {
            bail!("Confirm pairing on both devices first");
        }
        if !session.supports(protocol::CAP_FILE) {
            bail!("Peer does not support file transfer");
        }
        let metadata = tokio::fs::metadata(&path)
            .await
            .context("Cannot open selected file")?;
        if !metadata.is_file() || metadata.len() > protocol::MAX_FILE {
            bail!("Select a regular file up to 20 GiB");
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("Filename must be valid Unicode")?
            .to_string();
        protocol::validate_name(&name)?;
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = self.begin_transfer(Transfer {
            id: id.clone(),
            peer_id: peer_id.into(),
            name: name.clone(),
            size: metadata.len(),
            bytes: 0,
            direction: "out".into(),
            status: "hashing".into(),
            error: String::new(),
            path: Some(path.to_string_lossy().into_owned()),
            timestamp: now(),
        })?;
        let n = self.clone();
        let transfer_id = id.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => Err(anyhow::anyhow!("Transfer cancelled")),
                result = n.send_file_inner(&session,&transfer_id,&path,name) => result
            };
            n.finish_transfer(&transfer_id, &result);
        });
        Ok(id)
    }
    async fn send_file_inner(
        &self,
        session: &Session,
        id: &str,
        path: &Path,
        name: String,
    ) -> Result<()> {
        let (hash, size) = hash_file(path).await?;
        if let Some(t) = self.transfers.lock().unwrap().get_mut(id) {
            t.size = size;
        }
        let (mut send, mut recv) = session.conn.open_bi().await?;
        self.progress(id, "awaiting_acceptance", 0);
        protocol::write(
            &mut send,
            &Request::File {
                id: id.into(),
                name,
                size,
                hash,
            },
        )
        .await?;
        let offset = protocol::read::<Reply>(&mut recv).await?.check()?;
        if offset > size {
            bail!("Invalid resume offset");
        }
        let mut file = tokio::fs::File::open(path).await?;
        if file.metadata().await?.len() != size {
            bail!("File changed before sending");
        }
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut sent = offset;
        let mut buffer = vec![0u8; protocol::CHUNK];
        self.progress(id, "transferring", sent);
        while sent < size {
            let max = (buffer.len() as u64).min(size - sent) as usize;
            let count = file.read(&mut buffer[..max]).await?;
            if count == 0 {
                bail!("File changed while sending");
            }
            tokio::time::timeout(Duration::from_secs(30), send.write_all(&buffer[..count]))
                .await??;
            sent += count as u64;
            self.progress(id, "transferring", sent);
        }
        send.finish()?;
        self.progress(id, "verifying", size);
        protocol::read::<Reply>(&mut recv).await?.check()?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn receive_file(
        &self,
        session: &Session,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        wire_id: String,
        name: String,
        size: u64,
        hash: String,
    ) -> Result<()> {
        protocol::validate_name(&name)?;
        if uuid::Uuid::parse_str(&wire_id).is_err()
            || !protocol::valid_hash(&hash)
            || size > protocol::MAX_FILE
        {
            bail!("Invalid file offer");
        }
        let id = format!("rx-{}-{wire_id}", &session.peer.id[..8]);
        let cancel = self.begin_transfer(Transfer {
            id: id.clone(),
            peer_id: session.peer.id.clone(),
            name: name.clone(),
            size,
            bytes: 0,
            direction: "in".into(),
            status: "offered".into(),
            error: String::new(),
            path: None,
            timestamp: now(),
        })?;
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(anyhow::anyhow!("Transfer cancelled")),
            result = self.receive_file_inner(session,send,recv,&id,&name,size,&hash) => result
        };
        self.finish_transfer(&id, &result);
        result
    }
    #[allow(clippy::too_many_arguments)]
    async fn receive_file_inner(
        &self,
        session: &Session,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        id: &str,
        name: &str,
        size: u64,
        hash: &str,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.decisions.lock().unwrap().insert(id.into(), tx);
        let accept = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(120),rx) => result.context("File offer expired")?.context("File offer cancelled")?,
            _ = session.conn.closed() => bail!("Sender disconnected")
        };
        if !accept {
            bail!("Receiver declined the file");
        }
        if !session.ready() {
            bail!("Pairing is no longer trusted");
        }
        let partial_dir = self.receive_dir.join(".partial");
        tokio::fs::create_dir_all(&partial_dir).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&partial_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let partial = partial_dir.join(format!("{}-{hash}.part", session.peer.id));
        let std_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&partial)?;
        std_file
            .try_lock_exclusive()
            .context("This file is already being received")?;
        let mut offset = std_file.metadata()?.len();
        if offset > size {
            std_file.set_len(0)?;
            offset = 0;
        }
        if fs2::available_space(&self.receive_dir)? < size - offset + 1024 * 1024 {
            bail!("Not enough free disk space");
        }
        let mut file = tokio::fs::File::from_std(std_file);
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        protocol::write(send, &Reply::ok(offset)).await?;
        let mut received = offset;
        let mut buffer = vec![0u8; protocol::CHUNK];
        while received < size {
            let max = (buffer.len() as u64).min(size - received) as usize;
            let count =
                tokio::time::timeout(Duration::from_secs(30), recv.read(&mut buffer[..max]))
                    .await??
                    .context("Transfer interrupted; resend the same file to resume")?;
            if count == 0 {
                bail!("Transfer ended early");
            }
            file.write_all(&buffer[..count]).await?;
            received += count as u64;
            self.progress(id, "transferring", received);
        }
        let mut extra = [0u8; 1];
        if tokio::time::timeout(Duration::from_secs(30), recv.read(&mut extra))
            .await??
            .is_some()
        {
            bail!("File exceeds declared size");
        }
        file.flush().await?;
        file.sync_all().await?;
        self.progress(id, "verifying", received);
        // Read through the handle that owns the lock: Windows range locks reject
        // reads made through a second handle, even from this process.
        let (actual, actual_size) = hash_open_file(&mut file).await?;
        if actual != hash || actual_size != size {
            file.set_len(0).await?;
            bail!("BLAKE3 verification failed. The partial file was reset; resend the original");
        }
        let final_path = self
            .receive_dir
            .join(format!("{}_{}", uuid::Uuid::new_v4(), name));
        // Same filesystem, atomic no-clobber publication; never overwrite a user's file.
        tokio::fs::hard_link(&partial, &final_path)
            .await
            .context("Cannot publish received file")?;
        drop(file);
        tokio::fs::remove_file(&partial).await?;
        if let Some(t) = self.transfers.lock().unwrap().get_mut(id) {
            t.path = Some(final_path.to_string_lossy().into_owned());
        }
        protocol::write(send, &Reply::ok(size)).await?;
        Ok(())
    }
}
