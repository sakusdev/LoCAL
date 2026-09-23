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
      │    ScreenStreamInit
      │    21-byte frame headers + encoded access units
      │
      ├─ optional audio stream (later phase)
      │
      └─ optional control stream (later phase)
```

The existing v1 Text/File request framing remains unchanged. Screen sharing is a negotiated extension so old peers continue to interoperate for v1 features. Discovery capability fields are only hints; the TLS-authenticated Hello is authoritative. Normal builds still advertise only Text/File/Clipboard until a real platform capture or decode backend registers screen capabilities.

## Video transport

The initial codec preference is:

1. H.264 for broad hardware encode/decode support
2. VP9 when the source encoder and viewer decoder both advertise it
3. AV1 when compatible hardware support is available in both required directions

A `ScreenOffer` contains a UUID session ID, a safe LocalMesh resource path such as `screen/display/display-0`, the selected codec, width, height, target FPS, and requested optional features. The receiver must explicitly accept the offer before the source can open video. Pending offers expire after 60 seconds.

Video payloads are not packed into the existing 96 KiB CBOR control frames. After acceptance, the source opens a dedicated ordered QUIC unidirectional stream. The stream begins with a CBOR `ScreenStreamInit` that must exactly match the accepted offer. Encoded access units then use a fixed 21-byte header: 8-byte sequence, 8-byte monotonic microsecond timestamp, 1-byte keyframe flag and 4-byte payload length, all integer fields big-endian.

The encoded payload is limited to 16 MiB and validated before allocation. Frame sequence must strictly increase and timestamps must not move backwards. The QUIC transport allows at most eight concurrent unidirectional streams per connection.

The receiver keeps a four-frame queue. If decoding/rendering falls behind, the oldest queued encoded frame is dropped before the queue can grow further. This intentionally favors freshness over accumulating seconds of latency. A later implementation can make this GOP/keyframe-aware once real encoders are connected.

### Session control

Stop and keyframe-request messages use authenticated bidirectional control streams. Session IDs are UUIDs and are bound to the authenticated peer that created the session. A peer cannot stop, signal or attach video to another peer's session.

The core bounds screen state to eight active/pending sessions and fifty visible session records. Disconnecting a peer, revoking the connection or shutting down LoCAL closes relevant pending/active state and wakes local consumers.

Remote pointer/keyboard events are deliberately not included in this phase. `screen.control` currently participates in capability/offer negotiation only; actual input transport and OS injection remain Phase S4.

## Capture backends

Platform capture belongs outside `local-core`. Each backend implements the `local-screen` encoded-capture boundary and reports real encode capabilities only when its capture/encoder path is available. Decode capability belongs to the viewer/rendering backend and is advertised separately.

### Windows

Preferred path: Windows Graphics Capture with hardware H.264 through Media Foundation where available. Desktop Duplication is a fallback for older or unusual systems.

The v0.3 preview captures displays through Windows Graphics Capture and encodes through a conservative OpenH264 software path. Windows WebView2 and Android WebView decode Annex B H.264 only after runtime codec probing. The viewer requires explicit acceptance and the sender starts capture after that acceptance. Android capture, hardware encoding and two-device visual performance testing remain future work.

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

System audio is negotiated separately from video. A source must advertise `screen.audio` plus `system_audio_capture`; a viewer must advertise `screen.audio` plus `system_audio_playback`. The later audio transport should use Opus at 48 kHz over its own logical transport and timestamps tied to the same monotonic session clock as video. Microphone forwarding is a different resource and must not be enabled implicitly with screen audio.

## Security requirements

- screen sharing requires an already paired TLS-authenticated peer
- incoming offers require explicit acceptance unless a future narrowly scoped remembered permission is deliberately added
- share, view, system audio and control roles are independently advertised and authorized
- session IDs are bound to the authenticated peer and accepted profile
- capture state must be visibly indicated on the source device once a real capture backend exists
- closing the local capture indicator must terminate the remote stream
- no Internet rendezvous or cloud relay is implied by this design
- resource URIs are identifiers, not authorization tokens
- frame lengths, dimensions, FPS and codec metadata are validated before allocation or decode
- control events will be data messages, never operating-system shell strings

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

### Phase S0.75 — session control ✅

- screen offer / accept / reject / stop messages
- dedicated video-stream initialization frame
- fixed bounded encoded-frame transport
- keyframe request control message
- strict session ownership and pairing checks
- bounded pending/active session state and freshness-oriented receive queue
- real two-peer QUIC integration tests for accept, video, keyframe, stop and rejection

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
