# LocalMesh protocol v1 in LoCAL

The Text + File MVP shares a Rust core between the CLI, Tauri desktop shell and Android JNI shell. Android uses a bundled offline WebView UI rather than a separate Compose UI. No remote frontend assets are loaded.

## Discovery and identity

JSON UDP beacons on port 53318 every 3 seconds use IPv4 broadcast and supplementary multicast `239.255.53.18`. Fields: `magic=LOCAL_DISCOVERY`, version, device ID, name and listening port. Source IP comes from the socket. Peers expire after 20 seconds; trusted entries remain. Metadata is untrusted until TLS identity is checked. Manual IPv4:port works without discovery. DNS-SD/mDNS, IPv6, QR, Internet rendezvous and relays are not implemented.

Each device persists an Ed25519 private key and self-signed certificate. Identity is the full BLAKE3 hash of the public key. Quinn/rustls uses QUIC + TLS 1.3, ALPN `localmesh/1`, required client certificates. Certificate format and handshake signatures are verified; public CA chains and DNS names do not define trust. The certificate key must match the expected/discovered ID.

The first bidirectional stream exchanges `{version,id,name,port}`. A six-digit code is derived from 32 bytes of TLS exporter material with label `LoCAL pairing v1` and empty context. Users compare codes on both devices. Each side exchanges `Confirm`; BOTH local and remote confirmation are required before data requests or persisted trust. Known peers confirm existing stored trust automatically. Pairing expires after 120 seconds. Forgetting a peer requires confirmation again on reconnect.

## Framing

Operations open separate QUIC bidirectional streams. CBOR frames use a four-byte unsigned big-endian length, capped at 96KiB. Trailing bytes in the CBOR frame are rejected. Tagged requests:

- `confirm`
- `message`: UUID `id`, `channel` (`mesh.text` or `mesh.clipboard`), UTF-8 `text` (1–16384 bytes)
- `file`: UUID `id`, safe basename `name`, `size` (0–20GiB), BLAKE3 `hash` (64 lowercase hexadecimal characters)

Replies contain `ok`, `error`, `offset`. Text is saved before acknowledgement with a composite message ID / peer ID / direction key. Timestamps are local. UI polls local state independently of network framing.

## Files

Sender hashes and offers a file. Receiver accepts explicitly within 120 seconds, locks a partial file keyed by sender ID + full content hash, and replies with its existing byte length. Sender seeks to that offset and streams the remainder with 1MiB buffers. FIN terminates the data. Receiver verifies size, syncs, hashes the complete content and atomically hard-links it to a unique UUID-prefixed destination without overwriting files. Completion is then acknowledged.

Resumption uses contiguous offsets, not a chunk bitmap. Partial files survive restarts; retries require new consent. A hash mismatch truncates the partial to zero for a clean retry. Simultaneous writers to a partial are rejected. Limits: 8 active transfers, 100 visible transfers. Complete and partial bytes persist; the transfer list is session-local.

## Local resource model

Future LoCAL services are identified with strict local resource URIs:

```text
lm://<64-hex-device-id>/files
lm://<64-hex-device-id>/clipboard
lm://<64-hex-device-id>/screen/main
lm://<64-hex-device-id>/audio/output
lm://<64-hex-device-id>/sensors/gyro
```

`lm://` is an application-level identifier only. It does not replace TLS identity, pairing, capability checks or per-feature permission. Query strings, fragments, empty path segments and `.` / `..` traversal are rejected by the core parser.

The first capability vocabulary is:

- `text`
- `file`
- `clipboard`
- `screen.view`
- `screen.audio`
- `screen.control`
- `audio`
- `sensor`

Capability advertisement is intentionally being introduced as an extension rather than changing the current v1 Hello immediately. This keeps existing v0.1 peers wire-compatible while the negotiation flow is implemented and tested.

## Screen-sharing extension foundation

The core contains serializable screen protocol primitives for future negotiated sessions:

- codec enum: H.264, VP9 and AV1
- screen capability limits: codecs, maximum dimensions / FPS, control and system-audio support
- `ScreenOffer`: source, selected codec, dimensions, FPS and requested optional features
- `ScreenFrameHeader`: sequence, monotonic timestamp, keyframe flag and encoded payload length

Large encoded video frames are **not** carried inside the 96KiB CBOR control-frame limit. Screen video will use a dedicated QUIC stream after explicit offer / accept negotiation. See [`SCREEN_SHARING.md`](SCREEN_SHARING.md) for capture backends, permission separation, transport policy and implementation phases.

## Storage and future work

`identity.json` has mode 0600 on Unix, its data directory 0700. A process lock prevents simultaneous use. SQLite WAL stores trusted IDs, name and messages; UI shows latest 200 messages. File content is separate. Future channels include screen sharing, Opus, sensors, clipboard images and custom channels; v1 does not silently accept them.
