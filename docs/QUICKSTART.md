# LoCAL の使い方

GitHub Releasesから端末に合うファイルを選びます。

| 端末 | 配布物 |
| --- | --- |
| Pixel 7a / 最近のAndroid | `android-arm64-v8a-debug.apk` |
| 古い32bit ARM Android | `android-armeabi-v7a-debug.apk` |
| Androidエミュレーター x86_64 | `android-x86_64-debug.apk` |
| Androidの種類がわからない | `android-universal-debug.apk` |
| Windows Intel / AMD | `windows-x64-setup.exe` |
| Windows Snapdragon / ARM | `windows-arm64-setup.exe` |
| Linux Intel / AMD | `linux-x64.AppImage` または `.deb` |
| Linux ARM64 / Raspberry Pi 64bit OS | `linux-arm64.AppImage` または `.deb` |
| Mac Apple Silicon | `macos-arm64.dmg` |
| Mac Intel | `macos-x64.dmg` |

実際のファイル名には `LoCAL-0.2.1-` が付きます。署名鍵を設定したAndroidビルドでは末尾が `release.apk` になります。

Android 8以降、Windows 10/11（WebView2）、Linux x64はUbuntu 22.04以降、Linux ARM64はUbuntu 24.04相当、macOS 11以降を対象としています。WindowsセットアップはWebView2がなければダウンロードします。アプリ本体のLAN通信はインターネット不要です。

Windows同士で画面を見せるには、両方をペアリング後、送り側の「画面共有」で画面と相手を選び「共有を申し込む」を押します。受け側で「画面を見る」を承認し、画面セッションの「表示する」を押します。WebView2に対応するH.264デコーダーが必要です。閲覧専用で、音声・遠隔操作はありません。終了するときは「表示を終了」を押します。この機能はプレビューで、Windows実機2台での描画・遅延確認は未実施です。

APKはActionsで生成した開発署名付きです。ビルドごとに開発鍵が変わるため、v0.2.0以前のAPKから更新する場合は再インストールが必要です。Androidの受信ファイルは先に書き出してください。Windows / macOSは商用コード署名・公証をしていません。

AppImageは実行権限を付けて起動します。FUSEがない環境では `.deb` または `--appimage-extract-and-run` を使えます。GUIに必要なWebKit/GTK等がないサーバーはCLI版を選んでください。

## 2台をつなぐ

1. 両方を同じWi-Fi / LANにつなぎ、両方でLoCALを開きます。
2. 「近くの端末」に相手が表示されたら「ペアリングする」を押します。
3. 両方の端末に表示された6桁コードを直接見比べ、**両方で**「一致しています」を押します。
4. 緑色の「暗号化接続」になったら送受信できます。以後、同じ鍵の端末は保存した信頼情報で再接続できます。

片方だけの確認では送受信を許可しません。コードは2分で失効します。コードが異なるときは接続を取り消してください。

## ファイル

「ファイルを送る」から選択 → 相手の「ファイル転送」で「受信する」。受信後にBLAKE3を検証し、完了したファイルのみ保存先に公開します。名前にUUIDを付けて、既存ファイルを上書きしません。

Androidは受信ファイルをアプリ内に保存します。「端末に保存…」でDownloadsなどへ書き出してください。再起動後も「ファイル転送」内の「保存済みの受信ファイル」から取り出せます。一覧は50件ずつ表示し、「次へ」で古いファイルを開けます。アプリの削除やデータ消去で、書き出していない受信ファイルと端末の鍵・履歴が消えます。

1ファイル20GiBまで、最大8転送。1MiBバッファでストリーミングします。切断・キャンセル後は**同じ相手に同じファイルを選び直す**と受信済みデータを使って続行します。再起動後も部分ファイルは残ります。自動再試行はしません。送信中は元ファイルを編集しないでください。Androidは送信前に選んだファイルを一時領域へコピーするため、その分の空き容量も必要です。フォルダーはZIPにまとめてください。

## テキストとクリップボード

「メッセージ」で入力して送信。Ctrl/Cmd+Enterでも送信できます。「クリップボードを貼り付け」は下書きに貼り付け、送信ボタンを押して初めて相手へ送ります。受け取った内容の「コピー」で自分のクリップボードに保存します。自動の読み取り・自動同期は行いません。

## 接続できない

- 相手の「設定」にあるIPv4アドレスとポートを「IPアドレスで接続」に入力。例 `192.168.1.20:53319`。
- ファイアウォールでLoCALのプライベートネットワーク通信を許可。UDP 53318が発見用、UDP 53319がQUIC通信用です。
- ゲストWi-FiやAP isolationがある場合は端末間通信を許可したLANへ。VPNや複数アダプター、Multicast制限ではIP指定を試してください。
- Androidの接続中通知の「停止」で接続を終了します。OSのバックグラウンド制限やAndroid 15のdataSync時間制限で終了した場合はアプリを開き直してください。
- CLIとGUIを同時に使う場合はCLIに別の `--data-dir` と `--port` を設定します。

## CLI

`local --name Main-PC` で起動し、`help` で一覧、`status` で端末IDやコードを表示します。

```text
connect 192.168.1.20:53319
status
pair PEER_ID 123456
text PEER_ID こんにちは
file PEER_ID /path/to/file.zip
accept TRANSFER_ID
quit
```

`local --json` は標準入力1行に1つJSONを受け取り、標準出力に1行JSON応答を出します。HTTPサーバーを公開しません。

```json
{"op":"snapshot"}
{"op":"connect","address":"192.168.1.20:53319"}
{"op":"confirm","peer_id":"<id>","code":"123456"}
{"op":"send_text","peer_id":"<id>","text":"hello","channel":"mesh.text"}
```
