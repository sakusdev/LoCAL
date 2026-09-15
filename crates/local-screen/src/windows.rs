use crate::{
    CaptureSource, CaptureSourceKind, EncodedCaptureBackend, EncodedCaptureSession,
    EncodedVideoFrame, NegotiatedScreen, MAX_ENCODED_FRAME,
};
use anyhow::{bail, Context, Result};
use local_core::protocol::{ScreenCodec, ScreenMediaCapabilities};
use openh264::{
    encoder::Encoder,
    formats::{BgraSliceU8, YUVBuffer},
    Timestamp,
};
use std::{
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    time::{Duration, Instant},
};
use windows_capture::{
    capture::{CaptureControl, Context as CaptureContext, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

const SOFTWARE_MAX_WIDTH: u32 = 1920;
const SOFTWARE_MAX_HEIGHT: u32 = 1080;
const SOFTWARE_MAX_FPS: u16 = 30;
const FRAME_QUEUE: usize = 2;

/// Windows Graphics Capture source with an OpenH264 software encoder fallback.
///
/// This intentionally keeps the fallback conservative at 1080p30. A Media
/// Foundation hardware implementation can implement the same
/// `EncodedCaptureBackend` trait without changing `local-core` or the wire
/// protocol.
pub struct WindowsCaptureBackend;

impl WindowsCaptureBackend {
    pub fn new() -> Result<Self> {
        if !GraphicsCaptureApi::is_supported().context("Cannot query Windows Graphics Capture")? {
            bail!("Windows Graphics Capture is not supported on this system");
        }
        Ok(Self)
    }

    fn monitor_for_source(source_id: &str) -> Result<Monitor> {
        let index = source_id
            .strip_prefix("display-")
            .context("Windows capture source is not a display")?
            .parse::<usize>()
            .context("Invalid Windows display source id")?;
        if index == 0 {
            bail!("Windows display index is one-based");
        }
        Monitor::from_index(index).map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn capture_source(monitor: Monitor) -> Result<CaptureSource> {
        let index = monitor
            .index()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let width = monitor
            .width()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let height = monitor
            .height()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let name = monitor
            .name()
            .or_else(|_| monitor.device_name())
            .unwrap_or_else(|_| format!("Display {index}"));
        let primary = Monitor::primary().ok().is_some_and(|item| item == monitor);
        let source = CaptureSource {
            id: format!("display-{index}"),
            name,
            kind: CaptureSourceKind::Display,
            width,
            height,
            primary,
        };
        source.validate()?;
        Ok(source)
    }
}

impl EncodedCaptureBackend for WindowsCaptureBackend {
    fn backend_name(&self) -> &'static str {
        "windows-wgc-openh264"
    }

    fn encode_capabilities(&self) -> ScreenMediaCapabilities {
        ScreenMediaCapabilities {
            codecs: vec![ScreenCodec::H264],
            max_width: SOFTWARE_MAX_WIDTH,
            max_height: SOFTWARE_MAX_HEIGHT,
            max_fps: SOFTWARE_MAX_FPS,
        }
    }

    fn sources(&self) -> Result<Vec<CaptureSource>> {
        let monitors = Monitor::enumerate().map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let sources: Vec<_> = monitors
            .into_iter()
            .filter_map(|monitor| Self::capture_source(monitor).ok())
            .collect();
        if sources.is_empty() {
            bail!("No capturable Windows displays were found");
        }
        Ok(sources)
    }

    fn start(
        &self,
        source_id: &str,
        profile: &NegotiatedScreen,
    ) -> Result<Box<dyn EncodedCaptureSession>> {
        if profile.codec != ScreenCodec::H264 {
            bail!("Windows software fallback currently supports H.264 only");
        }
        if profile.width == 0
            || profile.height == 0
            || !profile.width.is_multiple_of(2)
            || !profile.height.is_multiple_of(2)
            || profile.width > SOFTWARE_MAX_WIDTH
            || profile.height > SOFTWARE_MAX_HEIGHT
            || profile.fps == 0
            || profile.fps > SOFTWARE_MAX_FPS
        {
            bail!("Screen profile exceeds Windows software fallback limits");
        }
        if profile.system_audio || profile.control {
            bail!("Windows software fallback is view-only without system audio");
        }

        let monitor = Self::monitor_for_source(source_id)?;
        let source = Self::capture_source(monitor)?;
        let (tx, rx) = mpsc::sync_channel(FRAME_QUEUE);
        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::Default,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            HandlerFlags {
                tx,
                profile: profile.clone(),
            },
        );
        let control = SoftwareH264Handler::start_free_threaded(settings)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        Ok(Box::new(WindowsCaptureSession {
            source,
            profile: profile.clone(),
            rx,
            control: Some(control),
        }))
    }
}

struct HandlerFlags {
    tx: SyncSender<EncodedVideoFrame>,
    profile: NegotiatedScreen,
}

struct SoftwareH264Handler {
    encoder: Encoder,
    tx: SyncSender<EncodedVideoFrame>,
    profile: NegotiatedScreen,
    sequence: u64,
    started: Instant,
    last_encoded: Option<Instant>,
    compact_bgra: Vec<u8>,
    scaled_bgra: Vec<u8>,
}

impl SoftwareH264Handler {
    fn force_keyframe(&mut self) {
        self.encoder.force_intra_frame();
    }

    fn should_encode(&mut self, now: Instant) -> bool {
        let interval = Duration::from_secs_f64(1.0 / f64::from(self.profile.fps));
        if self
            .last_encoded
            .is_some_and(|last| now.saturating_duration_since(last) < interval)
        {
            return false;
        }
        self.last_encoded = Some(now);
        true
    }

    fn encode_pixels(
        &mut self,
        pixels: &[u8],
        source_width: u32,
        source_height: u32,
        timestamp_us: u64,
    ) -> Result<Option<EncodedVideoFrame>, String> {
        let expected = usize::try_from(source_width)
            .ok()
            .and_then(|width| {
                usize::try_from(source_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "Captured frame dimensions overflow".to_owned())?;
        if pixels.len() != expected {
            return Err("Captured BGRA frame has an unexpected byte length".into());
        }

        let encoded_pixels =
            if source_width == self.profile.width && source_height == self.profile.height {
                pixels
            } else {
                scale_bgra_letterbox(
                    pixels,
                    source_width,
                    source_height,
                    self.profile.width,
                    self.profile.height,
                    &mut self.scaled_bgra,
                )?;
                self.scaled_bgra.as_slice()
            };

        let bgra = BgraSliceU8::new(
            encoded_pixels,
            (self.profile.width as usize, self.profile.height as usize),
        );
        let yuv = YUVBuffer::from_rgb_source(bgra);
        let bitstream = self
            .encoder
            .encode_at(&yuv, Timestamp::from_millis(timestamp_us / 1_000))
            .map_err(|error| error.to_string())?;
        let data = bitstream.to_vec();
        if data.is_empty() {
            return Ok(None);
        }
        if data.len() > MAX_ENCODED_FRAME {
            return Err("OpenH264 produced an oversized access unit".into());
        }
        let frame = EncodedVideoFrame {
            sequence: self.sequence,
            timestamp_us,
            keyframe: annex_b_contains_idr(&data),
            data,
        };
        self.sequence = self.sequence.saturating_add(1);
        Ok(Some(frame))
    }
}

impl GraphicsCaptureApiHandler for SoftwareH264Handler {
    type Flags = HandlerFlags;
    type Error = String;

    fn new(ctx: CaptureContext<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        let encoder = Encoder::new().map_err(|error| error.to_string())?;
        Ok(Self {
            encoder,
            tx: ctx.flags.tx,
            profile: ctx.flags.profile,
            sequence: 0,
            started: Instant::now(),
            last_encoded: None,
            compact_bgra: Vec::new(),
            scaled_bgra: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        let now = Instant::now();
        if !self.should_encode(now) {
            return Ok(());
        }

        let source_width = frame.width();
        let source_height = frame.height();
        let buffer = frame.buffer().map_err(|error| error.to_string())?;
        // `as_nopadding_buffer` may return a slice backed by `compact_bgra`.
        // Own this fallback frame locally before mutably borrowing the encoder
        // state again. The software path already performs CPU color conversion,
        // so this copy keeps the borrow boundary simple and deterministic.
        let pixels = buffer.as_nopadding_buffer(&mut self.compact_bgra).to_vec();
        let timestamp_us = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let Some(encoded) =
            self.encode_pixels(&pixels, source_width, source_height, timestamp_us)?
        else {
            return Ok(());
        };

        match self.tx.try_send(encoded) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                capture_control.stop();
                Ok(())
            }
        }
    }
}

type SoftwareCaptureControl = CaptureControl<SoftwareH264Handler, String>;

struct WindowsCaptureSession {
    source: CaptureSource,
    profile: NegotiatedScreen,
    rx: Receiver<EncodedVideoFrame>,
    control: Option<SoftwareCaptureControl>,
}

impl EncodedCaptureSession for WindowsCaptureSession {
    fn source(&self) -> &CaptureSource {
        &self.source
    }

    fn profile(&self) -> &NegotiatedScreen {
        &self.profile
    }

    fn next_frame(&mut self) -> Result<Option<EncodedVideoFrame>> {
        match self.rx.recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(_) => Ok(None),
        }
    }

    fn request_keyframe(&self) -> Result<()> {
        let control = self
            .control
            .as_ref()
            .context("Capture session is stopped")?;
        control.callback().lock().force_keyframe();
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(control) = self.control.take() {
            control
                .stop()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        }
        Ok(())
    }
}

impl Drop for WindowsCaptureSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn scale_bgra_letterbox(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    target: &mut Vec<u8>,
) -> std::result::Result<(), String> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return Err("Cannot scale a zero-sized capture frame".into());
    }
    let target_len = usize::try_from(target_width)
        .ok()
        .and_then(|width| {
            usize::try_from(target_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Scaled frame dimensions overflow".to_owned())?;
    target.resize(target_len, 0);
    for pixel in target.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }

    let (scaled_width, scaled_height) = if u64::from(target_width) * u64::from(source_height)
        <= u64::from(target_height) * u64::from(source_width)
    {
        let height = (u64::from(source_height) * u64::from(target_width) / u64::from(source_width))
            .max(1) as u32;
        (target_width, height.min(target_height))
    } else {
        let width = (u64::from(source_width) * u64::from(target_height) / u64::from(source_height))
            .max(1) as u32;
        (width.min(target_width), target_height)
    };
    let offset_x = (target_width - scaled_width) / 2;
    let offset_y = (target_height - scaled_height) / 2;

    let source_width_usize = source_width as usize;
    let target_width_usize = target_width as usize;
    for y in 0..scaled_height {
        let source_y = u64::from(y) * u64::from(source_height) / u64::from(scaled_height);
        for x in 0..scaled_width {
            let source_x = u64::from(x) * u64::from(source_width) / u64::from(scaled_width);
            let source_index = ((source_y as usize * source_width_usize) + source_x as usize) * 4;
            let target_x = (offset_x + x) as usize;
            let target_y = (offset_y + y) as usize;
            let target_index = (target_y * target_width_usize + target_x) * 4;
            target[target_index..target_index + 4]
                .copy_from_slice(&source[source_index..source_index + 4]);
        }
    }
    Ok(())
}

fn annex_b_contains_idr(data: &[u8]) -> bool {
    let mut index = 0usize;
    while index + 4 <= data.len() {
        let nal = if data[index..].starts_with(&[0, 0, 0, 1]) {
            index + 4
        } else if data[index..].starts_with(&[0, 0, 1]) {
            index + 3
        } else {
            index += 1;
            continue;
        };
        if nal < data.len() && data[nal] & 0x1f == 5 {
            return true;
        }
        index = nal.saturating_add(1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_idr_detection_handles_three_and_four_byte_start_codes() {
        assert!(annex_b_contains_idr(&[0, 0, 0, 1, 0x65, 1, 2, 3]));
        assert!(annex_b_contains_idr(&[0, 0, 1, 0x65, 1, 2, 3]));
        assert!(!annex_b_contains_idr(&[0, 0, 0, 1, 0x41, 1, 2, 3]));
    }

    #[test]
    fn letterbox_scaler_preserves_target_geometry() {
        let source = vec![255u8; 4 * 4 * 4];
        let mut target = Vec::new();
        scale_bgra_letterbox(&source, 4, 4, 8, 4, &mut target).unwrap();
        assert_eq!(target.len(), 8 * 4 * 4);
        assert_eq!(&target[0..4], &[0, 0, 0, 255]);
        let center = (2 * 8 + 4) * 4;
        assert_eq!(&target[center..center + 4], &[255, 255, 255, 255]);
    }
}
