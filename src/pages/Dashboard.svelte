<script lang="ts">
  import FilterBar from "../components/FilterBar.svelte";
  import StatTile from "../components/StatTile.svelte";
  import Chart from "../components/Chart.svelte";
  import { api } from "../lib/api";
  import { app, backendFilter, filter, money as moneyCtx, persistUi } from "../lib/state.svelte";
  import { previousRange } from "../lib/periods";
  import {
    SERVICE_LABEL,
    compact,
    convert,
    int,
    money,
    moneyAxis,
    moneyIn,
    pct,
    projectLabel,
    ratio,
    dateTime,
    toDisplay,
  } from "../lib/format";
  import { assignColors, categorical, chrome, SEQ_BLUE, serviceColor, theme } from "../lib/colors.svelte";
  import { base, categoryAxis, tipRow, valueAxis } from "../lib/echarts";
  import type { Dashboard, Group, ServiceId } from "../lib/types";

  type Split = "model" | "service" | "project" | "none";
  type BucketSel = "auto" | "hour" | "day" | "week" | "month";

  let split = $state<Split>("model");
  let bucketSel = $state<BucketSel>("auto");
  let metric = $state<"cost" | "tokens">("cost");
  let cumulative = $state(false);

  let data = $state<Dashboard | null>(null);
  let prevTotal = $state<number | null>(null);
  let loading = $state(false);

  const m = $derived(moneyCtx());
  const modelOrder = $derived((app.meta?.models ?? []).map((x) => x.name));
  const projectOrder = $derived(
    [...(app.meta?.projects ?? [])].sort((a, b) => b.cost - a.cost).map((p) => p.path),
  );

  const bucket = $derived.by(() => {
    if (bucketSel !== "auto") return bucketSel;
    const f = backendFilter();
    const from = f.from ?? app.meta?.first_ts ?? Date.now();
    const to = Math.min(f.to ?? Date.now(), Date.now());
    const days = (to - from) / 86_400_000;
    if (days <= 2) return "hour";
    if (days > 180) return "month";
    if (days > 75) return "week";
    return "day";
  });

  let seq = 0;
  $effect(() => {
    // 依存: 絞り込み条件、グラフの設定、データと設定の更新。
    const f = backendFilter();
    const opts = { bucket, split, top: 7 };
    app.dataVersion;
    app.settings;
    const my = ++seq;
    loading = true;
    const t = setTimeout(async () => {
      try {
        const d = await api.dashboard(f, opts);
        let prev: number | null = null;
        if (f.from !== undefined && f.to !== undefined) {
          const to = Math.min(f.to, Date.now());
          const r = previousRange(f.from, to);
          const p = await api.dashboard({ ...f, from: r.from, to: r.to }, { bucket: "month", split: "none", top: 1 });
          prev = p.totals.requests > 0 ? p.totals.total_cost : null;
        }
        if (my === seq) {
          data = d;
          prevTotal = prev;
        }
      } catch (e) {
        console.error(e);
      } finally {
        if (my === seq) loading = false;
      }
    }, 40);
    return () => clearTimeout(t);
  });

  // ------------------------------------------------------------ 指標
  const feeTotal = $derived(
    data ? data.fees.reduce((s, f) => s + convert(f.amount, f.currency, m), 0) : 0,
  );
  const apiDisplay = $derived(data ? toDisplay(data.totals.total_cost, m) : 0);
  // 元取り倍率の分子: プランの契約期間内の利用だけ（契約していない期間の利用で倍率が上がらないように）
  const planDisplay = $derived(data ? toDisplay(data.plan_cost, m) : 0);
  const delta = $derived(
    data && prevTotal && prevTotal > 0 ? data.totals.total_cost / prevTotal - 1 : null,
  );
  const perDay = $derived(data && data.active_days > 0 ? data.totals.total_cost / data.active_days : 0);
  const t = $derived(data?.totals.tokens);
  const totalTokens = $derived(t ? t.input + t.output + t.cache_read + t.cache_write_5m + t.cache_write_1h : 0);

  // ------------------------------------------------------------ 推移グラフ
  function seriesName(key: string): string {
    if (key === "__other__") return "その他";
    if (split === "service") return SERVICE_LABEL[key as ServiceId] ?? key;
    if (split === "project") return projectLabel(key);
    if (split === "none") return metric === "cost" ? "API換算額" : "トークン";
    return key;
  }

  function bucketLabel(b: string): string {
    if (bucket === "hour") return b.slice(5).replace("-", "/");
    if (bucket === "month") return b.replace("-", "/");
    return b.slice(5).replace("-", "/");
  }

  const mainOption = $derived.by(() => {
    if (!data) return {};
    theme.dark;
    const c = chrome();
    const keys = data.series.map((s) => s.key);
    const colors =
      split === "service"
        ? keys.map((k) => (k === "__other__" ? c.muted : serviceColor(k)))
        : split === "none"
          ? [categorical()[0]]
          : assignColors(keys, split === "model" ? modelOrder : projectOrder);
    const conv = (v: number) => (metric === "cost" ? toDisplay(v, m) : v);
    const fmt = (v: number) => (metric === "cost" ? moneyAxis(v, m.currency) : compact(v));
    const series = data.series.map((s, i) => {
      let vals = (metric === "cost" ? s.cost : s.tokens).map(conv);
      if (cumulative) {
        let acc = 0;
        vals = vals.map((v) => (acc += v));
      }
      return cumulative
        ? {
            name: seriesName(s.key),
            type: "line",
            stack: "all",
            symbol: "none",
            lineStyle: { width: 2, color: colors[i] },
            areaStyle: { opacity: 0.18, color: colors[i] },
            itemStyle: { color: colors[i] },
            data: vals,
          }
        : {
            name: seriesName(s.key),
            type: "bar",
            stack: "all",
            barMaxWidth: 28,
            itemStyle: { color: colors[i], borderColor: c.surface, borderWidth: 1 },
            emphasis: { focus: "series" },
            data: vals,
          };
    });
    const b = base();
    return {
      ...b,
      color: colors,
      grid: { left: 8, right: 16, top: 36, bottom: 8, containLabel: true },
      legend: { ...b.legend, top: 0, left: 0, show: data.series.length > 1 },
      tooltip: {
        ...b.tooltip,
        trigger: "axis",
        axisPointer: { type: cumulative ? "line" : "shadow", shadowStyle: { color: c.grid, opacity: 0.4 } },
        formatter: (ps: any[]) => {
          const rows = [...ps].filter((p) => p.value > 0).sort((a, b) => b.value - a.value);
          const total = ps.reduce((s, p) => s + (p.value || 0), 0);
          return (
            `<div style="margin-bottom:4px;color:${c.secondary}">${ps[0]?.axisValue ?? ""}</div>` +
            (ps.length > 1 ? tipRow("transparent", "合計", fmt(total), true) : "") +
            rows.map((p) => tipRow(p.color, p.seriesName, fmt(p.value))).join("")
          );
        },
      },
      xAxis: { ...categoryAxis(data.buckets, bucketLabel), boundaryGap: !cumulative },
      yAxis: valueAxis(fmt),
      dataZoom: data.buckets.length > 120 ? [{ type: "inside" }] : [],
      series,
    };
  });

  // ------------------------------------------------------------ ヒートマップ
  const heatOption = $derived.by(() => {
    if (!data) return {};
    theme.dark;
    const c = chrome();
    const days = ["月", "火", "水", "木", "金", "土", "日"];
    const cells: [number, number, number][] = [];
    let max = 0;
    data.heatmap.forEach((row, d) =>
      row.forEach((v, h) => {
        const dv = toDisplay(v, m);
        max = Math.max(max, dv);
        cells.push([h, d, dv]);
      }),
    );
    const b = base();
    return {
      ...b,
      grid: { left: 8, right: 8, top: 8, bottom: 40, containLabel: true },
      tooltip: {
        ...b.tooltip,
        formatter: (p: any) =>
          `${days[p.value[1]]}曜 ${p.value[0]}:00〜${p.value[0] + 1}:00<br/><b>${moneyIn(p.value[2], m.currency)}</b>`,
      },
      xAxis: {
        ...categoryAxis(Array.from({ length: 24 }, (_, i) => String(i))),
        splitArea: { show: false },
      },
      yAxis: { ...categoryAxis(days), inverse: true },
      visualMap: {
        min: 0,
        max: max || 1,
        calculable: false,
        text: [moneyAxis(max, m.currency), moneyAxis(0, m.currency)],
        orient: "horizontal",
        left: "center",
        bottom: 0,
        itemWidth: 10,
        itemHeight: 140,
        textStyle: { color: c.muted, fontSize: 11 },
        formatter: (v: number) => moneyAxis(v, m.currency),
        inRange: { color: theme.dark ? [c.surface, ...SEQ_BLUE.slice(2)] : ["#f4f3ef", ...SEQ_BLUE] },
      },
      series: [
        {
          type: "heatmap",
          data: cells,
          itemStyle: { borderColor: c.surface, borderWidth: 2, borderRadius: 3 },
          emphasis: { itemStyle: { borderColor: c.text, borderWidth: 1 } },
        },
      ],
    };
  });

  // ------------------------------------------------------------ 内訳
  const modelColors = $derived.by(() => {
    theme.dark;
    return data ? assignColors(data.by_model.map((g) => g.key), modelOrder) : [];
  });
  const projectColors = $derived.by(() => {
    theme.dark;
    return data ? assignColors(data.by_project.slice(0, 10).map((g) => g.key), projectOrder) : [];
  });

  const composition = $derived.by(() => {
    if (!data) return [];
    theme.dark;
    const c = data.totals.cost;
    const cat = categorical();
    return [
      { label: "出力", v: c.output, color: cat[0] },
      { label: "入力", v: c.input, color: cat[1] },
      { label: "キャッシュ書込", v: c.cache_write, color: cat[2] },
      { label: "キャッシュ読込", v: c.cache_read, color: cat[3] },
      { label: "Web検索", v: c.web, color: cat[4] },
    ].filter((x) => x.v > 0);
  });

  function addModel(name: string) {
    if (!filter.models.includes(name)) filter.models = [...filter.models, name];
    persistUi();
  }
  function addProject(path: string) {
    if (!filter.projects.includes(path)) filter.projects = [...filter.projects, path];
    persistUi();
  }

  function share(g: Group): number {
    return data && data.totals.total_cost > 0 ? g.agg.total_cost / data.totals.total_cost : 0;
  }

  const rl = $derived(app.meta?.rate_limit ?? null);
  function windowLabel(min: number | null): string {
    if (!min) return "枠";
    if (min % 10080 === 0) return "週";
    if (min % 1440 === 0) return `${min / 1440}日`;
    if (min % 60 === 0) return `${min / 60}時間`;
    return `${min}分`;
  }
