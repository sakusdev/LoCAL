# Security model and limitations

LoCAL v0.1 is an early implementation, not an independently audited security product.

TLS proves possession of the device's Ed25519 key. Trust requires comparing the session-specific six-digit code on both physical displays over a trusted path. An active attacker has roughly a one-in-a-million chance per independent session of matching the short code. Do not approve unexplained requests. The full key fingerprint is also shown for stronger out-of-band comparison. Revoke trust from Settings.

Both confirmations are required for data channels. Discovery exposes device name, public-key ID and port to the LAN and is not authenticated. A familiar display name alone is not proof of identity. This application does not promise protection against a compromised local OS, traffic analysis or denial of service by a hostile LAN.

The UI escapes untrusted text, uses a restrictive CSP and loads no external assets. Android's bridge is attached to bundled local assets; external requests and navigation are blocked. Clipboard reads/writes are user-triggered. Received files are never executed or opened automatically.

Keys, messages and files use OS directory permissions, not extra database encryption at rest. A party able to read private app data can impersonate the device. Android excludes app data from backup. Paths reject traversal/separators/control characters/reserved names. Files require per-offer consent, are bounded to 20GiB, streamed and BLAKE3 checked. Partial files and Android outgoing caches can consume disk after cancellation. The receiving folder must support locks and hard links; network shares and FAT volumes may not work.

Default Android previews use Gradle's development signing key. Independently built debug APKs may use different keys and require reinstalling. Use the signing secrets in BUILDING.md for consistent production updates. Never commit a real signing key. Desktop packages are unsigned/unnotarized. Release checksums and manifest identify packaged content and source commit.

