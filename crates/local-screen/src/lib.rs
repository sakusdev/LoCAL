//! Platform-neutral screen-sharing boundary for LoCAL.
//!
//! Capture and hardware encoding stay in platform backends. This crate only
//! negotiates a safe profile and moves already encoded video access units toward
//! the LocalMesh transport layer, avoiding mandatory GPU -> CPU readback.

use anyhow::{bail, Context, Result};
use local_core::protocol::{
    ScreenCapabilities, ScreenCodec, ScreenFrameHeader, ScreenMediaCapabilities,
};
use serde::{Deserialize, Serialize};

#[cfg(target_os = "windows")]
pub mod windows;

pub const MAX_ENCODED_FRAME: usize = 16 * 1024 * 1024;
pub const CODEC_PREFERENCE: &[ScreenCodec] =
    &[ScreenCodec::H264, ScreenCodec::Vp9, ScreenCodec::Av1];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSourceKind {
    Display,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSource {
    /// Stable for the lifetime of the app and safe as one `lm://` path segment.
    pub id: String,
    pub name: String,
    pub kind: CaptureSourceKind,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl CaptureSource {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > 80
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        {
            bail!("Invalid capture source id");
        }
        if self.name.is_empty()
            || self.name.len() > 160
            || self.name.chars().any(char::is_control)
            || self.width == 0
            || self.height == 0
            || self.width > 16_384
            || self.height > 16_384
        {
            bail!("Invalid capture source metadata");
        }
        Ok(())
    }

    pub fn resource_path(&self) -> String {
        match self.kind {
            CaptureSourceKind::Display => format!("screen/display/{}", self.id),
            CaptureSourceKind::Window => format!("screen/window/{}", self.id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRequest {
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u16,
    pub system_audio: bool,
    pub control: bool,
}

impl Default for ScreenRequest {
    fn default() -> Self {
        Self {
            max_width: 1920,
            max_height: 1080,
            max_fps: 60,
            system_audio: false,
            control: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedScreen {
    pub codec: ScreenCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u16,
    pub system_audio: bool,
    pub control: bool,
}

fn sane(capabilities: &ScreenMediaCapabilities) -> bool {
    !capabilities.codecs.is_empty()
        && capabilities.codecs.len() <= 4
        && capabilities.max_width > 0
        && capabilities.max_width <= 16_384
        && capabilities.max_height > 0
        && capabilities.max_height <= 16_384
        && capabilities.max_fps > 0
        && capabilities.max_fps <= 240
}

fn even_dimension(value: u32) -> u32 {
    value & !1
}

/// Chooses a profile supported by the source encoder and viewer decoder.
///
/// H.264 wins when available because the initial LoCAL target is broad hardware
/// interoperability. Optional audio/control requests gracefully negotiate down
/// to `false` rather than making view-only sharing fail. Dimensions are rounded
/// down to even values because the initial H.264/YUV420 path requires even
/// chroma geometry.
pub fn negotiate(
    source: &ScreenCapabilities,
    viewer: &ScreenCapabilities,
    request: &ScreenRequest,
) -> Result<NegotiatedScreen> {
    let encode = source
        .encode
        .as_ref()
        .context("Source cannot encode screen video")?;
    let decode = viewer
        .decode
        .as_ref()
        .context("Viewer cannot decode screen video")?;
    if !sane(encode)
        || !sane(decode)
        || request.max_width == 0
        || request.max_height == 0
        || request.max_fps == 0
    {
        bail!("Invalid screen negotiation limits");
    }
    let codec = CODEC_PREFERENCE
        .iter()
        .copied()
        .find(|codec| encode.codecs.contains(codec) && decode.codecs.contains(codec))
        .ok_or_else(|| anyhow::anyhow!("No common screen codec"))?;
    let width = even_dimension(
        request
            .max_width
            .min(encode.max_width)
            .min(decode.max_width),
    );
    let height = even_dimension(
        request
            .max_height
            .min(encode.max_height)
            .min(decode.max_height),
    );
    if width == 0 || height == 0 {
        bail!("Negotiated screen size is too small");
    }
    Ok(NegotiatedScreen {
        codec,
        width,
        height,
        fps: request.max_fps.min(encode.max_fps).min(decode.max_fps),
        system_audio: request.system_audio
            && source.system_audio_capture
            && viewer.system_audio_playback,
        control: request.control && source.control_target,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVideoFrame {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

impl EncodedVideoFrame {
    pub fn header(&self) -> Result<ScreenFrameHeader> {
        if self.data.is_empty() || self.data.len() > MAX_ENCODED_FRAME {
            bail!("Encoded screen frame has invalid length");
        }
        Ok(ScreenFrameHeader {
            sequence: self.sequence,
            timestamp_us: self.timestamp_us,
            keyframe: self.keyframe,
            payload_len: u32::try_from(self.data.len())?,
        })
    }
}

#[derive(Debug, Default)]
pub struct FrameValidator {
    last_sequence: Option<u64>,
    last_timestamp_us: Option<u64>,
}

impl FrameValidator {
    pub fn validate(&mut self, header: &ScreenFrameHeader) -> Result<()> {
        if header.payload_len == 0 || header.payload_len as usize > MAX_ENCODED_FRAME {
            bail!("Encoded screen frame exceeds negotiated safety limit");
        }
        if self
            .last_sequence
            .is_some_and(|last| header.sequence <= last)
        {
            bail!("Screen frame sequence moved backwards");
        }
        if self
            .last_timestamp_us
            .is_some_and(|last| header.timestamp_us < last)
        {
            bail!("Screen frame timestamp moved backwards");
        }
        self.last_sequence = Some(header.sequence);
        self.last_timestamp_us = Some(header.timestamp_us);
        Ok(())
    }
}

/// Platform implementation contract. A backend should use native capture and,
/// where possible, native hardware encoding before returning frames here.
pub trait EncodedCaptureBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn encode_capabilities(&self) -> ScreenMediaCapabilities;
    fn system_audio_capture(&self) -> bool {
        false
    }
    fn sources(&self) -> Result<Vec<CaptureSource>>;
    fn start(
        &self,
        source_id: &str,
        profile: &NegotiatedScreen,
    ) -> Result<Box<dyn EncodedCaptureSession>>;
}

pub trait EncodedCaptureSession: Send {
    fn source(&self) -> &CaptureSource;
    fn profile(&self) -> &NegotiatedScreen;
    /// Returns the next encoded access unit, or `None` when capture ended.
    fn next_frame(&mut self) -> Result<Option<EncodedVideoFrame>>;
    /// Requests an intra frame so a receiver can recover after loss or startup.
    fn request_keyframe(&self) -> Result<()> {
        bail!("Capture backend does not support keyframe requests")
    }
    fn stop(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(codecs: Vec<ScreenCodec>) -> ScreenMediaCapabilities {
        ScreenMediaCapabilities {
            codecs,
            max_width: 3840,
            max_height: 2160,
            max_fps: 120,
        }
    }

    fn source(codecs: Vec<ScreenCodec>) -> ScreenCapabilities {
        ScreenCapabilities {
            encode: Some(media(codecs)),
            decode: None,
            control_target: true,
            system_audio_capture: true,
            system_audio_playback: false,
        }
    }

    fn viewer(codecs: Vec<ScreenCodec>) -> ScreenCapabilities {
        ScreenCapabilities {
            encode: None,
            decode: Some(media(codecs)),
            control_target: false,
            system_audio_capture: false,
            system_audio_playback: true,
        }
    }

    #[test]
    fn negotiation_prefers_h264_and_clamps_limits() {
        let source = source(vec![ScreenCodec::Av1, ScreenCodec::H264]);
        let mut viewer = viewer(vec![ScreenCodec::H264, ScreenCodec::Vp9]);
        viewer.decode.as_mut().unwrap().max_width = 2560;
        viewer.decode.as_mut().unwrap().max_fps = 60;
        viewer.system_audio_playback = false;
        let request = ScreenRequest {
            max_width: 1920,
            max_height: 1080,
            max_fps: 90,
            system_audio: true,
            control: true,
        };
        let profile = negotiate(&source, &viewer, &request).unwrap();
        assert_eq!(profile.codec, ScreenCodec::H264);
        assert_eq!(
            (profile.width, profile.height, profile.fps),
            (1920, 1080, 60)
        );
        assert!(!profile.system_audio);
        assert!(profile.control);
    }

    #[test]
    fn negotiation_rounds_dimensions_for_yuv420() {
        let profile = negotiate(
            &source(vec![ScreenCodec::H264]),
            &viewer(vec![ScreenCodec::H264]),
            &ScreenRequest {
                max_width: 1919,
                max_height: 1079,
                max_fps: 30,
                system_audio: false,
                control: false,
            },
        )
        .unwrap();
        assert_eq!((profile.width, profile.height), (1918, 1078));
    }

    #[test]
    fn negotiation_requires_encode_decode_roles_and_common_codec() {
        let source = source(vec![ScreenCodec::Av1]);
        let viewer = viewer(vec![ScreenCodec::H264]);
        assert!(negotiate(&source, &viewer, &ScreenRequest::default()).is_err());
        assert!(negotiate(&viewer, &viewer, &ScreenRequest::default()).is_err());
    }

    #[test]
    fn capture_source_builds_safe_resource_path() {
        let source = CaptureSource {
            id: "display-0".into(),
            name: "Main display".into(),
            kind: CaptureSourceKind::Display,
            width: 2560,
            height: 1440,
            primary: true,
        };
        source.validate().unwrap();
        assert_eq!(source.resource_path(), "screen/display/display-0");
        let mut invalid = source;
        invalid.id = "../secret".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn frame_header_and_order_are_bounded() {
        let frame = EncodedVideoFrame {
            sequence: 7,
            timestamp_us: 42_000,
            keyframe: true,
            data: vec![1, 2, 3],
        };
        let header = frame.header().unwrap();
        assert_eq!(header.payload_len, 3);
        let mut validator = FrameValidator::default();
        validator.validate(&header).unwrap();
        assert!(validator.validate(&header).is_err());
        assert!(validator
            .validate(&ScreenFrameHeader {
                sequence: 8,
                timestamp_us: 41_999,
                keyframe: false,
                payload_len: 1,
            })
            .is_err());
    }
}
