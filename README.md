# LoCAL — Local Connectivity & Access Layer

LoCAL is a local-first peer-to-peer app and connectivity layer based on the LocalMesh design. No account, cloud server, analytics, or Internet connection is needed to exchange data on your LAN.

Rust core · QUIC / TLS 1.3 · Android · Windows · Linux · macOS · CLI

## 最初の実用版

同じWi-Fi / LANで端末を見つけ、両方の画面の6桁コードを確認してペアリング。テキスト、手動のクリップボード共有、受信確認付きファイル転送を行います。履歴と信頼済み端末は各端末のSQLiteに保存します。

[Releasesからダウンロード](https://github.com/sakusdev/LoCAL/releases) → 同じWi-Fiで両方のアプリを起動 → 相手を選択 → 両方の画面の6桁コードを確認 → ファイルやテキストを送信。

Pixel 7aは **Android arm64-v8a APK**、一般的なPCは **Windows x64 setup.exe** を選んでください。

| 機能 | v0.2 |
| --- | --- |
| 自動発見 / IP接続 | IPv4 UDP Broadcast + 補助Multicast |
| 鍵 / ペアリング | Ed25519 + TLS exporterの6桁コード、両側確認 |
| 通信 | QUIC / TLS 1.3、端末間で直接暗号化 |
| ファイル | 受信承認、最大20GiB、キャンセル、再送で再開、BLAKE3検証 |
| クリップボード | 手動のテキスト共有 |
| Android | JNIコア、ファイル選択・書き出し、接続通知 |
| GUI / CLI | 日本語UI、PC/スマホ、CLI対話モード + JSON IPC |
| 画面共有プレビュー | Windows同士の閲覧専用H.264、受信側の承認・停止 |

## 次の段階

LoCALは単なるLAN内ファイル共有ではなく、端末がローカル機能を安全に公開できる **Local Connectivity & Access Layer** を目指します。将来のリソースは `lm://<device-id>/<resource>` で表現し、たとえば `screen/main`、`audio/output`、`sensors/gyro` のように扱います。

Windows同士の閲覧専用画面共有をプレビューとして実装しています。受信側のWebView2がH.264 Annex Bを復号できるときに使用でき、送信側の画面は受信側の承認後に取得します。Windows実機2台での画質・遅延検証は今後の課題です。続いてQRペアリング、音声、センサー、mDNS / IPv6を実装します。画面共有のシステム音声・リモート操作は別権限とし、マルチホップと画像クリップボードはその後の段階です。

APKはビルドごとの開発署名付き、Windows/macOSは商用コード署名なしのプレビューです。APKの更新には再インストールが必要です。

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
