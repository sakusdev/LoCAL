# Build and release

Stable Rust + rustfmt/clippy; Cargo.lock pins dependencies. Node 22 is used for Tauri tooling. Android uses Java 17 / Gradle 8.11.1 / SDK 35 / NDK 27.2.12479018.

```sh
cargo test --locked -p local-core -p local-cli
cargo clippy --locked -p local-core -p local-cli -p local-android -- -D warnings
cargo fmt --all -- --check
cargo run -p local-cli -- --name Dev-PC
```

Desktop needs Tauri prerequisites. Ubuntu: `libwebkit2gtk-4.1-dev build-essential libxdo-dev libssl-dev librsvg2-dev patchelf`.

```sh
cd apps/desktop
npm ci
npm run tauri dev
npm run tauri build
```

UI files are embedded directly from `ui/`. A browser-only preview has no native network engine.

Android on Linux, after installing SDK/NDK and setting `ANDROID_NDK_ROOT`:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
bash scripts/build-android.sh
cd apps/android
gradle assembleDebug
```

Actions installs a pinned Gradle version rather than downloading an unpinned wrapper. Android Studio can open `apps/android`. Build the native libraries before packaging.

## Actions

CI tests real QUIC endpoints, Clippy and rustfmt on every main push / PR. Release runs manually or when `release.json` changes. Increment `revision` to rebuild a corrected candidate. Validation gates all six desktop rows and Android; publication waits for EVERY platform. Only publish has contents-write permission. It creates a draft, uploads packages/checksums/manifest, then publishes. It refuses to replace a release from a different commit; rerunning the same commit is supported.

New versions: update Cargo workspace, Tauri config, desktop package/lock, Android version, packaging script and `release.json.tag`. v0.2.3 is marked prerelease.

## Android production signing

| Actions secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | Base64-encoded private release keystore |
| `ANDROID_KEYSTORE_PASSWORD` | Keystore password |
| `ANDROID_KEY_ALIAS` | Signing key alias |
| `ANDROID_KEY_PASSWORD` | Key password |

With all four configured, Actions produces signed `release.apk`; otherwise clearly named installable, Android Debug signed `debug.apk`. Actions verifies the signature and certificate before publishing. CI-generated debug keys change between runs; reinstalling does not migrate app-private data automatically. Keep an offline backup of any production key.
