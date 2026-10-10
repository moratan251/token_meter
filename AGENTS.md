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
  src/limits.rs          使用上限（%）の履歴の取り込みと、1% あたりの API 換算額の推定
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

## 使用上限（%）との比較

- 使用率の取得元は 4 つ。
  - **Claude**: デスクトップアプリが書く `plan-usage-history.json`。Microsoft Store 版（MSIX）では
    `%LOCALAPPDATA%\Packages\Claude_<発行元ID>\LocalCache\Roaming\Claude\` にあり、
    通常版では `%APPDATA%\Claude\` にあるので両方を探す（`desktop_history_paths`）
    （非公開の形式。`{version: 2, samples: [{t, org, u: {fh, sd}}]}`、`fh` = 5 時間枠 %、
    `sd` = 7 日枠 %）。アプリを開いている間しか記録されず、古いものから消えるので、読み込んだ分を
    アプリ側の `limit-history.json` に蓄積する。複数アカウントが混ざらないよう最新サンプルの `org` だけ使う。
  - **Claude（ステータスライン）**: 使用上限の画面から `~/.claude/settings.json` の `statusLine` に
    `resources/statusline.ps1` を設定すると（`limits::install_statusline`）、Claude Code が応答するたびに
    `~/.claude/token-meter/rate-limits.jsonl` に `{t, fh, fh_reset, sd, sd_reset}`（リセットは Unix 秒）を
    値が変わったときだけ追記する。他のステータスラインが設定済みなら上書きしない。スクリプトは
    Windows PowerShell 5.1 で動くように書き、BOM 付きで置く。記録されるのは端末の `claude`（CLI）を
    使ったときだけで、デスクトップアプリの Code タブではステータスラインが実行されない（2026-10 に確認）。
  - **Codex**: ログの `rate_limits`（`used_percent`、`window_minutes`、`resets_at` は秒）。
    使用率が変わった時点だけを `RateSample` として残す。
  - **手動記録**: 使用量画面のテキストを貼り付けて読み取る（`src/lib/usageParse.ts`）。
    モデル別の枠（週次・Fable など）はここでしか取れない。
- 公式に使用率を取れるのは Claude Code のステータスライン入力の `rate_limits`
  （`five_hour` / `seven_day`。`used_percentage` は小数、`resets_at` は Unix 秒。Pro / Max のみで、
  セッションの最初の応答の後から入る）だけ。公式ドキュメントに導入バージョンの記載はない
  （コミュニティの情報では v2.1.80）。OAuth トークンを使う非公開 API は使わない。
- 枠の区切り（`limits::analyze`）: リセット時刻が分かればそれを使う。分からなければ、3 ポイント以上の
  低下・0% への低下・枠の長さの経過で区切る。開始時刻が推定の枠は「最初の記録 + 枠の長さ」で必ず終わる。
  0%（0.5 以下）の記録は「枠が始まっていない」とみなして枠に入れず、次の枠の開始時刻の下限にする。
  開始時刻は、下限の後の最初の Code の記録の直前（なければ区間の中ほど）とする。実データでは 3 ポイント
  程度の低下も時刻から見て本当のリセットだったので、低下の判定を緩めないこと。
- 推定は「今の枠の API 換算額 ÷ 今の使用率」を基本にし、使えなければ使用率 10% 以上の直近の枠、
  最後に過去 30 日の区間平均を使う。今有効なプランの開始日が 30 日以内なら、それより前のデータは使わない
  （プランで 1% の重みが変わるため）。
- Claude の 5 時間枠と週間枠は、両方の使用率がそろった連続する記録から「5 時間枠の 1% が週間枠の何 % か」を
  求め（`claude_pair_ratio`）、互いの推定を照らし合わせる（`reconcile`）。2.5 倍以上食い違うか片方の信頼度が
  低ければ、信頼度の高い方（同じなら週間枠）から換算した値を使い、`basis = "cross"` にする。
- 推定値には信頼度（`quality`）を付ける。Code 分の増加が 10 ポイント未満、Code 以外の比率が高い、
  最後の記録が古い、などで下げる。
- 使用率にはチャットなど Code 以外の利用も含まれるので、分母から除く。使用率が上がった区間に
  Code の記録が 1 件もなければ（反映の遅れとして直前の記録の 10 分前まで見る）、その増加を
  「Code 以外」とする。週間枠は 1% 上がるまでに Code とチャットが混ざりやすいので、同じ時間帯の
  5 時間枠の内訳で按分する（`other_share`）。同じ区間での併用や、デスクトップアプリを閉じていて
  記録がない時間帯は分けられないので、1% あたりの額はまだ低めに出ることがある。
  除かない値は `raw_usd_per_percent` として UI に併記する。
- リセット時刻が分からない枠は、使用率の急な低下から開始時刻を推定する。手動記録などでリセット時刻が
  分かれば、そちらを優先する。

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

インストール版の Token Meter が起動していると、WebView2 のデータフォルダが競合して開発版の起動が
`0x8007139F` で失敗する。インストール版は終了させず、`WEBVIEW2_USER_DATA_FOLDER` に別のフォルダを
指定して起動する。

`http://127.0.0.1:9333/json` でページを取得でき、Chrome と同じく `Runtime.evaluate` や
`Page.captureScreenshot` が使えます。

## 環境についての注意

- **Claude デスクトップアプリ（MSIX）の中で動くエージェントへ:** シェルなどの子プロセスはアプリの
  コンテナ内で動くため、`%APPDATA%` の読み書きが `%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\`
  に振り替えられる。開発版のアプリやテストが書く設定・キャッシュはユーザーが普段起動するアプリとは別の
  場所になり、逆にユーザーの実際の設定は見えない。ユーザー環境の挙動を確かめるときはこの点に注意する。

- Windows 11 です。**スマートアプリコントロール**が有効だと、署名のないバイナリ（Rust の
  ツールチェーン、ビルドスクリプト、ビルドした exe）がブロックされます。開発機ではオフに
  してあります。セキュリティ設定を自分で変更しないでください。
- `~/.claude/.credentials.json`、`~/.codex/auth.json` などの認証情報ファイルは読まないでください。
  アプリに必要なのは JSONL のログだけです。
- `dist/`、`node_modules/`、`src-tauri/target/`、`src-tauri/gen/` はコミットしないでください。