</script>

<FilterBar />

<div class="page" class:stale={loading && data}>
  {#if !data}
    <div class="muted">集計中…</div>
  {:else if data.totals.requests === 0}
    <div class="card empty">
      <h2>この条件に一致する使用履歴はありません</h2>
      <p class="muted">期間やサービスの条件を変えてみてください。ログの場所は「設定」で確認できます。</p>
    </div>
  {:else}
    {#if data.unpriced.length > 0}
      <div class="banner">
        <span class="badge warn">価格未設定</span>
        <span>
          {data.unpriced.map((u) => u.model).join(", ")} の単価が不明なため 0 として計算しています（{compact(
            data.unpriced.reduce((s, u) => s + u.tokens, 0),
          )} トークン）。
        </span>
        <span class="spacer"></span>
        <button class="btn small" onclick={() => (app.page = "pricing")}>価格表で設定</button>
      </div>
    {/if}

    <div class="kpis">
      <StatTile
        label="API換算額"
        value={money(data.totals.total_cost, m)}
        sub={`1日あたり ${money(perDay, m)}（稼働 ${data.active_days} 日）`}
        {delta}
      />
      <StatTile
        label="元取り倍率"
        value={feeTotal > 0 ? ratio(planDisplay / feeTotal) : "—"}
        sub={feeTotal > 0 ? `料金 ${moneyIn(feeTotal, m.currency)}（期間按分）` : "プランを登録すると表示されます"}
      >
        {#if feeTotal > 0}
          <div class="meter" title="料金のうち API 換算で使い切った割合">
            <div class="fill" style:width="{Math.min(100, (planDisplay / feeTotal) * 100)}%"></div>
          </div>
          <div class="muted tiny">
            {planDisplay >= feeTotal
              ? `サブスクの方が ${moneyIn(planDisplay - feeTotal, m.currency)} お得`
              : `API 従量課金の方が ${moneyIn(feeTotal - planDisplay, m.currency)} 安い`}
          </div>
          {#if data.subset}
            <div class="muted tiny">絞り込み中: 料金は絞り込めないため参考値です</div>
          {:else if planDisplay < apiDisplay * 0.995}
            <div class="muted tiny">プランの契約期間外の {moneyIn(apiDisplay - planDisplay, m.currency)} は除外</div>
          {/if}
        {:else}
          <button class="btn small" style="align-self:flex-start" onclick={() => (app.page = "subscription")}>プランを登録</button>
        {/if}
      </StatTile>
      <StatTile
        label="トークン"
        value={compact(totalTokens)}
        sub={`入力 ${compact(t!.input)} ・ 出力 ${compact(t!.output)} ・ キャッシュ ${compact(t!.cache_read + t!.cache_write_5m + t!.cache_write_1h)}`}
      />
      <StatTile
        label="キャッシュで節約"
        value={money(data.totals.cost.cache_savings, m)}
        sub={`キャッシュ読込 ${compact(t!.cache_read)} トークン`}
      />
      <StatTile
        label="リクエスト"
        value={int(data.totals.requests)}
        sub={`${int(data.sessions)} セッション・サブエージェント ${pct(data.totals.total_cost > 0 ? data.subagent_cost / data.totals.total_cost : 0, 0)}`}
      />
    </div>

    {#if data.by_service.length > 1}
      <div class="card">
        <div class="card-head"><h2>サービス比較</h2></div>
        <div class="compare">
          {#each data.by_service as g}
            {@const s = g.key as ServiceId}
            <div class="cmp">
              <div class="row">
                <span class="dot" style:background={serviceColor(s)}></span>
                <b>{SERVICE_LABEL[s]}</b>
                <span class="spacer"></span>
                <span class="muted">{pct(share(g))}</span>
              </div>
              <div class="cmp-value">{money(g.agg.total_cost, m)}</div>
              <div class="bar-track"><div class="bar-fill" style:width="{share(g) * 100}%" style:background={serviceColor(s)}></div></div>
              <div class="cmp-stats muted num">
                <span>{compact(g.agg.tokens.input + g.agg.tokens.output + g.agg.tokens.cache_read + g.agg.tokens.cache_write_5m + g.agg.tokens.cache_write_1h)} トークン</span>
                <span>{int(g.agg.requests)} リクエスト</span>
                <span>{int(g.sessions)} セッション</span>
                <span>1リクエスト {money(g.agg.total_cost / Math.max(1, g.agg.requests), m)}</span>
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <div class="card">
      <div class="card-head">
        <h2>{metric === "cost" ? "API換算額" : "トークン"}の推移</h2>
        <div class="seg">
          <button class:on={metric === "cost"} onclick={() => (metric = "cost")}>金額</button>
          <button class:on={metric === "tokens"} onclick={() => (metric = "tokens")}>トークン</button>
        </div>
        <div class="seg">
          <button class:on={split === "model"} onclick={() => (split = "model")}>モデル別</button>
          <button class:on={split === "service"} onclick={() => (split = "service")}>サービス別</button>
          <button class:on={split === "project"} onclick={() => (split = "project")}>プロジェクト別</button>
          <button class:on={split === "none"} onclick={() => (split = "none")}>合計</button>
        </div>
        <div class="seg">
          {#each [["auto", "自動"], ["hour", "時間"], ["day", "日"], ["week", "週"], ["month", "月"]] as [v, l]}
            <button class:on={bucketSel === v} onclick={() => (bucketSel = v as BucketSel)}>{l}</button>
          {/each}
        </div>
        <label class="toggle"><input type="checkbox" bind:checked={cumulative} /> 累計</label>
      </div>
      <Chart option={mainOption} height={320} />
    </div>

    <div class="two">
      <div class="card">
        <div class="card-head"><h2>モデル別</h2><span class="muted tiny">クリックで絞り込み</span></div>
        <div class="bars">
          {#each data.by_model as g, i}
            <button class="bar-row" onclick={() => addModel(g.key)} title="{g.key} で絞り込む">
              <span class="dot" style:background={modelColors[i]}></span>
              <span class="name ellipsis">{g.key}</span>
              {#if g.service}<span class="badge">{SERVICE_LABEL[g.service]}</span>{/if}
              <span class="bar-track grow"><span class="bar-fill" style:width="{share(g) * 100}%" style:background={modelColors[i]}></span></span>
              <span class="val num">{money(g.agg.total_cost, m)}</span>
              <span class="tok num muted">{compact(g.agg.tokens.input + g.agg.tokens.output + g.agg.tokens.cache_read + g.agg.tokens.cache_write_5m + g.agg.tokens.cache_write_1h)}</span>
            </button>
          {/each}
        </div>
      </div>
      <div class="card">
        <div class="card-head"><h2>プロジェクト別</h2><span class="muted tiny">上位10件・クリックで絞り込み</span></div>
        <div class="bars">
          {#each data.by_project.slice(0, 10) as g, i}
            <button class="bar-row" onclick={() => addProject(g.key)} title={g.key}>
              <span class="dot" style:background={projectColors[i]}></span>
              <span class="name ellipsis">{projectLabel(g.key)}</span>
              <span class="bar-track grow"><span class="bar-fill" style:width="{share(g) * 100}%" style:background={projectColors[i]}></span></span>
              <span class="val num">{money(g.agg.total_cost, m)}</span>
              <span class="tok num muted">{int(g.sessions)} セッション</span>
            </button>
          {/each}
        </div>
      </div>
    </div>

    <div class="two">
      <div class="card">
        <div class="card-head"><h2>費用の内訳</h2></div>
        <div class="stack">
          {#each composition as x}
            <span style:flex-grow={x.v} style:background={x.color} title="{x.label}: {money(x.v, m)}"></span>
          {/each}
        </div>
        <table class="data comp">
          <tbody>
            {#each composition as x}
              <tr>
                <td><span class="dot" style:background={x.color}></span> {x.label}</td>
                <td class="r">{money(x.v, m)}</td>
                <td class="r muted">{pct(x.v / Math.max(1e-12, data.totals.total_cost))}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if rl}
          <div class="rl">
            <div class="row">
              <h3>Codex の使用枠</h3>
              {#if rl.plan_type}<span class="badge accent">{rl.plan_type}</span>{/if}
              <span class="spacer"></span>
              <span class="muted tiny">{dateTime(rl.ts)} 時点のログ</span>
            </div>
            {#each [rl.primary, rl.secondary] as w}
              {#if w}
                <div class="rl-row">
                  <span class="secondary">{windowLabel(w.window_minutes)}</span>
                  <span class="bar-track grow"><span class="bar-fill" style:width="{Math.min(100, w.used_percent)}%" style:background={serviceColor("codex")}></span></span>
                  <span class="num">{w.used_percent.toFixed(0)}%</span>
                  {#if w.resets_at}<span class="muted tiny">リセット {dateTime(w.resets_at * 1000)}</span>{/if}
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      </div>
      <div class="card">
        <div class="card-head"><h2>曜日 × 時間帯</h2><span class="muted tiny">API換算額</span></div>
        <Chart option={heatOption} height={250} />
      </div>
    </div>
  {/if}
</div>

<style>
  .page {
    padding: 18px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    transition: opacity 0.15s;
  }
  .page.stale {
    opacity: 0.6;
  }
  .empty p {
    margin: 8px 0 0;
  }
  .kpis {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 14px;
  }
  .tiny {
    font-size: 11.5px;
  }
  .meter {
    height: 6px;
    border-radius: 3px;
    background: var(--accent-soft);
    overflow: hidden;
    margin-top: 4px;
  }
  .meter .fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
  }
  .compare {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 24px;
  }
  .cmp {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .cmp-value {
    font-size: 22px;
    font-weight: 650;
  }
  .cmp-stats {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    font-size: 12px;
  }
  .toggle {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    color: var(--text-2);
    cursor: pointer;
  }
  .two {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(420px, 1fr));
    gap: 14px;
  }
  .bars {
    display: flex;
    flex-direction: column;
  }
  .bar-row {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 0;
    background: transparent;
    padding: 6px 6px;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
    min-width: 0;
  }
  .bar-row:hover {
    background: var(--surface-hover);
  }
  .name {
    width: 190px;
    flex: none;
  }
  .bar-track {
    height: 8px;
    border-radius: 4px;
    background: var(--surface-2);
    overflow: hidden;
    display: block;
  }
  .bar-track.grow {
    flex: 1;
    min-width: 40px;
  }
  .bar-fill {
    display: block;
    height: 100%;
    border-radius: 4px;
    min-width: 2px;
  }
  .val {
    width: 88px;
    text-align: right;
    flex: none;
  }
  .tok {
    width: 82px;
    text-align: right;
    flex: none;
    font-size: 12px;
  }
  .stack {
    display: flex;
    gap: 2px;
    height: 14px;
    border-radius: 4px;
    overflow: hidden;
    margin-bottom: 10px;
  }
  .stack span {
    flex-basis: 0;
    min-width: 2px;
  }
  .comp td {
    padding: 5px 6px;
  }
  .rl {
    margin-top: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .rl-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .rl-row .secondary {
    width: 48px;
  }
</style>
