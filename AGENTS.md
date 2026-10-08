# AGENTS.md

このリポジトリで作業するコーディングエージェント（Claude Code、Codex など）向けの手引きです。

## 概要

**Token Meter** は、AI コーディング系サブスクのローカル使用ログを読み込み、トークン使用量を
**API 従量課金に換算した金額**として表示する Windows 向けデスクトップアプリです
（Tauri 2 + Svelte 5）。定額プランでどれだけ「元が取れているか」を確認できます。
`ccusage` を GUI で使いやすくしたものという位置づけです。

対応しているログ:

| サービス | ログの場所（自動検出） |
|---|---|
| Claude Code | `%USERPROFILE%\.claude\projects\**\*.jsonl`（`CLAUDE_CONFIG_DIR`、`~/.config/claude/projects` も） |
| Codex（CLI / Desktop） | `%USERPROFILE%\.codex\sessions\**\*.jsonl`、`archived_sessions`（`CODEX_HOME` も） |

**UI の文言とコード中のコメントは日本語**で書いてください。

## 構成

```
src/                     Svelte 5 フロントエンド（runes）
  App.svelte             全体の枠、テーマ、自動更新、起動時のスキャン
  pages/                 Dashboard（ダッシュボード）、Explorer（明細）、Subscription（元取り分析）、
                         Pricing（価格表）、Settings（設定）
  components/            FilterBar、MultiSelect、Chart（ECharts ラッパー）、StatTile、Sidebar
  lib/api.ts             Tauri の invoke ラッパー、LiteLLM・為替レートの取得
  lib/types.ts           Rust の構造体に対応する TS の型（snake_case）
  lib/state.svelte.ts    グローバルな状態（設定、メタ情報、絞り込み条件）
  lib/colors.svelte.ts   検証済みのグラフ配色とテーマのフラグ
src-tauri/
  src/sources/claude.rs  Claude Code の JSONL 解析
  src/sources/codex.rs   Codex のロールアウト解析
  src/store.rs           差分スキャン、ファイル単位のキャッシュ、統合と重複排除
  src/pricing.rs         価格情報、LiteLLM の取り込み、金額計算
  src/query.rs           絞り込み、ダッシュボード・明細の集計、プラン料金の按分
  src/settings.rs        保存される設定（アプリ設定フォルダの JSON）
  src/lib.rs             Tauri コマンド
  resources/curated_prices.json   公式価格で確認済みの Anthropic の単価（100 万トークンあたり USD）
  resources/litellm_snapshot.json ビルド時に同梱する LiteLLM 価格表の抜粋
```

## コマンド

Rust は `%USERPROFILE%\.cargo\bin` にあります。ログインシェルでない場合は `PATH` に追加してください。

```bash
npm install
npm run app:dev            # tauri dev（vite :1420 + デバッグビルド）
npm run app:build          # リリースビルド + NSIS インストーラー
npm run check              # svelte-check（エラー・警告ともに 0 であること）
npx vite build             # フロントエンドのみ

cd src-tauri
cargo test --lib                                              # 単体テスト
cargo test --release --lib real_data -- --ignored --nocapture # 実際のローカルログを読んで合計と処理時間を表示
```

コミット前に `cargo test --lib` と `npm run check` を実行してください。

## 間違えやすい解析ルール

- **Claude Code** はストリーミングされたコンテンツブロックごとに 1 行を書きます。同じリクエストの
  行は `message.id` + `requestId` が同じで、`output_tokens` は行が進むごとに増えます。
  このキーでまとめて**各フィールドの最大値**を採ってください（最初の行だけ数えると出力が少なく
  なる）。`model == "<synthetic>"` の行はプレースホルダーなので除外します。
  サブエージェントの会話ログは `<session>/subagents/` にあり、`isSidechain: true` が付きます。
