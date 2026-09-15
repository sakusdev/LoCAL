# LoCAL screen sharing design

LoCAL treats screen sharing as a local resource, not as a special remote-desktop product. The first target is low-latency view-only sharing on a trusted LAN. Remote pointer, keyboard control and system-audio capture are separate capabilities and permissions.

## Resource model

A device resource uses the LocalMesh URI form:

```text
lm://<64-hex-device-id>/screen/main
lm://<64-hex-device-id>/screen/window/<stable-local-id>
lm://<64-hex-device-id>/audio/output
lm://<64-hex-device-id>/sensors/gyro
```

The URI identifies a resource only. Trust still comes from the peer TLS identity and stored pairing record; a URI must never bypass pairing or permissions.

The protocol core defines independent capabilities:

- `screen.view` — receive encoded screen video
- `screen.audio` — receive captured system audio with a screen session
- `screen.control` — send remote pointer / keyboard input

A peer may expose `screen.view` without either of the other capabilities. UI must never infer control permission from view permission.

## Session phases

```text
paired QUIC session
      │
      ├─ capability negotiation
      │
      ├─ screen offer
      │    source / codec / size / fps
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

The existing v1 Text/File request framing remains unchanged. Screen sharing is added as a negotiated extension so old peers continue to interoperate for v1 features.

## Video transport

The initial codec preference is:

1. H.264 for broad hardware encode/decode support
2. VP9 when both peers advertise it
3. AV1 when hardware support is available on both ends

A `ScreenOffer` declares the selected source, codec, width, height and target FPS. Video payloads must not be packed into the existing 96 KiB CBOR control frames. Instead, screen video uses a dedicated QUIC stream. Each encoded access unit is preceded by a compact `ScreenFrameHeader` containing sequence number, monotonic timestamp, keyframe flag and payload length.

The receiver enforces negotiated maximum resolution, frame rate and encoded-frame limits before allocation. Decoders must reject unreasonable dimensions and integer-overflowing lengths.

### Stream reliability policy

For the first implementation LoCAL should use a dedicated unidirectional QUIC stream for encoded video because it is simple, encrypted and ordered. The sender should favor freshness over preserving an ever-growing encoder queue: if capture or encode falls behind, drop frames before they enter QUIC.

A later low-latency mode may use QUIC DATAGRAM for independently decodable chunks or short-GOP frame groups, but this should only be introduced with explicit loss recovery and keyframe-request behavior.

## Capture backends

Platform capture belongs outside `local-core`.

### Windows

Preferred path: Windows Graphics Capture with hardware H.264 through Media Foundation where available. Desktop Duplication is a fallback for older or unusual systems.

### macOS

Preferred path: ScreenCaptureKit. Screen-recording permission must be requested by the app and failure must remain visible to the user.

### Linux

Preferred path on Wayland: xdg-desktop-portal + PipeWire. On X11, use an explicit X11 capture backend. LoCAL should not silently bypass the desktop portal on Wayland.

### Android

Use MediaProjection. Android must show the operating-system capture consent dialog; LoCAL must never attempt to suppress or work around it. MediaCodec is the preferred hardware encoder.

## Remote control

Remote control is deliberately later than view-only sharing.

Permissions are split into:

```text
screen.view
screen.control.pointer
screen.control.keyboard
```

The user must explicitly enable control for a paired device. A new control session requires a visible local indicator and a one-click way to terminate it. Sensitive key combinations and text injection need platform-specific handling.

Control messages use normalized coordinates rather than sender pixels so different scaling factors and display sizes do not corrupt pointer position. Keyboard events carry physical key identity plus modifiers; they must not transport arbitrary shell commands.

## Audio

System audio is a separate `screen.audio` capability. It should use Opus at 48 kHz over its own logical transport and timestamps tied to the same monotonic session clock as video. Microphone forwarding is a different resource and must not be enabled implicitly with screen audio.

## Security requirements

- screen sharing requires an already paired TLS-authenticated peer
- incoming offers require explicit acceptance unless the user creates a narrowly scoped remembered permission
- `screen.view`, system audio and control are independent permissions
- capture state must be visibly indicated on the source device
- closing the local capture indicator terminates the remote stream
- no Internet rendezvous or cloud relay is implied by this design
- resource URIs are identifiers, not authorization tokens
- frame lengths, dimensions, FPS and codec metadata are validated before allocation or decode
- control events are data messages, never operating-system shell strings

## Implementation sequence

### Phase S0 — protocol foundation

- capability names in `local-core`
- strict `lm://` resource parser
- codec / screen capability / offer / frame-header wire types
- protocol and security documentation

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
