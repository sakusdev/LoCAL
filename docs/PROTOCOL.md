# LocalMesh protocol v1 in LoCAL

The Text + File MVP shares a Rust core between the CLI, Tauri desktop shell and Android JNI shell. Android uses a bundled offline WebView UI rather than a separate Compose UI. No remote frontend assets are loaded.

## Discovery and identity

JSON UDP beacons on port 53318 every 3 seconds use IPv4 broadcast and supplementary multicast `239.255.53.18`. Fields: `magic=LOCAL_DISCOVERY`, version, device ID, name, listening port and an optional capability hint list. Source IP comes from the socket. Peers expire after 20 seconds; trusted entries remain. Discovery metadata, including capabilities, is **untrusted** until TLS identity is checked. Manual IPv4:port works without discovery. DNS-SD/mDNS, IPv6, QR, Internet rendezvous and relays are not implemented.

Each device persists an Ed25519 private key and self-signed certificate. Identity is the full BLAKE3 hash of the public key. Quinn/rustls uses QUIC + TLS 1.3, ALPN `localmesh/1`, required client certificates. Certificate format and handshake signatures are verified; public CA chains and DNS names do not define trust. The certificate key must match the expected/discovered ID.

The first bidirectional stream exchanges `{version,id,name,port,capabilities,screen?}`. `capabilities` and `screen` are additive serde fields: older v1 implementations omit them, while older decoders ignore the additional fields. A missing capability list from a v1 peer is interpreted as the original MVP set: `text`, `file`, `clipboard`.

A six-digit code is derived from 32 bytes of TLS exporter material with label `LoCAL pairing v1` and empty context. Users compare codes on both devices. Each side exchanges `Confirm`; BOTH local and remote confirmation are required before data requests or persisted trust. Known peers confirm existing stored trust automatically. Pairing expires after 120 seconds. Forgetting a peer requires confirmation again on reconnect.

## Capability negotiation

Authenticated Hello metadata is the source of truth for peer capabilities. LAN discovery can show capability hints before connecting, but the UI marks them as unauthenticated and they must never authorize a feature.

Current vocabulary:

- `text`
- `file`
- `clipboard`
- `screen.share` — the device can capture and encode a local screen/window
- `screen.view` — the device can decode and display a remote screen stream
- `screen.audio` — system-audio capture and/or playback is available as declared in screen metadata
- `screen.control` — the device can act as a remote-control target
- `audio`
- `sensor`

Capability names are lowercase dot-separated tokens, limited to 64 bytes each and 32 entries per peer. Duplicates and malformed values are rejected during the authenticated handshake.

Screen metadata is directional. `encode` describes codecs and limits available when this device shares a screen; `decode` describes codecs and limits available when this device views one. `screen.share` requires valid encode metadata and `screen.view` requires valid decode metadata. This matters on mobile and older GPUs, where encode and decode codec support can differ.

`control_target` requires `screen.control` and an encode/share role. `system_audio_capture` requires `screen.audio` plus an encode role; `system_audio_playback` requires `screen.audio` plus a decode role. Duplicate codecs, zero dimensions/FPS, dimensions above 16384, FPS above 240, or inconsistent role/capability combinations are rejected.

Audio-call and screen capabilities are registered by the local UI only after the required WebRTC/WebCodecs backend is available and before connecting peers. CLI-only nodes therefore do not advertise media features they cannot use.

The local state snapshot exposes both `capabilities` and `capabilities_authenticated`. Connected peers always use the TLS-authenticated Hello values, preventing a forged discovery beacon from overriding a live session.

## Framing

Normal operations open separate QUIC bidirectional streams. CBOR frames use a four-byte unsigned big-endian length, capped at 96KiB. Trailing bytes in the CBOR frame are rejected. Tagged requests:

- `confirm`
- `message`: UUID `id`, `channel` (`mesh.text` or `mesh.clipboard`), UTF-8 `text` (1–16384 bytes)
- `file`: UUID `id`, safe basename `name`, `size` (0–20GiB), BLAKE3 `hash` (64 lowercase hexadecimal characters)
- `screen_offer`: validated `ScreenOffer`
- `screen_signal`: UUID session `id` plus `stop` or `request_keyframe`
- `audio_signal`: UUID call ID, `offer` / `answer` / `candidate` / `reject` / `end` kind, and at most 64KiB of JSON WebRTC signaling data

Replies contain `ok`, `error`, `offset`, and an optional `accepted` decision used by screen offers. Text is saved before acknowledgement with a composite message ID / peer ID / direction key. Timestamps are local. UI polls local state independently of network framing.

Message, file and screen handlers verify pairing and the authenticated peer capability metadata before accepting data. Legacy v1 peers remain compatible through the original-MVP fallback described above.

