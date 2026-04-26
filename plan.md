# Immich Auto Uploader 仕様書

## 1. 概要

Windows 上で動作するタスクトレイ常駐型の Immich 自動アップローダー。複数の監視対象フォルダを設定し、Immich CLI (`immich upload --watch`) をフォルダごとに起動して、新規追加されたファイルを自動的に Immich サーバーへアップロードする。

CLI の標準出力 (進捗バー含む) をそのままアプリ内のログ領域に表示する。

## 2. 技術スタック

- **言語**: Rust (edition 2021 以降)
- **GUI フレームワーク**: `egui` / `eframe`
- **タスクトレイ**: `tray-icon` クレート
- **非同期ランタイム**: `tokio` (process, io, sync 機能を使用)
- **設定永続化**: JSON (`serde` + `serde_json`)
- **設定ファイル配置**: `directories` クレートで OS 標準の config ディレクトリ取得
- **ターゲット OS**: Windows (まずは Windows 専用で OK、クロスプラットフォーム対応は将来検討)

### 主要依存クレート (Cargo.toml の例)

> 注: 以下のバージョンは目安。実装開始時に `cargo search <crate>` または crates.io で最新版を確認すること (eframe/egui や tray-icon は更新が早い)。

```toml
[dependencies]
eframe = "0.33"      # GUI フレームワーク (※ 0.34+ は rustc 1.92 必須。1.91 環境では 0.33 まで)
# egui は eframe の再エクスポート (eframe::egui) を使うため明示的依存は不要
tray-icon = "0.22"   # タスクトレイ
tokio = { version = "1", features = ["process", "io-util", "rt-multi-thread", "sync", "macros", "time"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
directories = "6"
anyhow = "1"
thiserror = "2"
image = "0.25"       # トレイアイコン読み込み用 (0.26 は Rust 1.92+ 必須)
kill_tree = { version = "0.2", features = ["tokio"] }  # Tokio 連携プロセスツリーキル
vte = "0.15"         # ANSI エスケープシーケンスのステートマシンパーサ
tracing = "0.1"      # 内部ロギング (アプリ自体のデバッグ用)
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }  # フォルダ ID 生成
rfd = "0.17"         # ネイティブファイル/フォルダ選択ダイアログ
winreg = "0.56"      # Windows レジストリ操作 (起動時自動起動の Run キー)
windows = { version = "0.62", features = [
    "Win32_System_Threading",        # CreateMutexW (シングルインスタンス)
    "Win32_Foundation",
    "Win32_Security",                # CreateMutexW が要求 (SECURITY_ATTRIBUTES)
    "Win32_UI_WindowsAndMessaging",  # ShowWindow など (将来用)
    "Win32_System_Diagnostics_ToolHelp",  # プロセス列挙 (kill_tree 内部で使用)
] }
```

> 上記は Rust 1.91 環境で `cargo add` を実行した際に解決された実バージョン。`cargo update` でマイナーアップデートは入る想定。

## 3. 前提条件

- ユーザーの PC に **Node.js** および **Immich CLI** (`@immich/cli`) がインストール済みであること
  - `npm install -g @immich/cli` でインストール
  - `immich` コマンドが PATH 上で実行可能であること
- ユーザーが Immich サーバーの URL と API キーを把握していること

これらが満たされない場合のヘルプは、初回起動時の設定画面に簡潔に表示する。

## 4. 機能要件

### 4.1 認証管理

- アプリ内で Immich サーバー URL と API キーを入力できる UI を提供する
- 「接続テスト」ボタンを押すと `immich login <url> <key>` を実行し、成功・失敗を表示する
  - ※ Immich CLI 公式の login コマンドは `immich login [url] [key]` (引数 2 つ、サブコマンドは `login` であり `login-key` ではない)
- 認証情報は Immich CLI 標準の `~/.config/immich/auth.yml` に保存させる方式を採用 (CLI 自身に管理させる)
- アプリ側の設定 JSON にもサーバー URL は保持しておき、UI 表示用に使う (API キーはアプリ JSON には保存しない、CLI 管理に委ねる)

### 4.2 監視対象フォルダの管理

