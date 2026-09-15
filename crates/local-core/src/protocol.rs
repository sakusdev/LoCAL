use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const VERSION: u8 = 1;
pub const MAX_FRAME: usize = 96 * 1024;
pub const MAX_TEXT: usize = 16 * 1024;
pub const MAX_FILE: u64 = 20 * 1024 * 1024 * 1024;
pub const CHUNK: usize = 1024 * 1024;

pub const CAP_TEXT: &str = "text";
pub const CAP_FILE: &str = "file";
pub const CAP_CLIPBOARD: &str = "clipboard";
pub const CAP_SCREEN_VIEW: &str = "screen.view";
pub const CAP_SCREEN_CONTROL: &str = "screen.control";
pub const CAP_SCREEN_AUDIO: &str = "screen.audio";
pub const CAP_AUDIO: &str = "audio";
pub const CAP_SENSOR: &str = "sensor";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u8,
    pub id: String,
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Confirm,
    Message {
        id: String,
        channel: String,
        text: String,
    },
    File {
        id: String,
        name: String,
        size: u64,
        hash: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenCodec {
    H264,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenCapabilities {
    pub codecs: Vec<ScreenCodec>,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u16,
    pub control: bool,
    pub system_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenOffer {
    pub id: String,
    pub source: String,
    pub codec: ScreenCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u16,
    pub system_audio: bool,
    pub control: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrameHeader {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub payload_len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceUri {
    pub device_id: String,
    pub resource: String,
}

impl ResourceUri {
    pub fn parse(value: &str) -> Result<Self> {
        let rest = value
            .strip_prefix("lm://")
            .ok_or_else(|| anyhow::anyhow!("Resource URI must start with lm://"))?;
        if value.len() > 384 || rest.contains(['?', '#']) {
            bail!("Unsupported resource URI");
        }
        let (device_id, resource) = rest
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("Resource URI needs a device and resource"))?;
        if !valid_hash(device_id) {
            bail!("Invalid LocalMesh device ID");
        }
        if resource.is_empty()
            || resource.len() > 256
            || resource.starts_with('/')
            || resource.ends_with('/')
        {
            bail!("Invalid LocalMesh resource path");
        }
        for segment in resource.split('/') {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.len() > 80
                || !segment
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            {
                bail!("Invalid LocalMesh resource path");
            }
        }
        Ok(Self {
            device_id: device_id.to_owned(),
            resource: resource.to_owned(),
        })
    }

    pub fn format(&self) -> String {
        format!("lm://{}/{}", self.device_id, self.resource)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    pub error: String,
    pub offset: u64,
}
impl Reply {
    pub fn ok(offset: u64) -> Self {
        Self {
            ok: true,
            error: String::new(),
            offset,
        }
    }
    pub fn error(error: impl ToString) -> Self {
        Self {
            ok: false,
            error: error.to_string(),
            offset: 0,
        }
    }
    pub fn check(self) -> Result<u64> {
        if !self.ok {
            bail!("{}", self.error);
        }
        Ok(self.offset)
    }
}

pub async fn write<T: Serialize>(stream: &mut quinn::SendStream, value: &T) -> Result<()> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)?;
    if bytes.len() > MAX_FRAME {
        bail!("Protocol frame is too large");
    }
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

pub async fn read<T: DeserializeOwned>(stream: &mut quinn::RecvStream) -> Result<T> {
    let mut length = [0; 4];
    tokio::time::timeout(
        std::time::Duration::from_secs(130),
        stream.read_exact(&mut length),
    )
    .await??;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        bail!("Invalid protocol frame length");
    }
    let mut data = vec![0; length];
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        stream.read_exact(&mut data),
    )
    .await??;
    let mut reader = std::io::Cursor::new(&data);
    let result = ciborium::from_reader(&mut reader)?;
    if reader.position() as usize != data.len() {
        bail!("Trailing protocol bytes");
    }
    Ok(result)
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 180
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || name.ends_with('.')
        || name.ends_with(' ')
    {
        bail!("Unsafe or unsupported filename");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        bail!("Reserved filename");
    }
    Ok(())
}

pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localmesh_resource_uri_round_trip() {
        let id = "a".repeat(64);
        let uri = ResourceUri::parse(&format!("lm://{id}/screen/main")).unwrap();
        assert_eq!(uri.device_id, id);
        assert_eq!(uri.resource, "screen/main");
        assert_eq!(uri.format(), format!("lm://{}/screen/main", "a".repeat(64)));
    }

    #[test]
    fn localmesh_resource_uri_rejects_unsafe_paths() {
        let id = "b".repeat(64);
        for path in [
            format!("http://{id}/screen"),
            format!("lm://{id}/../screen"),
            format!("lm://{id}/screen//main"),
            format!("lm://{id}/screen?control=true"),
            "lm://not-a-device/screen".to_owned(),
        ] {
            assert!(ResourceUri::parse(&path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn screen_offer_is_cbor_serializable() {
        let offer = ScreenOffer {
            id: "session-1".into(),
            source: "display:0".into(),
            codec: ScreenCodec::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            system_audio: true,
            control: false,
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&offer, &mut bytes).unwrap();
        let decoded: ScreenOffer = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(decoded, offer);
    }
}