- **Codex** は `token_usage_record` イベント（API レスポンスごとに 1 件、`response_id` が一意。
  圧縮処理の呼び出しも含む）を優先します。それが出る前に書かれた部分は `token_count` イベントの
  `total_token_usage` の差分を使い、同じ累計の繰り返しは飛ばし、累計が減ったら
  `last_token_usage` を使います。
  OpenAI の定義では `input_tokens` は `cached_input_tokens` を**含み**、
  `output_tokens` は `reasoning_output_tokens` を**含み**ます。
  モデル名は `turn_context` / `thread_settings_applied` から取ります。古いバージョンが派生させた
  サブエージェントは記録しないため、store が親セッションで最も使われたモデルを割り当てます
  （`Store::fill_unknown_models`）。
- 記録は安定した FNV ハッシュのキーで**全体を通して**重複排除するので、複数のファイルに現れる
  リクエスト（再開したセッション、`archived_sessions` へ移動したロールアウト）は 1 回だけ数えます。
  キーはキャッシュに保存されるため、`DefaultHasher` に変えないでください。
- 解析結果が変わる修正をしたら `store.rs` の `PARSER_VERSION` を上げてください。既存のファイルは
  再解析され、すでに削除されたファイルの記録はキャッシュのまま残ります。

## 価格

- 単価はすべて **100 万トークンあたりの USD** です。決定順: ユーザー設定 →
  内蔵の確認済み価格（`curated_prices.json`）または最新の LiteLLM（`price_priority` で切り替え）→
  同梱の LiteLLM スナップショット。エイリアスで未知のモデルに別モデルの価格を割り当てられます。
- 内蔵の Claude の価格は Anthropic の公式価格で確認したものです。LiteLLM には誤った値が
  あったため（例: Sonnet 5.5 のキャッシュ読込）、既定では内蔵価格を優先し、価格表の画面で差異を
  表示します。
- 長文脈料金は、プロンプト（入力 + キャッシュ読込 + キャッシュ書込）が `above` を超えた
  **リクエスト全体**に適用します。
- キャッシュ単価がない場合は、読込 = 入力×0.1、5 分書込 = 入力×1.25、1 時間書込 = 入力×2 とします。
- 確認していない価格は追加しないでください。未設定のままにすれば UI が警告を出します。

## フロントエンドの決まりごと

- Svelte 5 の runes（`$state`、`$derived`、`$effect`、`$props`）のみを使います。コンポーネント外で
  runes を使うファイルは `*.svelte.ts` という名前にします。
- Tauri コマンドの引数名は JS 側では camelCase（`groupBy`）、引数の中の構造体のフィールドは
  snake_case（serde の既定）です。
- グラフは `lib/colors.svelte.ts` の配色（両テーマで色覚多様性を検証済み）を使います。色は
  グラフ内の順位ではなく対象に付けます（全期間の順位を使う `assignColors`）。1 つのグラフに y 軸は
  1 本だけ。文字は系列色ではなく通常の文字色で書きます。
- テーマの色はすべて `src/app.css` の CSS 変数で定義し、`[data-theme="dark"]` で上書きします。

## UI の確認方法

WebView2 のデバッグポートを有効にして起動し、CDP で操作します。

```bash
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333 npm run app:dev
```

`http://127.0.0.1:9333/json` でページを取得でき、Chrome と同じく `Runtime.evaluate` や
`Page.captureScreenshot` が使えます。

## 環境についての注意

- Windows 11 です。**スマートアプリコントロール**が有効だと、署名のないバイナリ（Rust の
  ツールチェーン、ビルドスクリプト、ビルドした exe）がブロックされます。開発機ではオフに
  してあります。セキュリティ設定を自分で変更しないでください。
- `~/.claude/.credentials.json`、`~/.codex/auth.json` などの認証情報ファイルは読まないでください。
  アプリに必要なのは JSONL のログだけです。
- `dist/`、`node_modules/`、`src-tauri/target/`、`src-tauri/gen/` はコミットしないでください。
