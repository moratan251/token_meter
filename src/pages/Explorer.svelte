<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";
  import FilterBar from "../components/FilterBar.svelte";
  import { api } from "../lib/api";
  import { app, backendFilter, filter, money as moneyCtx, persistUi, toast } from "../lib/state.svelte";
  import { SERVICE_LABEL, compact, dateTime, int, money, projectLabel, shortId, toDisplay, ymd } from "../lib/format";
  import { serviceColor } from "../lib/colors.svelte";
  import type { Row } from "../lib/types";

  type GroupBy = "day" | "week" | "month" | "session" | "project" | "model" | "service";
  const GROUPS: [GroupBy, string][] = [
    ["day", "日"],
    ["week", "週"],
    ["month", "月"],
    ["session", "セッション"],
    ["project", "プロジェクト"],
    ["model", "モデル"],
    ["service", "サービス"],
  ];

  type SortKey = "key" | "cost" | "input" | "output" | "cw" | "cr" | "total" | "requests" | "sessions" | "last";

  let groupBy = $state<GroupBy>("session");
  let rows = $state<Row[]>([]);
  let loading = $state(false);
  let search = $state("");
  let sortKey = $state<SortKey>("cost");
  let sortDesc = $state(true);
  let limit = $state(300);

  const m = $derived(moneyCtx());
  const isTime = $derived(groupBy === "day" || groupBy === "week" || groupBy === "month");

  let seq = 0;
  $effect(() => {
    const f = backendFilter();
    const g = groupBy;
    app.dataVersion;
    app.settings;
    const my = ++seq;
    loading = true;
    api
      .table(f, g)
      .then((r) => {
        if (my === seq) rows = r;
      })
      .catch((e) => toast(String(e), "error"))
      .finally(() => {
        if (my === seq) loading = false;
      });
  });

  function setGroup(g: GroupBy) {
    groupBy = g;
    limit = 300;
    if (g === "day" || g === "week" || g === "month") {
      sortKey = "key";
      sortDesc = true;
    } else {
      sortKey = "cost";
      sortDesc = true;
    }
  }

  const tok = (r: Row) =>
    r.agg.tokens.input + r.agg.tokens.output + r.agg.tokens.cache_read + r.agg.tokens.cache_write_5m + r.agg.tokens.cache_write_1h;

  function label(r: Row): string {
    if (groupBy === "session") return r.title ?? shortId(r.key);
    if (groupBy === "project") return projectLabel(r.key);
    if (groupBy === "service") return SERVICE_LABEL[r.key as "claude"] ?? r.key;
    return r.key;
  }

  const sortVal: Record<SortKey, (r: Row) => number | string> = {
    key: (r) => (isTime ? r.key : label(r).toLowerCase()),
    cost: (r) => r.agg.total_cost,
    input: (r) => r.agg.tokens.input,
    output: (r) => r.agg.tokens.output,
    cw: (r) => r.agg.tokens.cache_write_5m + r.agg.tokens.cache_write_1h,
    cr: (r) => r.agg.tokens.cache_read,
    total: tok,
    requests: (r) => r.agg.requests,
    sessions: (r) => r.sessions,
    last: (r) => r.last_ts,
  };

  const visible = $derived.by(() => {
    const q = search.trim().toLowerCase();
    let list = q
      ? rows.filter((r) =>
          [r.key, r.title ?? "", r.project ?? "", r.models.join(" ")].join(" ").toLowerCase().includes(q),
        )
      : rows;
    const f = sortVal[sortKey];
    list = [...list].sort((a, b) => {
      const x = f(a);
      const y = f(b);
      const c = typeof x === "string" ? x.localeCompare(y as string) : (x as number) - (y as number);
      return sortDesc ? -c : c;
    });
    return list;
  });

  const totals = $derived.by(() => {
    const t = { cost: 0, input: 0, output: 0, cw: 0, cr: 0, total: 0, requests: 0 };
    for (const r of visible) {
      t.cost += r.agg.total_cost;
      t.input += r.agg.tokens.input;
      t.output += r.agg.tokens.output;
      t.cw += r.agg.tokens.cache_write_5m + r.agg.tokens.cache_write_1h;
      t.cr += r.agg.tokens.cache_read;
      t.total += tok(r);
      t.requests += r.agg.requests;
    }
    return t;
  });

  function sortBy(k: SortKey) {
    if (sortKey === k) sortDesc = !sortDesc;
    else {
      sortKey = k;
      sortDesc = k !== "key";
    }
  }

  function arrow(k: SortKey) {
    return sortKey === k ? (sortDesc ? " ↓" : " ↑") : "";
  }

  function openSession(r: Row) {
    filter.sessions = [{ id: r.key, label: label(r) }];
    if (filter.period !== "all") {
      filter.period = "all";
      filter.from = null;
      filter.to = null;
    }
    persistUi();
    app.page = "dashboard";
  }

  function csvCell(v: string | number): string {
    const s = String(v);
    return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  }

  async function exportCsv() {
    const groupName = GROUPS.find((g) => g[0] === groupBy)?.[1] ?? groupBy;
    const path = await save({
      defaultPath: `token-meter_${groupBy}_${ymd(new Date())}.csv`,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!path) return;
    const head = [
      groupName,
      ...(groupBy === "session" ? ["セッションID"] : []),
      "サービス",
      "プロジェクト",
      "モデル",
      "入力",
      "出力",
      "キャッシュ書込",
      "キャッシュ読込",
      "推論(出力の内数)",
      "合計トークン",
      "リクエスト",
      "セッション数",
      "API換算(USD)",
      `API換算(${m.currency})`,
      "最初",
      "最後",
    ];
    const lines = [head.map(csvCell).join(",")];
    for (const r of visible) {
      lines.push(
        [
          label(r),
          ...(groupBy === "session" ? [r.key] : []),
          r.service ? SERVICE_LABEL[r.service] : "複数",
          r.project ?? "",
          r.models.join(" / "),
          r.agg.tokens.input,
          r.agg.tokens.output,
          r.agg.tokens.cache_write_5m + r.agg.tokens.cache_write_1h,
          r.agg.tokens.cache_read,
          r.agg.tokens.reasoning,
          tok(r),
          r.agg.requests,
          r.sessions,
          r.agg.total_cost.toFixed(4),
          toDisplay(r.agg.total_cost, m).toFixed(m.currency === "JPY" ? 0 : 4),
          dateTime(r.first_ts),
          dateTime(r.last_ts),
        ]
          .map(csvCell)
          .join(","),
      );
    }
    try {
      await api.writeTextFile(path, lines.join("\r\n"));
      toast(`${visible.length} 行を書き出しました`);
    } catch (e) {
      toast(String(e), "error");
    }
  }
</script>

<FilterBar />

<div class="page">
  <div class="row head">
    <h1>明細</h1>
    <div class="seg">
      {#each GROUPS as [g, l]}
        <button class:on={groupBy === g} onclick={() => setGroup(g)}>{l}</button>
      {/each}
    </div>
    <input class="input search" placeholder="名前・パス・モデルで検索" bind:value={search} />
    <span class="spacer"></span>
    <span class="muted num">{visible.length.toLocaleString()} 行</span>
    <button class="btn" onclick={exportCsv} disabled={visible.length === 0}>CSV 書き出し</button>
  </div>

  <div class="card table-card" class:stale={loading}>
    <div class="scroll">
      <table class="data">
        <thead>
          <tr>
            <th class="sortable" onclick={() => sortBy("key")}>{GROUPS.find((g) => g[0] === groupBy)?.[1]}{arrow("key")}</th>
            {#if !isTime && groupBy !== "service"}<th>サービス</th>{/if}
            {#if groupBy !== "model"}<th>モデル</th>{/if}
            <th class="r sortable" onclick={() => sortBy("input")}>入力{arrow("input")}</th>
            <th class="r sortable" onclick={() => sortBy("output")}>出力{arrow("output")}</th>
            <th class="r sortable" onclick={() => sortBy("cw")}>キャッシュ書込{arrow("cw")}</th>
            <th class="r sortable" onclick={() => sortBy("cr")}>キャッシュ読込{arrow("cr")}</th>
            <th class="r sortable" onclick={() => sortBy("total")}>合計{arrow("total")}</th>
            <th class="r sortable" onclick={() => sortBy("requests")}>リクエスト{arrow("requests")}</th>
            {#if groupBy !== "session"}<th class="r sortable" onclick={() => sortBy("sessions")}>セッション{arrow("sessions")}</th>{/if}
            <th class="r sortable" onclick={() => sortBy("cost")}>API換算{arrow("cost")}</th>
            {#if !isTime}<th class="r sortable" onclick={() => sortBy("last")}>最終利用{arrow("last")}</th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each visible.slice(0, limit) as r (r.key)}
            <tr>
              <td class="keycell">
                {#if groupBy === "session"}
                  <button class="link ellipsis" onclick={() => openSession(r)} title="このセッションをダッシュボードで見る（{r.key}）">{label(r)}</button>
                  {#if r.project}<div class="muted sub ellipsis" title={r.project}>{projectLabel(r.project)}</div>{/if}
                {:else}
                  <span class="ellipsis" title={r.key}>{label(r)}</span>
                {/if}
              </td>
              {#if !isTime && groupBy !== "service"}
                <td>
                  {#if r.service}
                    <span class="row nowrap"><span class="dot" style:background={serviceColor(r.service)}></span>{SERVICE_LABEL[r.service]}</span>
                  {:else}<span class="muted">複数</span>{/if}
                </td>
              {/if}
              {#if groupBy !== "model"}
                <td class="models ellipsis" title={r.models.join("\n")}>
                  {r.models[0] ?? ""}{#if r.models.length > 1}<span class="muted"> +{r.models.length - 1}</span>{/if}
                </td>
              {/if}
              <td class="r">{compact(r.agg.tokens.input)}</td>
              <td class="r">{compact(r.agg.tokens.output)}</td>
              <td class="r">{compact(r.agg.tokens.cache_write_5m + r.agg.tokens.cache_write_1h)}</td>
              <td class="r">{compact(r.agg.tokens.cache_read)}</td>
              <td class="r">{compact(tok(r))}</td>
              <td class="r">{int(r.agg.requests)}</td>
              {#if groupBy !== "session"}<td class="r">{int(r.sessions)}</td>{/if}
              <td class="r strong">{money(r.agg.total_cost, m)}</td>
              {#if !isTime}<td class="r muted">{dateTime(r.last_ts)}</td>{/if}
            </tr>
          {/each}
        </tbody>
        {#if visible.length > 1}
          <tfoot>
            <tr>
              <td><b>合計</b></td>
              {#if !isTime && groupBy !== "service"}<td></td>{/if}
              {#if groupBy !== "model"}<td></td>{/if}
              <td class="r">{compact(totals.input)}</td>
              <td class="r">{compact(totals.output)}</td>
              <td class="r">{compact(totals.cw)}</td>
              <td class="r">{compact(totals.cr)}</td>
              <td class="r">{compact(totals.total)}</td>
              <td class="r">{int(totals.requests)}</td>
              {#if groupBy !== "session"}<td></td>{/if}
              <td class="r strong">{money(totals.cost, m)}</td>
              {#if !isTime}<td></td>{/if}
            </tr>
          </tfoot>
        {/if}
      </table>
    </div>
    {#if visible.length > limit}
      <div class="more"><button class="btn" onclick={() => (limit += 500)}>さらに表示（残り {visible.length - limit} 行）</button></div>
    {/if}
    {#if !loading && visible.length === 0}
      <div class="muted pad">該当するデータがありません</div>
    {/if}
  </div>
</div>

<style>
  .page {
    padding: 18px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .head {
    gap: 12px;
  }
  .search {
    width: 260px;
  }
  .table-card {
    padding: 0;
    transition: opacity 0.15s;
  }
  .table-card.stale {
    opacity: 0.6;
  }
  .scroll {
    overflow: auto;
    max-height: calc(100vh - 230px);
    border-radius: var(--radius);
  }
  .keycell {
    max-width: 280px;
    min-width: 160px;
  }
  .keycell .ellipsis {
    display: block;
  }
  .sub {
    font-size: 11.5px;
  }
  .models {
    max-width: 170px;
  }
  .link {
    border: 0;
    background: transparent;
    padding: 0;
    color: var(--accent-text);
    cursor: pointer;
    text-align: left;
    max-width: 100%;
  }
  .link:hover {
    text-decoration: underline;
  }
  .strong {
    font-weight: 600;
  }
  .nowrap {
    flex-wrap: nowrap;
    gap: 6px;
    white-space: nowrap;
  }
  tfoot td {
    position: sticky;
    bottom: 0;
    background: var(--surface);
    border-top: 1px solid var(--border-strong);
    font-weight: 600;
  }
  .more {
    padding: 12px;
    text-align: center;
  }
  .pad {
    padding: 16px;
  }
</style>
