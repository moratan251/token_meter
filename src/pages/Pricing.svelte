<script lang="ts">
  import { api, fetchLitellm } from "../lib/api";
  import { app, loadMeta, money as moneyCtx, saveSettings, toast } from "../lib/state.svelte";
  import { SERVICE_LABEL, compact, dateTime, int, money } from "../lib/format";
  import { serviceColor } from "../lib/colors.svelte";
  import type { CatalogEntry, ModelInfo, PriceSource, PriceSpec, Tier } from "../lib/types";

  let catalog = $state<CatalogEntry[]>([]);
  let updating = $state(false);
  let search = $state("");
  let editModel = $state<string | null>(null);
  let draft = $state<PriceSpec | null>(null);
  let aliasModel = $state<string | null>(null);
  let aliasTarget = $state("");

  const m = $derived(moneyCtx());
  const models = $derived(app.meta?.models ?? []);
  const catalogByKey = $derived(new Map(catalog.map((c) => [c.key, c])));

  $effect(() => {
    app.meta?.litellm_fetched_at;
    api.catalog().then((c) => (catalog = c));
  });

  const SOURCE: Record<PriceSource, { label: string; cls: string }> = {
    user: { label: "ユーザー設定", cls: "accent" },
    curated: { label: "公式価格（内蔵）", cls: "good" },
    litellm: { label: "LiteLLM 最新", cls: "" },
    litellm_snapshot: { label: "LiteLLM 内蔵", cls: "" },
  };

  function rate(v: number | null | undefined, fallback?: number): string {
    if (v === null || v === undefined) return fallback !== undefined ? `(${fmtRate(fallback)})` : "—";
    return fmtRate(v);
  }
  function fmtRate(v: number): string {
    return "$" + (v >= 1 ? v.toFixed(2).replace(/\.00$/, "") : v.toPrecision(2).replace(/0+$/, "").replace(/\.$/, ""));
  }

  function sameSpec(a: PriceSpec, b: PriceSpec): boolean {
    const k: (keyof PriceSpec)[] = ["input", "output", "cache_read", "cache_write_5m", "cache_write_1h"];
    return k.every((x) => Math.abs(((a[x] as number) ?? -1) - ((b[x] as number) ?? -1)) < 1e-9);
  }

  /** 内蔵価格を使っているとき、LiteLLM の値と食い違っていればそれを返す。 */
  function litellmDiff(mi: ModelInfo): PriceSpec | null {
    if (mi.price?.source !== "curated") return null;
    const c = catalogByKey.get(mi.price.key);
    if (!c?.litellm) return null;
    return sameSpec(c.litellm, mi.price.spec) ? null : c.litellm;
  }

  async function updateNow() {
    updating = true;
    try {
      const r = await fetchLitellm();
      await loadMeta();
      app.dataVersion++;
      toast(`LiteLLM から ${r.models} モデルの価格を取得しました`);
    } catch (e) {
      toast(String(e), "error");
    } finally {
      updating = false;
    }
  }

  async function setPriority(p: "curated" | "litellm") {
    if (!app.settings) return;
    await saveSettings({ ...app.settings, price_priority: p });
    await loadMeta();
  }

  async function setAuto(v: boolean) {
    if (!app.settings) return;
    await saveSettings({ ...app.settings, auto_update_prices: v });
  }

  function startEdit(mi: ModelInfo) {
    editModel = mi.name;
    aliasModel = null;
    const s = app.settings?.price_overrides[mi.name] ?? mi.price?.spec;
    draft = s
      ? JSON.parse(JSON.stringify(s))
      : { input: 0, output: 0, cache_read: null, cache_write_5m: null, cache_write_1h: null, tiers: [] };
    if (draft && !draft.tiers) draft.tiers = [];
  }

  function num(v: unknown): number | null {
    if (v === "" || v === null || v === undefined) return null;
    const n = Number(v);
    return isFinite(n) ? n : null;
  }

  async function saveEdit() {
    if (!app.settings || !editModel || !draft) return;
    const spec: PriceSpec = {
      input: num(draft.input) ?? 0,
      output: num(draft.output) ?? 0,
      cache_read: num(draft.cache_read),
      cache_write_5m: num(draft.cache_write_5m),
      cache_write_1h: num(draft.cache_write_1h),
      tiers: (draft.tiers ?? [])
        .filter((t) => num(t.above) !== null)
        .map((t) => ({
          above: num(t.above)!,
          input: num(t.input),
          output: num(t.output),
          cache_read: num(t.cache_read),
          cache_write_5m: num(t.cache_write_5m),
          cache_write_1h: num(t.cache_write_1h),
        })),
      fast_multiplier: num(draft.fast_multiplier),
    };
    await saveSettings({ ...app.settings, price_overrides: { ...app.settings.price_overrides, [editModel]: spec } });
    await loadMeta();
    editModel = null;
    draft = null;
    toast("単価を保存しました");
  }

  async function resetOverride(model: string) {
    if (!app.settings) return;
    const o = { ...app.settings.price_overrides };
    delete o[model];
    const a = { ...app.settings.aliases };
    delete a[model];
    await saveSettings({ ...app.settings, price_overrides: o, aliases: a });
    await loadMeta();
  }

  function startAlias(mi: ModelInfo) {
    aliasModel = mi.name;
    editModel = null;
    aliasTarget = app.settings?.aliases[mi.name] ?? "";
  }

  async function saveAlias() {
    if (!app.settings || !aliasModel) return;
    const target = aliasTarget.trim();
    if (target && !catalogByKey.has(target.toLowerCase()) && !app.settings.price_overrides[target]) {
      toast(`「${target}」は価格表にありません`, "error");
      return;
    }
    const a = { ...app.settings.aliases };
    if (target) a[aliasModel] = target;
    else delete a[aliasModel];
    await saveSettings({ ...app.settings, aliases: a });
    await loadMeta();
    aliasModel = null;
  }

  const results = $derived.by(() => {
    const q = search.trim().toLowerCase();
    if (!q) return [];
    return catalog.filter((c) => c.key.includes(q)).slice(0, 80);
  });

  function addTier() {
    draft?.tiers?.push({ above: 200000, input: null, output: null } as Tier);
  }