## Audio calls

Audio calls use the authenticated QUIC session for WebRTC offer, answer, ICE candidate, reject and end signals. Only a paired peer advertising `audio` may send them. The receiver bounds the in-memory signal queue to 256 events and exposes it to the local UI through a monotonic cursor; call signaling is not written to message history.

Media does not pass through a LoCAL server. WebRTC negotiates a direct LAN route with no STUN or TURN configuration, then protects audio using its DTLS-SRTP transport. Microphone access begins only after the user presses “電話をかける” or accepts an incoming call. The current implementation is one-to-one, audio-only, and has no Internet rendezvous or relay fallback.

## Files

Sender hashes and offers a file. Receiver accepts explicitly within 120 seconds, locks a partial file keyed by sender ID + full content hash, and replies with its existing byte length. Sender seeks to that offset and streams the remainder with 1MiB buffers. FIN terminates the data. Receiver verifies size, syncs, hashes the complete content and atomically hard-links it to a unique UUID-prefixed destination without overwriting files. Completion is then acknowledged.

Resumption uses contiguous offsets, not a chunk bitmap. Partial files survive restarts; retries require new consent. A hash mismatch truncates the partial to zero for a clean retry. Simultaneous writers to a partial are rejected. Limits: 8 active transfers, 100 visible transfers. Complete and partial bytes persist; the transfer list is session-local.

## Local resource model

Future LoCAL services are identified with strict local resource URIs:

```text
lm://<64-hex-device-id>/files
lm://<64-hex-device-id>/clipboard
lm://<64-hex-device-id>/screen/display/display-0
lm://<64-hex-device-id>/audio/output
lm://<64-hex-device-id>/sensors/gyro
```

`lm://` is an application-level identifier only. It does not replace TLS identity, pairing, capability checks or per-feature permission. Query strings, fragments, empty path segments and `.` / `..` traversal are rejected by the core parser.

## Screen-sharing session protocol

The core now contains the transport/session layer needed by future capture and rendering backends:

- codec enum: H.264, VP9 and AV1
- directional screen media limits: encode/decode codecs, maximum dimensions and FPS
- endpoint features: control target, system-audio capture and system-audio playback
- `ScreenOffer`: UUID session ID, LocalMesh screen resource path, selected codec, dimensions, FPS and requested optional features
- explicit accept/reject decision with a 60-second pending-offer timeout
- `ScreenSignal`: stop or request-keyframe control messages
- `ScreenStreamInit`: UUID, codec, dimensions and FPS binding a video stream to an accepted offer
- `ScreenFrameHeader`: sequence, monotonic timestamp, keyframe flag and encoded payload length

An offer is accepted only if the authenticated source encoder and viewer decoder both support the selected codec and limits. Optional audio requires source capture plus viewer playback capability. A control request requires the source to advertise itself as a control target. Session IDs are checked against the authenticated peer so a peer cannot signal or attach a video stream to another peer's session.

Large encoded video frames are **not** carried inside the 96KiB CBOR control-frame limit. After an offer is accepted, the source opens a dedicated QUIC unidirectional stream. The stream begins with one normal CBOR `ScreenStreamInit`, then switches to a fixed binary layout for each encoded access unit:

```text
8 bytes  sequence        unsigned big-endian
8 bytes  timestamp_us    unsigned big-endian
1 byte   keyframe        0 or 1
4 bytes  payload_len     unsigned big-endian
N bytes  encoded payload
```

The fixed frame header is 21 bytes. `payload_len` must be 1–16MiB; invalid flags or oversized lengths are rejected before allocation. Sequence numbers must strictly increase and timestamps must not move backwards. QUIC transport allows at most 8 concurrent unidirectional streams per connection.

The incoming video queue retains at most four encoded frames and drops the oldest queued frame under renderer backpressure. This deliberately favors freshness over building unlimited latency. Screen state is bounded to 8 active/pending sessions and 50 visible session records. Disconnect and local shutdown close pending/active screen state.

Android registers an H.264 decode role only when its WebView reports a usable WebCodecs decoder. It pulls bounded encoded frames from the same QUIC video queue and renders them locally. Android screen capture, remote pointer/keyboard events and system-audio media transport are **not** implemented yet; their capabilities remain independently negotiated for later phases. See [`SCREEN_SHARING.md`](SCREEN_SHARING.md) for capture backends, permission separation and the remaining implementation sequence.

## Storage and future work

`identity.json` has mode 0600 on Unix, its data directory 0700. A process lock prevents simultaneous use. SQLite WAL stores trusted IDs, name and messages; UI shows latest 200 messages. File content is separate. Future channels include Android screen capture, sensors, clipboard images and custom channels; v1 does not silently accept them.
