<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, fetchUsdJpy } from "../lib/api";
  import { app, loadMeta, saveSettings, scan, toast } from "../lib/state.svelte";
  import { SERVICE_LABEL, dateTime, int } from "../lib/format";
  import { serviceColor } from "../lib/colors.svelte";
  import type { ServiceId, Settings, SourceView } from "../lib/types";

  let newService = $state<ServiceId>("claude");
  let newPath = $state("");
  let fetchingRate = $state(false);
  let rebuilding = $state(false);

  const s = $derived(app.settings!);

  async function update(patch: Partial<Settings>, rescan = false) {
    await saveSettings({ ...app.settings!, ...patch });
    if (rescan) {
      await scan();
      await loadMeta();
      app.dataVersion++;
    }
  }

  async function toggleDir(d: SourceView) {
    const disabled = d.enabled
      ? [...s.disabled_dirs, d.path]
      : s.disabled_dirs.filter((x) => x.toLowerCase() !== d.path.toLowerCase());
    await update({ disabled_dirs: disabled }, true);
  }

  async function pickDir() {
    const p = await open({ directory: true, multiple: false, title: "ログのフォルダを選択" });
    if (typeof p === "string") newPath = p;
  }

  async function addDir() {
    const path = newPath.trim();
    if (!path) return;
    await update({ extra_dirs: [...s.extra_dirs, { service: newService, path }] }, true);
    newPath = "";
  }

  async function removeDir(d: SourceView) {
    await update(
      { extra_dirs: s.extra_dirs.filter((x) => !(x.path.toLowerCase() === d.path.toLowerCase() && x.service === d.service)) },
      true,
    );
  }

  async function refreshRate() {
    fetchingRate = true;
    try {
      const rate = await fetchUsdJpy();
      await update({ usd_jpy: Math.round(rate * 100) / 100, usd_jpy_updated: new Date().toISOString() });
      toast(`1 USD = ${rate.toFixed(2)} 円 に更新しました`);
    } catch (e) {
      toast(String(e), "error");
    } finally {
      fetchingRate = false;
    }
  }

  async function rebuild() {
    const msg = s.keep_deleted
      ? "キャッシュを作り直します。すでに削除されたログファイルの履歴は失われます。続けますか？"
      : "キャッシュを作り直します。続けますか？";
    if (!confirm(msg)) return;
    rebuilding = true;
    try {
      const st = await api.rebuildCache();
      await loadMeta();
      app.dataVersion++;
      toast(`${int(st.files_total)} ファイルを ${st.duration_ms} ms で読み込みました`);
    } catch (e) {
      toast(String(e), "error");
    } finally {
      rebuilding = false;
    }
  }

  function num(e: Event): number {
    return Number((e.currentTarget as HTMLInputElement).value);
  }
</script>

