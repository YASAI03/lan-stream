# lan-stream

Windows のウィンドウキャプチャを LAN 内に QOI over WebSocket で配信する軽量ストリーミングサーバー。

## モチベーション

- アイトラッカーを購入し、それを気軽に使えるようにしたいと考えた
- 会社用のPCに直接ソフトを入れるのは憚られるため、普段使っているメインのPCでキャプチャして、会社のPCからブラウザで見られるようにしたい
  - 作業しているモニターの裏側で動かすことによって、overlayすることも容易にできる

## 特徴

- **Windows Graphics Capture API** による高性能キャプチャ
  - 最小化・背面のウィンドウもキャプチャ可能
  - DirectX/OpenGL アプリ対応
  - GPU アクセラレーション（D3D11）
- **QOI (Quite OK Image) over WebSocket** でブラウザから直接視聴（プラグイン不要）
- **WASM デコーダー**（JS フォールバック付き）+ **WebGL レンダリング**（Canvas2D フォールバック付き）
- 複数クライアント同時接続対応
- クライアントごとのビットレート計測
- Web UI で設定変更・パフォーマンスモニタリング
- 設定は `config.toml` に永続化

## 動作要件

- Windows 10 1903 以降（Graphics Capture API）
- Rust 2024 edition

## セットアップ

```bash
git clone <repo-url>
cd lan-stream
cargo run
```

起動後、ブラウザで `http://localhost:8080` にアクセス。

## 設定

`config.toml` で設定を管理する。Web UI（`/config`）からも変更可能。

```toml
[capture]
window_title = "メモ帳"      # キャプチャ対象（部分一致）
target_fps = 30              # 目標FPS (1-120)
capture_cursor = true        # マウスカーソルを含める

[server]
host = "0.0.0.0"             # バインドアドレス
port = 8080                  # ポート番号
```

| 項目 | 説明 | 範囲 | デフォルト |
|------|------|------|-----------|
| `window_title` | キャプチャするウィンドウのタイトル（部分一致） | 任意文字列 | `""` |
| `target_fps` | 目標フレームレート | 1 - 120 | 30 |
| `capture_cursor` | マウスカーソルをキャプチャに含めるか | `true` / `false` | `true` |
| `host` | サーバーのバインドアドレス | — | `0.0.0.0` |
| `port` | サーバーのポート番号 | 1 - 65535 | 8080 |

> **注意**: `host` / `port` の変更はサーバー再起動後に反映されます。

## ページ

| パス | 説明 |
|------|------|
| `/` | ストリームビューア — WebSocket 経由の QOI ストリームをリアルタイム表示 |
| `/raw` | ストリームビューア（代替ページ） |
| `/config` | 設定ページ — キャプチャ・サーバー設定を Web UI で変更 |
| `/debug` | デバッグダッシュボード — FPS、フレーム処理時間、ビットレート、サーバーログ |

全ページに共通ヘッダー（ナビゲーション＋キャプチャ状態＋ビットレート表示）を表示。

## API

| メソッド | パス | 説明 |
|----------|------|------|
| `GET` | `/ws` | WebSocket ストリーム（QOI バイナリフレーム＋ビットレート JSON） |
| `GET` | `/api/config` | 現在の設定を JSON で取得 |
| `POST` | `/api/config` | 設定を更新・永続化 |
| `GET` | `/api/windows` | キャプチャ可能なウィンドウ一覧 |
| `GET` | `/api/debug` | パフォーマンスメトリクス・サーバーログ・ビットレート |
| `GET` | `/api/health` | ヘルスチェック（キャプチャ状態、接続クライアント数、FPS、ビットレート） |
| `GET` | `/api/ping` | レイテンシ計測用 |

詳細な API 仕様は [docs/openapi.json](docs/openapi.json) を参照。

## アーキテクチャ

```
┌──────────────┐    watch::channel    ┌──────────────┐   WebSocket     ┌──────────┐
│  Capture     │ ─── QOI frames ────→ │  Axum Server │ ──────────────→ │ Browser  │
│  Thread      │                      │  (async)     │  (multiple)     │          │
│              │                      │              │ ← JSON API ──── │          │
│ D3D11 + GCA  │                      │ /ws          │                 │ /        │
│ + QOI encode │                      │ /api/*       │                 │ /config  │
│              │                      │ /config      │                 │ /debug   │
└──────────────┘                      └──────────────┘                 └──────────┘
```

- **キャプチャスレッド**: Graphics Capture API でウィンドウをキャプチャ → D3D11 ステージングテクスチャにコピー → CPU 読み出し → QOI エンコード → `watch::channel` で最新フレーム配信
- **WebSocket サーバー (Axum)**: `/ws` から QOI フレームをバイナリメッセージで配信。毎秒テキストメッセージでクライアント個別ビットレートを通知。複数クライアント同時接続対応
- **フレーム配信**: `tokio::sync::watch` で最新フレームのみ保持。遅いクライアントによるバックプレッシャーを防止

## パフォーマンスメトリクス

デバッグダッシュボード（`/debug`）で以下を確認可能:

- **FPS**: 実測フレームレート
- **GPU Copy**: GPU テクスチャ → ステージングテクスチャのコピー時間
- **Map**: テクスチャのメモリマップ時間
- **Readback**: BGRA ピクセルデータの CPU 読み出し時間
- **Encode**: QOI エンコード時間
- **Total**: 1 フレームの合計処理時間
- **Bitrate**: WebSocket 全クライアント合計ビットレート

履歴チャート（約15分間）でトレンドも確認できる。

## 技術スタック

| 用途 | ライブラリ |
|------|-----------|
| ウィンドウキャプチャ | `windows` crate (Graphics Capture API, D3D11) |
| 画像エンコード | `qoi` (Quite OK Image) |
| HTTP/WebSocket サーバー | `axum` |
| 非同期ランタイム | `tokio` |
| 設定管理 | `serde` + `toml` |
| クライアントデコーダー | WASM (Rust → wasm32) + JS フォールバック |

## ライセンス

MIT OR Apache-2.0 のデュアルライセンスで提供されます。

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

いずれかを選択して利用できます。

## 備考

- このプロジェクトは[モチベーション](#モチベーション)に記載の個人的なニーズから始まり、ほとんどをGitHub Copilotの支援で開発しました。
