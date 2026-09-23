use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashSet;

pub const VERSION: u8 = 1;
pub const MAX_FRAME: usize = 96 * 1024;
pub const MAX_TEXT: usize = 16 * 1024;
pub const MAX_AUDIO_SIGNAL: usize = 64 * 1024;
pub const MAX_FILE: u64 = 20 * 1024 * 1024 * 1024;
pub const CHUNK: usize = 1024 * 1024;
pub const MAX_CAPABILITIES: usize = 32;
pub const MAX_SCREEN_FRAME: usize = 16 * 1024 * 1024;
pub const SCREEN_FRAME_HEADER: usize = 21;

pub const CAP_TEXT: &str = "text";
pub const CAP_FILE: &str = "file";
pub const CAP_CLIPBOARD: &str = "clipboard";
pub const CAP_SCREEN_SHARE: &str = "screen.share";
pub const CAP_SCREEN_VIEW: &str = "screen.view";
pub const CAP_SCREEN_CONTROL: &str = "screen.control";
pub const CAP_SCREEN_AUDIO: &str = "screen.audio";
pub const CAP_AUDIO: &str = "audio";
pub const CAP_SENSOR: &str = "sensor";
pub const BASE_CAPABILITIES: &[&str] = &[CAP_TEXT, CAP_FILE, CAP_CLIPBOARD];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u8,
    pub id: String,
    pub name: String,
    pub port: u16,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<ScreenCapabilities>,
}