</script>

<div class="page">
  <div class="row">
    <h1>価格表</h1>
    <span class="spacer"></span>
    <span class="muted tiny">
      LiteLLM: {app.meta?.litellm_fetched_at ? dateTime(Date.parse(app.meta.litellm_fetched_at)) + " 取得" : `内蔵版（${app.meta?.snapshot_date}）`}
    </span>
    <button class="btn" onclick={updateNow} disabled={updating}>{updating ? "取得中…" : "LiteLLM から最新価格を取得"}</button>
  </div>

  <div class="card settings">
    <div class="row">
      <span class="secondary">優先する価格:</span>
      <div class="seg">
        <button class:on={app.settings?.price_priority === "curated"} onclick={() => setPriority("curated")}>公式価格（内蔵）を優先</button>
        <button class:on={app.settings?.price_priority === "litellm"} onclick={() => setPriority("litellm")}>LiteLLM を優先</button>
      </div>
      <label class="toggle">
        <input type="checkbox" checked={app.settings?.auto_update_prices} onchange={(e) => setAuto(e.currentTarget.checked)} />
        起動時に価格表と為替レートを自動更新
      </label>
    </div>
    <p class="muted tiny">
      単価はすべて 100 万トークンあたりの USD です。決定順: ユーザー設定 → 公式価格（内蔵）/ LiteLLM（優先設定による）→ LiteLLM 内蔵版。
      キャッシュ単価が空欄のモデルは、読込 = 入力×0.1、書込 = 入力×1.25（1時間キャッシュは×2）で計算します。
    </p>
  </div>

  <div class="card">
    <div class="card-head"><h2>使用中のモデル</h2><span class="muted tiny">全期間の集計</span></div>
    <div class="scroll">
      <table class="data">
        <thead>
          <tr>
            <th>モデル</th>
            <th class="r">リクエスト</th>
            <th class="r">トークン</th>
            <th class="r">API換算</th>
            <th>価格の出典</th>
            <th class="r">入力</th>
            <th class="r">出力</th>
            <th class="r">キャッシュ読込</th>
            <th class="r">書込 5分</th>
            <th class="r">書込 1時間</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each models as mi (mi.name)}
            {@const p = mi.price}
            {@const diff = litellmDiff(mi)}
            <tr>
              <td>
                <div class="row nowrap">
                  {#each mi.services as s}<span class="dot" style:background={serviceColor(s)} title={SERVICE_LABEL[s]}></span>{/each}
                  <b>{mi.name}</b>
                </div>
                {#if p?.alias}<div class="muted tiny">→ {p.alias} の価格を使用</div>{/if}
                {#if p && p.key !== mi.name.toLowerCase() && !p.alias && p.source !== "user"}<div class="muted tiny">（{p.key} として照合）</div>{/if}
              </td>
              <td class="r">{int(mi.requests)}</td>
              <td class="r">{compact(mi.tokens)}</td>
              <td class="r">{money(mi.cost, m)}</td>
              <td>
                {#if p}
                  <span class="badge {SOURCE[p.source].cls}">{SOURCE[p.source].label}</span>
                  {#if diff}
                    <span class="badge warn" title="LiteLLM: 入力 {rate(diff.input)} / 出力 {rate(diff.output)} / 読込 {rate(diff.cache_read)} / 書込 {rate(diff.cache_write_5m)}">LiteLLM と差異</span>
                  {/if}
                  {#if p.spec.tiers?.length}<span class="badge" title={p.spec.tiers.map((t) => `${compact(t.above)} 超: 入力 ${rate(t.input)} / 出力 ${rate(t.output)}`).join("\n")}>長文脈料金あり</span>{/if}
                {:else}
                  <span class="badge critical">未設定（0 として計算）</span>
                {/if}
              </td>
              <td class="r">{rate(p?.spec.input)}</td>
              <td class="r">{rate(p?.spec.output)}</td>
              <td class="r">{p ? rate(p.spec.cache_read, p.spec.input * 0.1) : "—"}</td>
              <td class="r">{p ? rate(p.spec.cache_write_5m, p.spec.input * 1.25) : "—"}</td>
              <td class="r">{p && (p.spec.cache_write_1h != null || mi.services.includes("claude")) ? rate(p.spec.cache_write_1h, p.spec.input * 2) : "—"}</td>
              <td class="actions">
                <button class="btn small" onclick={() => startEdit(mi)}>単価を編集</button>
                <button class="btn small" onclick={() => startAlias(mi)}>別モデルの価格</button>
                {#if app.settings?.price_overrides[mi.name] || app.settings?.aliases[mi.name]}
                  <button class="btn small ghost" onclick={() => resetOverride(mi.name)}>元に戻す</button>
                {/if}
              </td>
            </tr>
            {#if editModel === mi.name && draft}
              <tr class="editor">
                <td colspan="11">
                  <div class="edit-grid">
                    <div class="field"><label for="e-in">入力</label><input id="e-in" class="input" type="number" step="any" bind:value={draft.input} /></div>
                    <div class="field"><label for="e-out">出力</label><input id="e-out" class="input" type="number" step="any" bind:value={draft.output} /></div>
                    <div class="field"><label for="e-cr">キャッシュ読込</label><input id="e-cr" class="input" type="number" step="any" bind:value={draft.cache_read} placeholder="入力×0.1" /></div>
                    <div class="field"><label for="e-cw5">書込 5分</label><input id="e-cw5" class="input" type="number" step="any" bind:value={draft.cache_write_5m} placeholder="入力×1.25" /></div>
                    <div class="field"><label for="e-cw1">書込 1時間</label><input id="e-cw1" class="input" type="number" step="any" bind:value={draft.cache_write_1h} placeholder="入力×2" /></div>
                    <div class="field"><label for="e-fast">fast モード倍率</label><input id="e-fast" class="input" type="number" step="any" bind:value={draft.fast_multiplier} placeholder="1" /></div>
                  </div>
                  <div class="tiers">
                    <div class="row"><span class="secondary">長文脈料金（プロンプトが指定トークン数を超えたリクエスト全体に適用）</span><button class="btn small" onclick={addTier}>＋ 追加</button></div>
                    {#each draft.tiers ?? [] as t, i}
                      <div class="row tier">
                        <input class="input" type="number" bind:value={t.above} style="width:110px" aria-label="閾値" /><span class="muted">トークン超:</span>
                        <input class="input" type="number" step="any" bind:value={t.input} placeholder="入力" style="width:90px" aria-label="入力" />
                        <input class="input" type="number" step="any" bind:value={t.output} placeholder="出力" style="width:90px" aria-label="出力" />
                        <input class="input" type="number" step="any" bind:value={t.cache_read} placeholder="読込" style="width:90px" aria-label="キャッシュ読込" />
                        <button class="btn small ghost danger" onclick={() => draft?.tiers?.splice(i, 1)}>削除</button>
                      </div>
                    {/each}
                  </div>
                  <div class="row">
                    <button class="btn primary small" onclick={saveEdit}>保存</button>
                    <button class="btn small" onclick={() => ((editModel = null), (draft = null))}>キャンセル</button>
                  </div>
                </td>
              </tr>
            {/if}
            {#if aliasModel === mi.name}
              <tr class="editor">
                <td colspan="11">
                  <div class="row">
                    <span class="secondary">{mi.name} の計算に使うモデル:</span>
                    <input class="input" list="catalog-keys" bind:value={aliasTarget} placeholder="例: gpt-5.6-luna" style="width:260px" />
                    <button class="btn primary small" onclick={saveAlias}>保存</button>
                    <button class="btn small" onclick={() => (aliasModel = null)}>キャンセル</button>
                    <span class="muted tiny">空欄で保存すると解除します</span>
                  </div>
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
    <datalist id="catalog-keys">
      {#each catalog as c (c.key)}<option value={c.key}></option>{/each}
    </datalist>
  </div>

  <div class="card">
    <div class="card-head">
      <h2>価格を検索</h2>
      <input class="input" placeholder="モデル名で検索（例: sonnet, gpt-5）" bind:value={search} style="width:300px" />
      <span class="muted tiny">{catalog.length} モデル収録</span>
    </div>
    {#if results.length > 0}
      <table class="data">
        <thead>
          <tr><th>モデル</th><th>出典</th><th class="r">入力</th><th class="r">出力</th><th class="r">キャッシュ読込</th><th class="r">書込</th><th>長文脈</th></tr>
        </thead>
        <tbody>
          {#each results as c (c.key)}
            {#each [c.curated ? { s: c.curated, l: "公式価格（内蔵）", cls: "good" } : null, c.litellm ? { s: c.litellm, l: c.litellm_latest ? "LiteLLM 最新" : "LiteLLM 内蔵", cls: "" } : null].filter(Boolean) as x}
              <tr>
                <td>{c.key}</td>
                <td><span class="badge {x!.cls}">{x!.l}</span></td>
                <td class="r">{rate(x!.s.input)}</td>
                <td class="r">{rate(x!.s.output)}</td>
                <td class="r">{rate(x!.s.cache_read)}</td>
                <td class="r">{rate(x!.s.cache_write_5m)}</td>
                <td>{x!.s.tiers?.length ? x!.s.tiers.map((t) => compact(t.above) + "超").join(", ") : ""}</td>
              </tr>
            {/each}
          {/each}
        </tbody>
      </table>
    {:else if search.trim()}
      <div class="muted">見つかりませんでした</div>
    {/if}
  </div>
</div>

<style>
  .page {
    padding: 22px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .tiny {
    font-size: 11.5px;
  }
  .settings p {
    margin: 10px 0 0;
  }
  .toggle {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    color: var(--text-2);
    cursor: pointer;
    margin-left: 8px;
  }
  .scroll {
    overflow-x: auto;
  }
  .nowrap {
    flex-wrap: nowrap;
    gap: 4px;
  }
  .actions {
    white-space: nowrap;
    text-align: right;
  }
  .actions .btn + .btn {
    margin-left: 4px;
  }
  tr.editor td {
    background: var(--surface-2);
  }
  tr.editor:hover td {
    background: var(--surface-2);
  }
  .edit-grid {
    display: grid;
    grid-template-columns: repeat(6, minmax(110px, 1fr));
    gap: 10px;
    margin-bottom: 10px;
  }
  .tiers {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
  }
  .tier {
    flex-wrap: nowrap;
  }
</style>
