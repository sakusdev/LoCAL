# LoCAL screen sharing design

LoCAL treats screen sharing as a local resource, not as a special remote-desktop product. The first target is low-latency view-only sharing on a trusted LAN. Remote pointer, keyboard control and system-audio capture are separate capabilities and permissions.

## Resource model

A device resource uses the LocalMesh URI form:

```text
lm://<64-hex-device-id>/screen/main
lm://<64-hex-device-id>/screen/display/<stable-local-id>
lm://<64-hex-device-id>/screen/window/<stable-local-id>
lm://<64-hex-device-id>/audio/output
lm://<64-hex-device-id>/sensors/gyro
```

The URI identifies a resource only. Trust still comes from the peer TLS identity and stored pairing record; a URI must never bypass pairing or permissions.

The protocol core defines independent screen capabilities:

- `screen.share` — capture and encode a local screen/window
- `screen.view` — decode and display a remote screen stream
- `screen.audio` — system-audio capture and/or playback, as declared by directional metadata
- `screen.control` — accept remote pointer / keyboard control while sharing

Screen metadata is intentionally directional. `encode` describes the codecs, dimensions and FPS the device can produce; `decode` describes what it can consume. A device may support one without the other, or support different codecs in each direction. `control_target`, `system_audio_capture`, and `system_audio_playback` describe endpoint roles independently so the UI never infers control or audio permission from video support.

## Implementation boundary

Screen sharing is deliberately split into two layers:

```text
OS capture API
   ↓
platform capture + hardware encoder
   ↓
local-screen
   ├─ source metadata
   ├─ directional capability/profile negotiation
   ├─ encoded-frame safety validation
   └─ backend/session interfaces
   ↓
local-core / QUIC transport
```

`local-core` never needs to understand Windows GPU textures, PipeWire buffers, ScreenCaptureKit surfaces or Android MediaProjection objects. `local-screen` also does **not** require every backend to copy full RGBA frames through Rust-owned memory. A platform backend should capture and hardware-encode natively where possible, then expose encoded access units.

This boundary exists specifically to keep a future zero-copy or near-zero-copy path possible on Windows and Android. Current `local-screen` negotiation matches the source encoder against the viewer decoder, prefers H.264, clamps resolution/FPS to limits supported by both peers, and caps one encoded access unit at 16 MiB before it enters the transport path.

## Session phases

```text
paired QUIC session
      │
      ├─ authenticated directional capability negotiation
      │
      ├─ enumerate local capture sources
      │
      ├─ screen offer
      │    resource / codec / size / fps / optional audio+control
      │
      ├─ explicit receiver accept
      │
      ├─ dedicated video stream
      │    frame headers + encoded access units
      │
      ├─ optional audio stream
      │
      └─ optional control stream
```

The existing v1 Text/File request framing remains unchanged. Screen sharing is added as a negotiated extension so old peers continue to interoperate for v1 features. Discovery capability fields are only hints; the TLS-authenticated Hello is authoritative.

## Video transport

The initial codec preference is:

1. H.264 for broad hardware encode/decode support
2. VP9 when the source encoder and viewer decoder both advertise it
3. AV1 when compatible hardware support is available in both required directions

A `ScreenOffer` contains a UUID session ID, a safe LocalMesh resource path such as `screen/display/display-0`, the selected codec, width, height, target FPS, and requested optional features. Video payloads must not be packed into the existing 96 KiB CBOR control frames. Instead, screen video uses a dedicated QUIC stream. Each encoded access unit is preceded by a compact `ScreenFrameHeader` containing sequence number, monotonic timestamp, keyframe flag and payload length.

The receiver enforces negotiated maximum resolution, frame rate and encoded-frame limits before allocation. Decoders must reject unreasonable dimensions, backwards frame sequence/timestamps and integer-overflowing lengths.

### Stream reliability policy

For the first implementation LoCAL should use a dedicated unidirectional QUIC stream for encoded video because it is simple, encrypted and ordered. The sender should favor freshness over preserving an ever-growing encoder queue: if capture or encode falls behind, drop frames before they enter QUIC.

A later low-latency mode may use QUIC DATAGRAM for independently decodable chunks or short-GOP frame groups, but this should only be introduced with explicit loss recovery and keyframe-request behavior.

## Capture backends