- 複数フォルダを登録可能 (件数上限なし、現実的には数十まで想定)
- 各フォルダごとに以下の設定を持てる:
  - **パス** (Windows のフォルダパス、例: `C:\Users\neo\Pictures\VRChat`)
  - **再帰的に処理するか** (`--recursive`, デフォルト ON)
  - **アルバム指定モード** (排他、UI はラジオボタン):
    - `none`: アルバム指定なし
    - `from_folder`: フォルダ名ベースで自動作成 (`--album`)
    - `named`: 指定名のアルバムに追加 (`--album-name <name>`、`album_name` フィールドの値)
    - ※ `--album` と `--album-name` を同時指定すると Immich CLI 側の挙動が未定義のため、UI で必ず排他化する
  - **隠しフォルダを含めるか** (`--include-hidden`, デフォルト OFF)
  - **無視パターン** (`--ignore <pattern>`、配列で複数指定可。Immich CLI は `--ignore` を複数回受け付ける)
  - **同時アップロード数** (`--concurrency`, デフォルト 4)
  - **有効/無効トグル** (一時的に止めたい時用)
- フォルダの追加 (フォルダ選択ダイアログ)、編集、削除が UI からできる

### 4.3 自動アップロード

- 各「有効」なフォルダに対して、それぞれ独立した子プロセスとして `immich upload --watch <フォルダのオプション群> <パス>` を起動する
- アプリ起動時に自動で全ての有効フォルダの監視を開始する設定 (デフォルト ON) を持つ
- 子プロセスの stdout / stderr を非同期で読み、UI のログ領域に流す
- 子プロセスが異常終了した場合は UI に表示し、自動再起動オプション (デフォルト ON、最大再試行回数とバックオフ間隔は設定可能、初期値: 5回・30秒間隔) で再起動する

### 4.4 手動操作

- 各フォルダ行に「Sync Now」ボタンを置く。押すと `--watch` なしの単発 `immich upload` を実行する (既存の watch プロセスとは別プロセス)
- 「全フォルダ Sync Now」ボタンも全体に1つ
- 各フォルダの監視を「一時停止 / 再開」できる (子プロセスの kill / spawn)

### 4.5 ログ表示

- アプリ内に大きめのログ領域 (`egui::ScrollArea` + 等幅フォント) を持つ
- フォルダごとにタブ分け、または「全部混ぜて表示」を切り替え可能
- ANSI エスケープシーケンスは**剥がして表示**する。対応すべき種類:
  - 色コード (`\x1b[...m`): 捨てる
  - カーソル移動・行クリア (`\x1b[2K`, `\x1b[1A`, `\x1b[K` ほか): 適切に処理する (Node.js 系の進捗ライブラリは行クリア + カーソル戻しを多用)
  - 推奨: `vte` クレート (alacritty が使うステートマシンパーサ) で確実にパースし、表示用テキストだけ取り出す
  - 軽量実装にするなら正規表現 `\x1b\[[0-9;?]*[a-zA-Z]` で全 CSI シーケンスを除去するだけでも実用上 OK
- キャリッジリターン (`\r`) は「同じ行を上書き」として処理する。具体的には:
  - 内部的には行バッファを保持
  - `\r` を受信したら現在の行バッファをクリアして同じ位置から書き直す
  - `\n` で行を確定して次の行へ
  - これにより `Hashing files | ████ | 100% | ETA: 0s` のような進捗バーが画面上で**1行として更新**されるように見える
- ログのバッファは `VecDeque<String>` (1フォルダあたり 5,000 行上限のリングバッファ、古い行から `pop_front`)
- 「ログをクリア」「ログを保存」ボタン

### 4.6 タスクトレイ

- ウィンドウの ✕ ボタンを押した時はアプリを終了せず、ウィンドウを隠してトレイに格納する
- トレイアイコン左ダブルクリックでウィンドウ復元
- トレイアイコン右クリックメニュー:
  - ウィンドウを表示
  - 全フォルダ Sync Now
  - 全フォルダ 一時停止
  - 全フォルダ 再開
  - 終了
  - ※ 「一時停止」と「再開」はトグル状態管理を避けるため別メニューにする
- アイコンの状態で動作状況を示す (任意、優先度低):
  - 全停止中
  - 監視中 (アイドル)
  - アップロード中
- 「明示的に終了」を選んだ時のみアプリ全体を終了する

### 4.7 起動時設定

