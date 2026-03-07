# lan-stream

Windows のウィンドウキャプチャを LAN 内に MJPEG で配信する軽量ストリーミングサーバー。

## 特徴

- **Windows Graphics Capture API** による高性能キャプチャ
  - 最小化・背面のウィンドウもキャプチャ可能
  - DirectX/OpenGL アプリ対応
  - GPU アクセラレーション（D3D11）
- **MJPEG over HTTP** でブラウザから直接視聴（プラグイン不要）
- **turbojpeg (libjpeg-turbo)** による SIMD 高速 JPEG エンコード
- Web UI で設定変更・パフォーマンスモニタリング
- 設定は `config.toml` に永続化

## 動作要件

- Windows 10 1903 以降（Graphics Capture API）
- Rust 2024 edition
- CMake（turbojpeg-sys ビルド用）

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
quality = 80                 # JPEG品質 (1-100)
capture_cursor = true        # マウスカーソルを含める

[server]
host = "0.0.0.0"             # バインドアドレス
port = 8080                  # ポート番号
```

| 項目 | 説明 | 範囲 | デフォルト |
|------|------|------|-----------|
| `window_title` | キャプチャするウィンドウのタイトル（部分一致） | 任意文字列 | `""` |
| `target_fps` | 目標フレームレート | 1 - 120 | 30 |
| `quality` | JPEG 圧縮品質 | 1 - 100 | 80 |
| `capture_cursor` | マウスカーソルをキャプチャに含めるか | `true` / `false` | `true` |
| `host` | サーバーのバインドアドレス | — | `0.0.0.0` |
| `port` | サーバーのポート番号 | 1 - 65535 | 8080 |

> **注意**: `host` / `port` の変更はサーバー再起動後に反映されます。

## ページ

| パス | 説明 |
|------|------|
| `/` | ストリームビューア — MJPEG 配信映像をリアルタイム表示 |
| `/config` | 設定ページ — キャプチャ・サーバー設定を Web UI で変更 |
| `/debug` | デバッグダッシュボード — FPS、フレーム処理時間、サーバーログ |

全ページに共通ヘッダー（ナビゲーション＋キャプチャ状態インジケータ）を表示。

## API

| メソッド | パス | 説明 |
|----------|------|------|
| `GET` | `/raw` | MJPEG 生ストリーム（`multipart/x-mixed-replace`）。同時接続 1 クライアント制限。 |
| `GET` | `/api/config` | 現在の設定を JSON で取得 |
| `POST` | `/api/config` | 設定を更新・永続化 |
| `GET` | `/api/windows` | キャプチャ可能なウィンドウ一覧 |
| `GET` | `/api/debug` | パフォーマンスメトリクス・サーバーログ |
| `GET` | `/api/health` | ヘルスチェック（キャプチャ状態、クライアント接続、FPS） |

詳細な API 仕様は [docs/openapi.json](docs/openapi.json) を参照。

## アーキテクチャ

```
┌──────────────┐    watch::channel    ┌──────────────┐    MJPEG/HTTP    ┌──────────┐
│  Capture     │ ─── JPEG frames ───→ │  Axum Server │ ──────────────→ │ Browser  │
│  Thread      │                      │  (async)     │                 │          │
│              │                      │              │ ← JSON API ──── │          │
│ D3D11 + GCA  │                      │ /raw         │                 │ /        │
│ + turbojpeg  │                      │ /api/*       │                 │ /config  │
│              │                      │ /config      │                 │ /debug   │
└──────────────┘                      └──────────────┘                 └──────────┘
```

- **キャプチャスレッド**: Graphics Capture API でウィンドウをキャプチャ → D3D11 ステージングテクスチャにコピー → CPU 読み出し → turbojpeg で JPEG エンコード → `watch::channel` で最新フレーム配信
- **HTTP サーバー (Axum)**: `/raw` から MJPEG ストリーム配信。設定 API、デバッグ API、HTML ページを提供
- **フレーム配信**: `tokio::sync::watch` で最新フレームのみ保持。遅いクライアントによるバックプレッシャーを防止

## パフォーマンスメトリクス

デバッグダッシュボード（`/debug`）で以下を確認可能:

- **FPS**: 実測フレームレート
- **GPU Copy**: GPU テクスチャ → ステージングテクスチャのコピー時間
- **Map**: テクスチャのメモリマップ時間
- **Readback**: BGRA ピクセルデータの CPU 読み出し時間
- **Encode**: JPEG エンコード時間
- **Total**: 1 フレームの合計処理時間

履歴チャート（約15分間）でトレンドも確認できる。

## 技術スタック

| 用途 | ライブラリ |
|------|-----------|
| ウィンドウキャプチャ | `windows` crate (Graphics Capture API, D3D11) |
| JPEG エンコード | `turbojpeg` (libjpeg-turbo, SIMD) |
| HTTP サーバー | `axum` |
| 非同期ランタイム | `tokio` |
| 設定管理 | `serde` + `toml` |
| ストリーム生成 | `async-stream` |

## ライセンス

MIT