Platform capture belongs outside `local-core`. Each backend implements the `local-screen` encoded-capture boundary and reports real encode capabilities only when its capture/encoder path is available. Decode capability belongs to the viewer/rendering backend and is advertised separately.

### Windows

Preferred path: Windows Graphics Capture with hardware H.264 through Media Foundation where available. Desktop Duplication is a fallback for older or unusual systems.

### macOS

Preferred path: ScreenCaptureKit, with a native hardware encoder such as VideoToolbox. Screen-recording permission must be requested by the app and failure must remain visible to the user.

### Linux

Preferred path on Wayland: xdg-desktop-portal + PipeWire. On X11, use an explicit X11 capture backend. LoCAL should not silently bypass the desktop portal on Wayland. Encoder choice can follow the hardware available to the system, with a development software fallback where necessary.

### Android

Use MediaProjection. Android must show the operating-system capture consent dialog; LoCAL must never attempt to suppress or work around it. MediaCodec is the preferred hardware encoder/decoder.

## Remote control

Remote control is deliberately later than view-only sharing. `screen.control` means the sharing endpoint can be controlled; it does not imply that every accepted screen session automatically grants control.

Permissions are split conceptually into:

```text
screen.view
screen.control.pointer
screen.control.keyboard
```

The user must explicitly enable control for a paired device. A new control session requires a visible local indicator and a one-click way to terminate it. Sensitive key combinations and text injection need platform-specific handling.

Control messages use normalized coordinates rather than sender pixels so different scaling factors and display sizes do not corrupt pointer position. Keyboard events carry physical key identity plus modifiers; they must not transport arbitrary shell commands.

## Audio

System audio is negotiated separately from video. A source must advertise `screen.audio` plus `system_audio_capture`; a viewer must advertise `screen.audio` plus `system_audio_playback`. It should use Opus at 48 kHz over its own logical transport and timestamps tied to the same monotonic session clock as video. Microphone forwarding is a different resource and must not be enabled implicitly with screen audio.

## Security requirements

- screen sharing requires an already paired TLS-authenticated peer
- incoming offers require explicit acceptance unless the user creates a narrowly scoped remembered permission
- share, view, system audio and control roles are independently advertised and authorized
- capture state must be visibly indicated on the source device
- closing the local capture indicator terminates the remote stream
- no Internet rendezvous or cloud relay is implied by this design
- resource URIs are identifiers, not authorization tokens
- frame lengths, dimensions, FPS and codec metadata are validated before allocation or decode
- control events are data messages, never operating-system shell strings

## Implementation sequence

### Phase S0 — protocol foundation ✅

- capability names in `local-core`
- strict `lm://` resource parser
- codec / screen capability / offer / frame-header wire types
- authenticated capability negotiation with v1 fallback
- protocol and security documentation

### Phase S0.5 — capture boundary ✅

- `local-screen` workspace crate
- source metadata and safe LocalMesh resource paths
- source-encode ↔ viewer-decode H.264 → VP9 → AV1 profile negotiation
- encoded access-unit size/order validation
- platform backend/session interfaces that keep raw GPU surfaces outside `local-core`

### Phase S0.75 — session control

- screen offer / accept / reject / stop messages
- dedicated video-stream initialization frame
- keyframe request control message
- strict session ownership and pairing checks
- bounded pending/active session state

### Phase S1 — desktop view-only prototype

- enumerate displays / windows
- capture one source
- H.264 hardware encode where available, software fallback for development
- receiver decode and render
- explicit offer / accept UI
- adaptive bitrate and FPS caps

### Phase S2 — Android interoperability

- MediaProjection capture
- MediaCodec encode/decode
- Android ↔ desktop view-only sessions
- rotation and resolution-change handling

### Phase S3 — audio

- system-audio capture where the OS permits it
- Opus transport
- A/V timestamp synchronization

### Phase S4 — remote control

- pointer permission and normalized coordinates
- keyboard permission
- local capture/control indicator
- emergency revoke shortcut

### Phase S5 — polish

- window-only sharing
- quality presets
- multi-monitor switching
- session statistics: bitrate, RTT, dropped frames, encode/decode time
- optional low-latency datagram transport after correctness testing

## Non-goals for the first version

The first screen-sharing release is not intended to cross NAT, relay over LoCAL servers, hide capture indicators, bypass OS privacy prompts, or emulate a public remote-support service. Its target is fast, private sharing between devices on a trusted local network.