- Windows 起動時に自動起動するかどうかのトグル (実装方法はレジストリの `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` への自身の実行ファイルパス登録)
- アプリ起動時にトレイのみで起動するか、ウィンドウを表示するかのトグル

## 5. 設定ファイル

### 5.1 配置場所

- Windows: `%APPDATA%\immich-auto-uploader\config.json`
  (`directories::ProjectDirs::from("", "", "immich-auto-uploader")` で取得)

### 5.2 スキーマ (例)

```json
{
  "schema_version": 1,
  "server_url": "http://192.168.10.71:30041",
  "start_on_boot": false,
  "start_minimized_to_tray": true,
  "auto_start_watching_on_launch": true,
  "auto_restart_on_failure": true,
  "max_restart_attempts": 5,
  "restart_backoff_seconds": 30,
  "folders": [
    {
      "id": "uuid-string",
      "path": "C:\\Users\\neo\\Pictures\\VRChat",
      "enabled": true,
      "recursive": true,
      "album_mode": "from_folder",
      "album_name": "",
      "include_hidden": false,
      "ignore_patterns": [],
      "concurrency": 4
    }
  ]
}
```

**フィールド補足:**
- `schema_version`: 将来のスキーマ変更時のマイグレーション用。初期値 `1`。読み込み時に未知のバージョンならデフォルト + 警告表示。
- `album_mode`: 排他列挙。`"none"` / `"from_folder"` (`--album` を付ける) / `"named"` (`--album-name <album_name>` を付ける) のいずれか。
- `album_name`: `album_mode == "named"` のときのみ使用。
- `ignore_patterns`: 配列 (`--ignore` を複数回付与可能)。空配列ならオプション省略。

設定変更は即時に保存する (デバウンスは任意で 500ms 程度)。

## 6. UI レイアウト

メインウィンドウは縦スクロール 1 画面。横幅 700px / 縦 600px 程度を初期サイズとする。

```
┌────────────────────────────────────────────────────────┐
│  Immich Auto Uploader                          [_][□][×]│
├────────────────────────────────────────────────────────┤
│  Server                                                 │
│  URL:    [http://192.168.10.71:30041            ]       │
│  API Key:[••••••••••••••••••••••••••]  [Test Login]    │
├────────────────────────────────────────────────────────┤
│  Folders                                  [+ Add Folder]│
│  ┌── ScrollArea (vertical) ───────────────────────────┐ │
│  │ ┌────────────────────────────────────────────────┐ │ │
│  │ │ ☑ C:\Users\neo\Pictures\VRChat                 │ │ │
│  │ │   Recursive: ☑                                 │ │ │
│  │ │   Album:  ○ None  ● From folder  ○ Named [   ] │ │ │
│  │ │   Status: ● Watching  [Sync Now][Pause][Edit][✕]│ │ │
│  │ └────────────────────────────────────────────────┘ │ │
│  │ ┌────────────────────────────────────────────────┐ │ │
│  │ │ ☐ D:\Photos\2024  (disabled)                   │ │ │
│  │ │   ...                                          │ │ │
│  │ └────────────────────────────────────────────────┘ │ │
│  └────────────────────────────────────────────────────┘ │
├────────────────────────────────────────────────────────┤
│  Log    [Tab: All ▼]                  [Clear] [Save]   │
│  ┌────────────────────────────────────────────────────┐ │
│  │ Discovered API at http://192.168.10.71:30041/api   │ │
│  │ Crawling for assets...                             │ │
│  │ Hashing files | ████████░░ | 80% | ETA: 12s        │ │
│  │ ...                                                │ │
│  └────────────────────────────────────────────────────┘ │
├────────────────────────────────────────────────────────┤
│  Settings                                               │
│  ☑ Start with Windows                                   │
│  ☑ Start minimized to tray                              │
│  ☑ Auto-restart failed watchers                         │
│  Max restart attempts: [5]   Backoff (sec): [30]        │
└────────────────────────────────────────────────────────┘
```

各セクションは折りたたみ可能 (`egui::CollapsingHeader`) にしておく。