impl Hello {
    /// v1 peers created before capability advertisement omit the field entirely.
    /// Those peers are known to implement the original Text/File/Clipboard MVP.
    pub fn effective_capabilities(&self) -> Vec<String> {
        if self.version == VERSION && self.capabilities.is_empty() {
            BASE_CAPABILITIES
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        } else {
            self.capabilities.clone()
        }
    }
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
    ScreenOffer {
        offer: ScreenOffer,
    },
    ScreenSignal {
        id: String,
        signal: ScreenSignal,
    },
    AudioSignal {
        call_id: String,
        kind: String,
        data: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenCodec {
    H264,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenMediaCapabilities {
    pub codecs: Vec<ScreenCodec>,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode: Option<ScreenMediaCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode: Option<ScreenMediaCapabilities>,
    #[serde(default)]
    pub control_target: bool,
    #[serde(default)]
    pub system_audio_capture: bool,
    #[serde(default)]
    pub system_audio_playback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenOffer {
    pub id: String,
    /// LocalMesh resource path, for example `screen/display/display-0`.
    pub resource: String,
    pub codec: ScreenCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u16,
    pub system_audio: bool,
    pub control: bool,
}

impl ScreenOffer {
    pub fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.id).is_err()
            || !valid_resource_path(&self.resource)
            || !self.resource.starts_with("screen/")
            || self.width == 0
            || self.height == 0
            || self.width > 16_384
            || self.height > 16_384
            || self.fps == 0
            || self.fps > 240
        {
            bail!("Invalid screen offer");
        }
        Ok(())
    }

    pub fn stream_init(&self) -> ScreenStreamInit {
        ScreenStreamInit {
            id: self.id.clone(),
            codec: self.codec,
            width: self.width,
            height: self.height,
            fps: self.fps,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenSignal {
    Stop,
    RequestKeyframe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenStreamInit {
    pub id: String,
    pub codec: ScreenCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u16,
}

impl ScreenStreamInit {
    pub fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.id).is_err()
            || self.width == 0
            || self.height == 0
            || self.width > 16_384
            || self.height > 16_384
            || self.fps == 0
            || self.fps > 240
        {
            bail!("Invalid screen stream initialization");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrameHeader {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub payload_len: u32,
}

impl ScreenFrameHeader {
    pub fn validate(&self) -> Result<()> {
        if self.payload_len == 0 || self.payload_len as usize > MAX_SCREEN_FRAME {
            bail!("Invalid encoded screen frame length");
        }
        Ok(())
    }
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
        if value.len() > 384 || rest.contains('?') || rest.contains('#') {
            bail!("Unsupported resource URI");
        }
        let (device_id, resource) = rest
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("Resource URI needs a device and resource"))?;
        if !valid_hash(device_id) {
            bail!("Invalid LocalMesh device ID");
        }
        if !valid_resource_path(resource) {
            bail!("Invalid LocalMesh resource path");
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
}
impl Reply {
    pub fn ok(offset: u64) -> Self {
        Self {
            ok: true,
            error: String::new(),
            offset,
            accepted: None,
        }
    }
    pub fn screen(accepted: bool) -> Self {
        Self {
            ok: true,
            error: String::new(),
            offset: 0,
            accepted: Some(accepted),
        }
    }
    pub fn error(error: impl ToString) -> Self {
        Self {
            ok: false,
            error: error.to_string(),
            offset: 0,
            accepted: None,
        }
    }
    pub fn check(self) -> Result<u64> {
        if !self.ok {
            bail!("{}", self.error);
        }
        Ok(self.offset)
    }
    pub fn screen_decision(self) -> Result<bool> {
        if !self.ok {
            bail!("{}", self.error);
        }
        self.accepted.context("Missing screen decision")
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

pub fn encode_screen_frame_header(header: &ScreenFrameHeader) -> Result<[u8; SCREEN_FRAME_HEADER]> {
    header.validate()?;
    let mut bytes = [0u8; SCREEN_FRAME_HEADER];
    bytes[0..8].copy_from_slice(&header.sequence.to_be_bytes());
    bytes[8..16].copy_from_slice(&header.timestamp_us.to_be_bytes());
    bytes[16] = u8::from(header.keyframe);
    bytes[17..21].copy_from_slice(&header.payload_len.to_be_bytes());
    Ok(bytes)
}

pub fn decode_screen_frame_header(bytes: &[u8; SCREEN_FRAME_HEADER]) -> Result<ScreenFrameHeader> {
    if bytes[16] > 1 {
        bail!("Invalid screen frame keyframe flag");
    }
    let header = ScreenFrameHeader {
        sequence: u64::from_be_bytes(bytes[0..8].try_into()?),
        timestamp_us: u64::from_be_bytes(bytes[8..16].try_into()?),
        keyframe: bytes[16] == 1,
        payload_len: u32::from_be_bytes(bytes[17..21].try_into()?),
    };
    header.validate()?;
    Ok(header)
}

pub async fn write_screen_frame(
    stream: &mut quinn::SendStream,
    header: &ScreenFrameHeader,
    payload: &[u8],
) -> Result<()> {
    if payload.len() != header.payload_len as usize {
        bail!("Screen frame payload length does not match header");
    }
    let bytes = encode_screen_frame_header(header)?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        stream.write_all(&bytes).await?;
        stream.write_all(payload).await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Screen video write timed out")??;
    Ok(())
}

pub async fn read_screen_frame(
    stream: &mut quinn::RecvStream,
) -> Result<Option<(ScreenFrameHeader, Vec<u8>)>> {
    let mut raw = [0u8; SCREEN_FRAME_HEADER];
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream.read(&mut raw[..1]),
    )
    .await
    .context("Screen video read timed out")??;
    let Some(first) = first else {
        return Ok(None);
    };
    if first != 1 {
        bail!("Invalid screen stream read");
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        stream.read_exact(&mut raw[1..]),
    )
    .await
    .context("Screen frame header timed out")??;
    let header = decode_screen_frame_header(&raw)?;
    let mut payload = vec![0u8; header.payload_len as usize];
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream.read_exact(&mut payload),
    )
    .await
    .context("Screen frame payload timed out")??;
    Ok(Some((header, payload)))
}

pub fn valid_capability(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 || value.starts_with('.') || value.ends_with('.') {
        return false;
    }
    value.split('.').all(|segment| {
        !segment.is_empty()
            && segment.len() <= 32
            && segment
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    })
}

fn validate_screen_media(media: &ScreenMediaCapabilities) -> bool {
    if media.codecs.is_empty()
        || media.codecs.len() > 4
        || media.max_width == 0
        || media.max_height == 0
        || media.max_width > 16_384
        || media.max_height > 16_384
        || media.max_fps == 0
        || media.max_fps > 240
    {
        return false;
    }
    let mut codecs = HashSet::with_capacity(media.codecs.len());
    media.codecs.iter().all(|codec| codecs.insert(*codec))
}

pub fn validate_capabilities(values: &[String], screen: Option<&ScreenCapabilities>) -> Result<()> {
    if values.len() > MAX_CAPABILITIES {
        bail!("Too many advertised capabilities");
    }
    let mut seen = HashSet::with_capacity(values.len());
    for value in values {
        if !valid_capability(value) || !seen.insert(value.as_str()) {
            bail!("Invalid or duplicate capability");
        }
    }
    let has = |capability: &str| values.iter().any(|value| value == capability);
    let has_screen_capability = has(CAP_SCREEN_SHARE)
        || has(CAP_SCREEN_VIEW)
        || has(CAP_SCREEN_CONTROL)
        || has(CAP_SCREEN_AUDIO);
    match screen {
        None if has_screen_capability => bail!("Screen capability metadata is missing"),
        None => {}
        Some(screen) => {
            if screen.encode.is_none() && screen.decode.is_none() {
                bail!("Screen metadata has no encode or decode role");
            }
            if let Some(encode) = &screen.encode {
                if !has(CAP_SCREEN_SHARE) || !validate_screen_media(encode) {
                    bail!("Invalid screen encode capability advertisement");
                }
            } else if has(CAP_SCREEN_SHARE) {
                bail!("screen.share requires encode metadata");
            }
            if let Some(decode) = &screen.decode {
                if !has(CAP_SCREEN_VIEW) || !validate_screen_media(decode) {
                    bail!("Invalid screen decode capability advertisement");
                }
            } else if has(CAP_SCREEN_VIEW) {
                bail!("screen.view requires decode metadata");
            }
            if screen.control_target && (!has(CAP_SCREEN_CONTROL) || screen.encode.is_none()) {
                bail!("screen.control requires a controllable share endpoint");
            }
            if screen.system_audio_capture && (!has(CAP_SCREEN_AUDIO) || screen.encode.is_none()) {
                bail!("screen.audio capture requires a share endpoint");
            }
            if screen.system_audio_playback && (!has(CAP_SCREEN_AUDIO) || screen.decode.is_none()) {
                bail!("screen.audio playback requires a view endpoint");
            }
        }
    }
    Ok(())
}

pub fn screen_capability_names(screen: Option<&ScreenCapabilities>) -> Vec<String> {
    let mut values: Vec<String> = BASE_CAPABILITIES
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let Some(screen) = screen else {
        return values;
    };
    if screen.encode.is_some() {
        values.push(CAP_SCREEN_SHARE.into());
    }
    if screen.decode.is_some() {
        values.push(CAP_SCREEN_VIEW.into());
    }
    if screen.control_target {
        values.push(CAP_SCREEN_CONTROL.into());
    }
    if screen.system_audio_capture || screen.system_audio_playback {
        values.push(CAP_SCREEN_AUDIO.into());
    }
    values
}

pub fn validate_screen_offer(
    source: &ScreenCapabilities,
    viewer: &ScreenCapabilities,
    offer: &ScreenOffer,
) -> Result<()> {
    offer.validate()?;
    let encode = source
        .encode
        .as_ref()
        .context("Screen source does not advertise an encoder")?;
    let decode = viewer
        .decode
        .as_ref()
        .context("Screen viewer does not advertise a decoder")?;
    if !encode.codecs.contains(&offer.codec)
        || !decode.codecs.contains(&offer.codec)
        || offer.width > encode.max_width
        || offer.width > decode.max_width
        || offer.height > encode.max_height
        || offer.height > decode.max_height
        || offer.fps > encode.max_fps
        || offer.fps > decode.max_fps
        || (offer.system_audio && (!source.system_audio_capture || !viewer.system_audio_playback))
        || (offer.control && !source.control_target)
    {
        bail!("Screen offer exceeds negotiated capabilities");
    }
    Ok(())
}

pub fn valid_resource_path(resource: &str) -> bool {
    if resource.is_empty()
        || resource.len() > 256
        || resource.starts_with('/')
        || resource.ends_with('/')
    {
        return false;
    }
    resource.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.len() <= 80
            && segment
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    })
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

    fn media(codecs: Vec<ScreenCodec>) -> ScreenMediaCapabilities {
        ScreenMediaCapabilities {
            codecs,
            max_width: 1920,
            max_height: 1080,
            max_fps: 60,
        }
    }

    fn share_caps() -> ScreenCapabilities {
        ScreenCapabilities {
            encode: Some(media(vec![ScreenCodec::H264, ScreenCodec::Av1])),
            decode: None,
            control_target: true,
            system_audio_capture: true,
            system_audio_playback: false,
        }
    }

    fn view_caps() -> ScreenCapabilities {
        ScreenCapabilities {
            encode: None,
            decode: Some(media(vec![ScreenCodec::H264, ScreenCodec::Vp9])),
            control_target: false,
            system_audio_capture: false,
            system_audio_playback: true,
        }
    }

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
    fn screen_offer_is_valid_and_cbor_serializable() {
        let offer = ScreenOffer {
            id: uuid::Uuid::new_v4().to_string(),
            resource: "screen/display/display-0".into(),
            codec: ScreenCodec::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            system_audio: true,
            control: false,
        };
        offer.validate().unwrap();
        validate_screen_offer(&share_caps(), &view_caps(), &offer).unwrap();
        let mut bytes = Vec::new();
        ciborium::into_writer(&offer, &mut bytes).unwrap();
        let decoded: ScreenOffer = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(decoded, offer);
    }

    #[test]
    fn screen_offer_rejects_unnegotiated_features() {
        let mut offer = ScreenOffer {
            id: uuid::Uuid::new_v4().to_string(),
            resource: "screen/display/display-0".into(),
            codec: ScreenCodec::Vp9,
            width: 1920,
            height: 1080,
            fps: 60,
            system_audio: false,
            control: false,
        };
        assert!(validate_screen_offer(&share_caps(), &view_caps(), &offer).is_err());
        offer.codec = ScreenCodec::H264;
        offer.width = 3840;
        assert!(validate_screen_offer(&share_caps(), &view_caps(), &offer).is_err());
    }

    #[test]
    fn screen_frame_header_has_fixed_binary_layout() {
        let header = ScreenFrameHeader {
            sequence: 7,
            timestamp_us: 42_000,
            keyframe: true,
            payload_len: 1234,
        };
        let bytes = encode_screen_frame_header(&header).unwrap();
        assert_eq!(bytes.len(), SCREEN_FRAME_HEADER);
        assert_eq!(decode_screen_frame_header(&bytes).unwrap(), header);
        let mut invalid = bytes;
        invalid[16] = 2;
        assert!(decode_screen_frame_header(&invalid).is_err());
    }

    #[test]
    fn legacy_hello_defaults_to_mvp_capabilities() {
        let id = "c".repeat(64);
        let json = format!(r#"{{"version":1,"id":"{id}","name":"Old peer","port":53319}}"#);
        let hello: Hello = serde_json::from_str(&json).unwrap();
        assert_eq!(
            hello.effective_capabilities(),
            vec!["text", "file", "clipboard"]
        );
        assert!(hello.screen.is_none());
    }

    #[test]
    fn screen_roles_require_matching_capabilities() {
        let screen = ScreenCapabilities {
            encode: Some(media(vec![ScreenCodec::H264])),
            decode: Some(media(vec![ScreenCodec::H264, ScreenCodec::Vp9])),
            control_target: true,
            system_audio_capture: true,
            system_audio_playback: true,
        };
        let capabilities = vec![
            CAP_SCREEN_SHARE.into(),
            CAP_SCREEN_VIEW.into(),
            CAP_SCREEN_CONTROL.into(),
            CAP_SCREEN_AUDIO.into(),
        ];
        validate_capabilities(&capabilities, Some(&screen)).unwrap();
        assert!(validate_capabilities(&[CAP_SCREEN_VIEW.into()], Some(&screen)).is_err());
        let duplicate = ScreenCapabilities {
            encode: Some(media(vec![ScreenCodec::H264, ScreenCodec::H264])),
            decode: None,
            control_target: false,
            system_audio_capture: false,
            system_audio_playback: false,
        };
        assert!(validate_capabilities(&[CAP_SCREEN_SHARE.into()], Some(&duplicate)).is_err());
    }

    #[test]
    fn capability_validation_rejects_duplicates_and_screen_without_metadata() {
        assert!(validate_capabilities(&["text".into(), "text".into()], None).is_err());
        assert!(!valid_capability("Screen.View"));
        assert!(!valid_capability("screen..view"));
        assert!(validate_capabilities(&[CAP_SCREEN_SHARE.into()], None).is_err());
        validate_capabilities(
            &screen_capability_names(Some(&share_caps())),
            Some(&share_caps()),
        )
        .unwrap();
    }
}
