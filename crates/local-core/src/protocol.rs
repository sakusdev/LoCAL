use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const VERSION: u8 = 1;
pub const MAX_FRAME: usize = 96 * 1024;
pub const MAX_TEXT: usize = 16 * 1024;
pub const MAX_FILE: u64 = 20 * 1024 * 1024 * 1024;
pub const CHUNK: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello { pub version: u8, pub id: String, pub name: String, pub port: u16 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Confirm,
    Message { id: String, channel: String, text: String },
    File { id: String, name: String, size: u64, hash: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Reply { pub ok: bool, pub error: String, pub offset: u64 }
impl Reply {
    pub fn ok(offset: u64) -> Self { Self { ok: true, error: String::new(), offset } }
    pub fn error(error: impl ToString) -> Self { Self { ok: false, error: error.to_string(), offset: 0 } }
    pub fn check(self) -> Result<u64> { if !self.ok { bail!("{}", self.error); } Ok(self.offset) }
}

pub async fn write<T: Serialize>(stream: &mut quinn::SendStream, value: &T) -> Result<()> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)?;
    if bytes.len() > MAX_FRAME { bail!("Protocol frame is too large"); }
    stream.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

pub async fn read<T: DeserializeOwned>(stream: &mut quinn::RecvStream) -> Result<T> {
    let mut length = [0; 4];
    tokio::time::timeout(std::time::Duration::from_secs(130), stream.read_exact(&mut length)).await??;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME { bail!("Invalid protocol frame length"); }
    let mut data = vec![0; length];
    tokio::time::timeout(std::time::Duration::from_secs(15), stream.read_exact(&mut data)).await??;
    let mut reader = std::io::Cursor::new(&data);
    let result = ciborium::from_reader(&mut reader)?;
    if reader.position() as usize != data.len() { bail!("Trailing protocol bytes"); }
    Ok(result)
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 180 || name == "." || name == ".."
        || name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || name.ends_with('.') || name.ends_with(' ') {
        bail!("Unsafe or unsupported filename");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9')) {
        bail!("Reserved filename");
    }
    Ok(())
}

pub fn valid_hash(value: &str) -> bool { value.len() == 64 && value.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) }

