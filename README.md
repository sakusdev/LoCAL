# LoCAL — Local Connectivity & Access Layer

LoCAL is a local-first peer-to-peer app and connectivity layer based on the LocalMesh design. No account, cloud server, analytics, or Internet connection is needed to exchange data on your LAN.

Rust core · QUIC / TLS 1.3 · Android · Windows · Linux · macOS · CLI

## 最初の実用版

同じWi-Fi / LANで端末を見つけ、両方の画面の6桁コードを確認してペアリング。テキスト、手動のクリップボード共有、受信確認付きファイル転送を行います。履歴と信頼済み端末は各端末のSQLiteに保存します。

[Releasesからダウンロード](https://github.com/sakusdev/LoCAL/releases) → 同じWi-Fiで両方のアプリを起動 → 相手を選択 → 両方の画面の6桁コードを確認 → ファイルやテキストを送信。

Pixel 7aは **Android arm64-v8a APK**、一般的なPCは **Windows x64 setup.exe** を選んでください。

| 機能 | v0.1 |
| --- | --- |
| 自動発見 / IP接続 | IPv4 UDP Broadcast + 補助Multicast |
| 鍵 / ペアリング | Ed25519 + TLS exporterの6桁コード、両側確認 |
| 通信 | QUIC / TLS 1.3、端末間で直接暗号化 |
| ファイル | 受信承認、最大20GiB、キャンセル、再送で再開、BLAKE3検証 |
| クリップボード | 手動のテキスト共有 |
| Android | JNIコア、ファイル選択・書き出し、接続通知 |
| GUI / CLI | 日本語UI、PC/スマホ、CLI対話モード + JSON IPC |

## 次の段階

LoCALは単なるLAN内ファイル共有ではなく、端末がローカル機能を安全に公開できる **Local Connectivity & Access Layer** を目指します。将来のリソースは `lm://<device-id>/<resource>` で表現し、たとえば `screen/main`、`audio/output`、`sensors/gyro` のように扱います。

次の主要機能はCapability negotiation、QRペアリング、画面共有、音声、センサー、mDNS / IPv6です。画面共有は閲覧・システム音声・リモート操作を別権限に分離し、最初はLAN内の閲覧専用セッションから実装します。マルチホップと画像クリップボードはその後の段階です。

初期APKは開発署名、Windows/macOSは商用コード署名なしのプレビューです。

- [使い方とトラブルシューティング](docs/QUICKSTART.md)
- [ビルド・Actions・Android署名](docs/BUILDING.md)
- [プロトコルと構成](docs/PROTOCOL.md)
- [画面共有の設計](docs/SCREEN_SHARING.md)
- [セキュリティモデルと制限](docs/SECURITY.md)

```sh
cargo test --locked -p local-core -p local-cli
cargo run -p local-cli -- --name Main-PC
```

`crates/local-core` が共通通信基盤、`crates/local-cli`・`crates/local-android`・`apps/desktop` がOSへの入口です。`ui` はすべて同梱します。Androidの外側はJavaの薄いネイティブシェルです。