{#if app.settings}
  <div class="page">
    <h1>設定</h1>

    <section class="card">
      <div class="card-head"><h2>ログの場所</h2></div>
      <table class="data">
        <thead>
          <tr><th>有効</th><th>サービス</th><th>フォルダ</th><th class="r">ファイル</th><th class="r">削除済み（保持中）</th><th></th></tr>
        </thead>
        <tbody>
          {#each app.meta?.sources ?? [] as d (d.service + d.path)}
            <tr>
              <td><input type="checkbox" checked={d.enabled} onchange={() => toggleDir(d)} aria-label="有効" /></td>
              <td><span class="row nowrap"><span class="dot" style:background={serviceColor(d.service)}></span>{SERVICE_LABEL[d.service]}</span></td>
              <td class="path">
                <span class="ellipsis" title={d.path}>{d.path}</span>
                {#if d.auto}<span class="badge">自動検出</span>{/if}
                {#if !d.exists}<span class="badge warn">見つかりません</span>{/if}
              </td>
              <td class="r">{int(d.files)}</td>
              <td class="r">{d.missing_files ? int(d.missing_files) : "—"}</td>
              <td class="r">{#if !d.auto}<button class="btn small ghost danger" onclick={() => removeDir(d)}>削除</button>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="row add">
        <select class="input" bind:value={newService} aria-label="サービス">
          <option value="claude">Claude Code（projects フォルダ）</option>
          <option value="codex">Codex（sessions フォルダ）</option>
        </select>
        <input class="input" style="flex:1;min-width:280px" placeholder={"例: \\\\wsl$\\Ubuntu\\home\\me\\.claude\\projects"} bind:value={newPath} />
        <button class="btn" onclick={pickDir}>フォルダを選択…</button>
        <button class="btn primary" onclick={addDir} disabled={!newPath.trim()}>追加</button>
      </div>
      <p class="muted tiny">WSL や別の PC から同期したフォルダも追加できます。同じリクエストが複数のファイルにあっても重複して数えません。</p>
    </section>

    <section class="card">
      <div class="card-head"><h2>履歴の保持</h2></div>
      <label class="check">
        <input type="checkbox" checked={s.keep_deleted} onchange={(e) => update({ keep_deleted: e.currentTarget.checked }, true)} />
        <div>
          <div>削除されたログファイルの集計結果を残す</div>
          <div class="muted tiny">Claude Code は既定で 30 日より古い会話ログを自動削除します。オンにすると、削除後もこのアプリ内に使用量の履歴が残ります。</div>
        </div>
      </label>
      <div class="row spaced">
        <button class="btn" onclick={rebuild} disabled={rebuilding}>{rebuilding ? "再構築中…" : "キャッシュを再構築"}</button>
        <span class="muted tiny">
          {int(app.meta?.files ?? 0)} ファイル（うち削除済み {int(app.meta?.missing_files ?? 0)}）・最終読込 {app.meta ? `${app.meta.last_scan.duration_ms} ms` : "—"}
        </span>
      </div>
      <div class="muted tiny">保存先: {app.meta?.data_dir}</div>
    </section>

    <section class="card">
      <div class="card-head"><h2>通貨</h2></div>
      <div class="row">
        <span class="secondary">1 USD =</span>
        <input class="input" type="number" step="0.01" min="1" value={s.usd_jpy} onchange={(e) => update({ usd_jpy: num(e), usd_jpy_updated: null })} style="width:110px" />
        <span class="secondary">円</span>
        <button class="btn" onclick={refreshRate} disabled={fetchingRate}>{fetchingRate ? "取得中…" : "最新レートを取得"}</button>
        <span class="muted tiny">{s.usd_jpy_updated ? `${dateTime(Date.parse(s.usd_jpy_updated))} 取得` : "手動設定"}</span>
      </div>
      <p class="muted tiny">円表示はこのレートで一律に換算します。プランの料金は登録した通貨から換算します。</p>
    </section>

    <section class="card">
      <div class="card-head"><h2>表示</h2></div>
      <div class="fields">
        <div class="field">
          <span class="label">テーマ</span>
          <div class="seg">
            {#each [["system", "システム"], ["light", "ライト"], ["dark", "ダーク"]] as [v, l]}
              <button class:on={s.theme === v} onclick={() => update({ theme: v as Settings["theme"] })}>{l}</button>
            {/each}
          </div>
        </div>
        <div class="field">
          <span class="label">週の始まり</span>
          <div class="seg">
            <button class:on={s.week_start === 1} onclick={() => update({ week_start: 1 })}>月曜</button>
            <button class:on={s.week_start === 0} onclick={() => update({ week_start: 0 })}>日曜</button>
          </div>
        </div>
        <div class="field">
          <span class="label">自動更新</span>
          <select class="input" value={String(s.auto_refresh_secs)} onchange={(e) => update({ auto_refresh_secs: num(e) })}>
            <option value="0">オフ</option>
            <option value="30">30秒ごと</option>
            <option value="60">1分ごと</option>
            <option value="300">5分ごと</option>
          </select>
        </div>
      </div>
    </section>

    <section class="card">
      <div class="card-head"><h2>計算の詳細</h2></div>
      <div class="fields">
        <div class="field">
          <label for="geo">US 限定推論（inference_geo = us）の倍率</label>
          <input id="geo" class="input" type="number" step="0.01" min="1" value={s.geo_us_multiplier} onchange={(e) => update({ geo_us_multiplier: num(e) })} />
        </div>
        <div class="field">
          <label for="web">Web 検索 1 回あたり（USD）</label>
          <input id="web" class="input" type="number" step="0.001" min="0" value={s.web_search_usd} onchange={(e) => update({ web_search_usd: num(e) })} />
        </div>
      </div>
      <ul class="muted tiny notes">
        <li>Claude Code: 1つの応答が複数行に分かれて記録されるため、同じ message.id / requestId の行をまとめ、最終的なトークン数を採用しています。</li>
        <li>Codex: 応答ごとの token_usage_record を優先し、それがない古いログは token_count の累計差分から求めています。入力トークンに含まれるキャッシュ分は分離して計算します。</li>
        <li>サブエージェント（Claude の Task / Codex の子スレッド）の使用量も含みます。ダッシュボードで除外できます。</li>
      </ul>
    </section>
  </div>
{/if}

<style>
  .page {
    padding: 22px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    max-width: 1100px;
  }
  .tiny {
    font-size: 11.5px;
  }
  .path {
    max-width: 520px;
  }
  .path .ellipsis {
    display: inline-block;
    max-width: 400px;
    vertical-align: middle;
  }
  .nowrap {
    flex-wrap: nowrap;
    gap: 6px;
  }
  .add {
    margin-top: 12px;
  }
  .check {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .check input {
    margin-top: 3px;
  }
  .spaced {
    margin: 12px 0 6px;
  }
  .fields {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 14px;
  }
  .notes {
    margin: 12px 0 0;
    padding-left: 18px;
  }
  section p {
    margin: 8px 0 0;
  }
</style>