**UI 実装上の注意:**
- フォルダリストとログ領域はそれぞれ `egui::ScrollArea::vertical()` で囲み、コンテンツ量が増えてもレイアウトが破綻しないようにする (フォルダ数が増えた場合に必須)
- アルバム指定は 3 値ラジオボタン (`None` / `From folder` / `Named`) で UI 上排他化する。`Named` 選択時のみテキスト入力欄を有効化
- バックグラウンドからログイベントが届いたら `egui::Context::request_repaint()` を呼ぶ。何もしないと描画が更新されない
- mpsc は `tokio::sync::mpsc::UnboundedSender<LogEvent>` を使い、UI 側 (`App::update` 内) で `try_recv` ループで取り出してリングバッファに追加
- **日本語フォント対応**: egui のデフォルトフォント (Hack/Ubuntu) は CJK を含まないため、起動時に Windows のシステムフォントを `egui::Context::set_fonts` で挿入する必要がある。実装は `eframe::run_native` の create context callback 内で実行 (`cc.egui_ctx`)。
  - 候補順: `YuGothM.ttc` → `YuGothR.ttc` → `meiryo.ttc` → `msgothic.ttc` (Windows 8.1+ なら Yu Gothic は確実に存在)
  - 最初に `std::fs::read` 成功したフォントを `Proportional` の先頭 + `Monospace` の末尾に挿入
  - 全候補 not found の場合は `eprintln!` で警告し、デフォルト (豆腐表示) で続行

## 7. プロセス管理の詳細

### 7.1 起動コマンドの組み立て例

```
immich upload --watch --recursive --album "C:\Users\neo\Pictures\VRChat"
```

- 各フォルダ設定からコマンド引数を組み立てる
- `--no-progress` は **付けない** (進捗バーをログに出したいので)
- 環境変数 `FORCE_COLOR=0` を設定して ANSI カラーコードを抑制する (色コードのパース処理を簡素化)

### 7.2 stdout / stderr の取り扱い

```rust
// イメージ
let mut child = tokio::process::Command::new("immich")
    .args(["upload", "--watch", "--recursive", "--album", path])
    .env("FORCE_COLOR", "0")
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()?;

let stdout = child.stdout.take().unwrap();
// BufReader でバイト単位で読み、\r と \n でフラッシュする独自パーサに通す
// パース結果を mpsc::UnboundedSender<LogEvent> で UI に送る
```

- バッファリングは行単位ではなく**バイト/チャンク単位**で読む必要がある (進捗バーは `\r` で同じ行を上書きするため、`read_line` だと永遠に確定しない)
- パーサの状態は「現在の行バッファ」を保持し、`\r` でリセット、`\n` で確定

### 7.3 子プロセスの停止

- 「Pause」やアプリ終了時は子プロセスを終了する
- ⚠️ **重要**: Immich CLI は Node.js 経由で起動される (`immich.cmd` shim → `node.exe` → `@immich/cli` 本体)。`tokio::process::Child::kill()` は親プロセスにしか TerminateProcess を呼ばないため、孫の `node.exe` が残るリスクがある
- 解決策 (推奨順):
  1. **`kill_tree` クレート** (Tokio 対応・Windows 対応、内部で Win32 API を使う高速実装)
     ```rust
     use kill_tree::tokio::kill_tree;
     kill_tree(child.id().unwrap()).await?;
     ```
  2. `taskkill /F /T /PID <pid>` を `Command::new("taskkill")` で呼び出す (フォールバック)
  3. Windows Job Object を作成し子プロセスを join する (確実だが実装が複雑)
- アプリ終了時は全 watcher の停止 → `await` を経てプロセスが本当に終了したことを確認してから exit

### 7.4 シングルインスタンス制約

- ユーザーが起動アイコンを 2 回ダブルクリックすると同じフォルダを 2 重監視するリスクがある
- Windows の名前付き Mutex で先発インスタンスを検出する
- 実装 (`windows` クレート使用):
  ```rust
  use windows::Win32::System::Threading::CreateMutexW;
  use windows::core::w;
  unsafe {
      let _ = CreateMutexW(None, true, w!("Local\\immich-auto-uploader-instance"))?;
      // GetLastError() が ERROR_ALREADY_EXISTS なら既に起動中 → 既存ウィンドウに Show 通知 or 自分は即終了
  }
  ```
- 後発インスタンスは既存ウィンドウを前面表示してから即終了するのが UX 上ベスト (実装が複雑なら単に終了でも可)
- Mutex ハンドルはアプリのライフタイム全体で保持 (drop されるとカーネルが Mutex を解放)

