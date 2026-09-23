# LoCAL — Local Connectivity & Access Layer

LoCAL is a local-first peer-to-peer app and connectivity layer based on the LocalMesh design. No account, cloud server, analytics, or Internet connection is needed to exchange data on your LAN.

Rust core · QUIC / TLS 1.3 · Android · Windows · Linux · macOS · CLI

## 最初の実用版

同じWi-Fi / LANで端末を見つけ、両方の画面の6桁コードを確認してペアリング。テキスト、選択した端末とのクリップボード同期、受信確認付きファイル転送を行います。履歴と信頼済み端末は各端末のSQLiteに保存します。

[Releasesからダウンロード](https://github.com/sakusdev/LoCAL/releases) → 同じWi-Fiで両方のアプリを起動 → 相手を選択 → 両方の画面の6桁コードを確認 → ファイルやテキストを送信。

Pixel 7aは **Android arm64-v8a APK**、一般的なPCは **Windows x64 setup.exe** を選んでください。

| 機能 | v0.4.1 |
| --- | --- |
| 自動発見 / IP接続 | IPv4 UDP Broadcast + 補助Multicast |
| 鍵 / ペアリング | Ed25519 + TLS exporterの6桁コード、両側確認 |
| 通信 | QUIC / TLS 1.3、端末間で直接暗号化 |
| ファイル | 受信承認、最大20GiB、キャンセル、再送で再開、BLAKE3検証 |
| 複数ファイル | Android / デスクトップで最大8件を一度に選択して送信 |
| クリップボード | 選択した端末とのテキスト自動同期、手動送信・取得、ループ防止 |
| 音声通話 | 1対1 WebRTC、着信承認、ミュート、LAN内直接接続 |
| IP接続 | 設定に表示された接続用アドレスを個別にコピー |
| Android | JNIコア、ファイル選択・書き出し、音声通話、画面共有の送受信、接続・共有通知 |
| GUI / CLI | 明るい実用画面の日本語UI、PC/スマホ、CLI対話モード + JSON IPC |
| 画面共有プレビュー | Windows / AndroidからH.264送信、対応デスクトップ / Androidで閲覧、受信側の承認・停止 |

## 次の段階

LoCALは単なるLAN内ファイル共有ではなく、端末がローカル機能を安全に公開できる **Local Connectivity & Access Layer** を目指します。将来のリソースは `lm://<device-id>/<resource>` で表現し、たとえば `screen/main`、`audio/output`、`sensors/gyro` のように扱います。

ペアリング済み端末間の1対1音声通話を実装しています。呼制御は既存のQUIC/TLS接続、音声はWebRTCのDTLS-SRTPでLAN内を直接流れます。画面共有はWindows Graphics CaptureまたはAndroid MediaProjectionで取得し、受信側の承認後にH.264で送信します。AndroidではさらにOSの画面共有許可が必要です。実機間の音質・画質・遅延検証は今後の課題です。続いてQRペアリング、センサー、mDNS / IPv6を実装します。画面共有のシステム音声・リモート操作は別権限です。

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