## 8. エラー処理

- **アプリ起動時に `immich --version` を実行して CLI 検出** (Tokio の `Command` を使う):
  - 成功: バージョンをステータスに表示
  - 失敗 (ENOENT 系): 「Immich CLI が見つかりません」モーダル + インストール手順を表示
    ```
    1. Node.js をインストール: https://nodejs.org/
    2. ターミナルで `npm install -g @immich/cli` を実行
    3. アプリを再起動
    ```
  - 検出失敗時はフォルダ監視を開始しない (起動はする、設定はできる状態にする)
- ネットワークエラー、認証失敗などはログ領域に表示しつつ、フォルダのステータスを「Error」に変更
- 設定ファイルの読み込み失敗時はデフォルト設定で起動し、警告を表示
- 設定ファイルの `schema_version` が未知の値の場合: デフォルト + 警告 (ファイルは上書きしない、ユーザーに修正機会を与える)

## 9. ビルド・配布

- `cargo build --release` で Windows 用バイナリを生成
- リリースビルドではコンソールウィンドウを抑制し、デバッグビルドではコンソールを残す:
  ```rust
  // main.rs の先頭
  #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
  ```
  これにより `cargo run` 時はログが見えるが、`cargo build --release` で生成したバイナリは黒いコンソールが出ない
- 配布形式は単一の `.exe` (まずは)。インストーラ化は後回し
- アイコンリソースは `winres` クレートで埋め込み (任意、優先度低)

## 10. 開発の進め方 (推奨ステップ)

Claude Code に渡す際、以下の順で実装すると詰まりにくい。

実装ステータス凡例: ✅ 実装済 / ⚠️ 部分実装 / ⬜ 未着手

1. ✅ **基礎**: `eframe` で空のウィンドウを表示できる ([src/main.rs](src/main.rs))
2. ✅ **設定 I/O**: JSON で config を読み書きできる ([src/config.rs](src/config.rs))
3. ✅ **フォルダリスト UI**: 追加・削除・編集ができる ([src/app.rs](src/app.rs) `ui_folder_row`)
4. ✅ **CLI 起動 (シングル)**: `immich upload --watch` 起動と stdout/stderr の取り込み ([src/process.rs](src/process.rs) `spawn_watcher`)
5. ✅ **ログ UI 統合**: mpsc → UI ログ、`\r` 上書きを vte パーサで処理 ([src/ansi.rs](src/ansi.rs), [src/app.rs](src/app.rs) `LogBuffer`)
6. ✅ **複数プロセス管理**: フォルダごとに独立 `WatcherHandle` ([src/process.rs](src/process.rs))
7. ✅ **タスクトレイ**: `tray-icon` で常駐化、最小化・復元 ([src/tray.rs](src/tray.rs), [src/app.rs](src/app.rs) `handle_tray_events`)
8. ✅ **接続テスト・サーバー設定 UI**: `immich login` 連携 ([src/cli.rs](src/cli.rs), [src/app.rs](src/app.rs) `kick_login`)
9. ✅ **自動再起動・エラー処理**: 固定バックオフで再試行、最大回数到達でエラー ([src/process.rs](src/process.rs) `run_watcher_loop`)
10. ✅ **起動時自動起動**: HKCU Run キーに登録 ([src/autostart.rs](src/autostart.rs))
11. ⚠️ **磨き込み**: 簡易アイコン (tray のみ) + リリースビルド設定済 / winres 埋め込み・README は未対応 (任意・優先度低)

## 11. 非機能要件 / Non-goals

### 含めない
- ファイルの削除・移動の同期 (Immich CLI 側に任せる)
- スマートフォン連携、Web UI
- アップロード後のローカルファイル削除 (`--delete` は将来検討、誤操作リスクあり)
- 複数 Immich サーバーの同時管理
- 日本語以外のローカライズ (UI は英語ベースで OK、コメントは日本語可)

### 性能目標
- メモリ常駐時: 100MB 以下
- 配布バイナリサイズ: 20MB 以下
- ウィンドウ非表示時の CPU 使用率: アイドル時 1% 未満

## 12. ソースツリー構成

実装は以下のモジュール分割で行う (1 モジュール = 1 ファイル、`mod` 宣言は `main.rs` に集約)。

```
immich-uploader/
├── Cargo.toml           # 依存定義 (セクション 2 参照)
├── plan.md              # 本仕様書
└── src/
    ├── main.rs          # エントリポイント:
    │                    #   - #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
    │                    #   - SingleInstanceGuard 取得 (二重起動防止)
    │                    #   - tracing_subscriber 初期化
    │                    #   - Config::load_or_default
    │                    #   - autostart::sync (start_on_boot に追従)
    │                    #   - tokio::runtime::Runtime 構築 → handle を App に渡す
    │                    #   - eframe::run_native + tray::build_tray
    │                    #   - install_japanese_fonts: 日本語システムフォントを ctx.set_fonts
    ├── app.rs           # eframe::App 実装、UI レイアウト全体、ログバッファ管理
    │                    # 主要構造体: App, LogBuffer, FolderState, LogTab
    │                    # UI セクション: Server / Folders / Log / Settings (CollapsingHeader)
    ├── config.rs        # 設定 I/O:
    │                    #   - Config / FolderConfig / AlbumMode 型
    │                    #   - load_or_default / save (即時保存)
    │                    #   - schema_version によるバージョンチェック
    ├── ansi.rs          # vte ベースの LineProcessor:
    │                    #   - \n で行確定、\r でバッファクリア
    │                    #   - CSI 'K'/'J' (行/画面クリア) も buffer.clear()
    │                    #   - 色コード等は print に来ないので自動で剥がれる
    │                    # tests でユニットテスト 4 件 (改行/CR上書き/ANSI除去/CSI K)
    ├── process.rs       # 子プロセス管理:
    │                    #   - build_upload_args(folder, watch) → コマンド引数
    │                    #   - spawn_watcher → WatcherHandle (cancel + JoinHandle)
    │                    #   - run_sync_now (--watch なし単発)
    │                    #   - LogEvent enum (Line/Started/Exited/Error)
    │                    #   - 停止時は kill_tree::tokio::kill_tree で孫プロセスごと終了
    │                    #   - FORCE_COLOR=0 + NO_COLOR=1 + CREATE_NO_WINDOW
    ├── cli.rs           # Immich CLI 連携:
    │                    #   - check_immich_installed: immich --version
    │                    #   - login(url, key): immich login <url> <key>
    │                    #   - 失敗時はインストール手順を含むエラーメッセージ
    ├── tray.rs          # tray-icon 統合:
    │                    #   - TrayState { icon, menu_*_id }
    │                    #   - 内部で簡易円形アイコンを生成 (32x32 RGBA)
    │                    #   - メニュー: 表示 / Sync All / Pause All / Resume All / 終了
    ├── autostart.rs     # Windows 起動時自動起動:
    │                    #   - HKCU\Software\Microsoft\Windows\CurrentVersion\Run
    │                    #   - enable / disable / sync(bool)
    └── single_instance.rs  # 名前付き Mutex:
                         #   - CreateMutexW("Local\\immich-auto-uploader-instance")
                         #   - ERROR_ALREADY_EXISTS で None を返す
                         #   - SingleInstanceGuard が Drop で CloseHandle
```

**スレッドモデル:**
- メインスレッド: eframe の winit イベントループ (UI 描画、tray-icon メッセージループ)
- tokio runtime (multi-thread): 子プロセス監視、ログ転送、CLI コマンド実行
- 通信: `tokio::sync::mpsc::UnboundedChannel<LogEvent>` (tokio タスク → UI スレッド)
- UI スレッドは `App::update` の冒頭で `log_rx.try_recv()` ループ + `ctx.request_repaint()`

## 13. 参考リンク

- Immich CLI ドキュメント: https://docs.immich.app/features/command-line-interface/
- egui: https://github.com/emilk/egui
- tray-icon: https://crates.io/crates/tray-icon
- tokio process: https://docs.rs/tokio/latest/tokio/process/index.html

---

## 補足: Claude Code への指示テンプレート (任意)

> このリポジトリで、上記仕様書 (`plan.md`) に従って Rust プロジェクトを新規作成してください。
> まずは仕様書セクション 10 のステップ 1〜5 まで実装してください。
> ステップごとにコミットを分け、各ステップの完了時に動作確認の手順を README に追記してください。
> 不明点があれば実装前に質問してください。