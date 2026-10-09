<script lang="ts">
  import Chart from "../components/Chart.svelte";
  import StatTile from "../components/StatTile.svelte";
  import { api } from "../lib/api";
  import { app, money as moneyCtx, saveSettings, toast } from "../lib/state.svelte";
  import { SERVICE_LABEL, convert, date, moneyAxis, moneyIn, ratio, toDisplay, ymd } from "../lib/format";
  import { chrome, serviceColor, theme } from "../lib/colors.svelte";
  import { base, categoryAxis, tipRow, valueAxis } from "../lib/echarts";
  import type { Plan, PlanReport, ServiceId } from "../lib/types";

  const PRESETS: { service: ServiceId; name: string; price: number; currency: string }[] = [
    { service: "claude", name: "Claude Pro", price: 20, currency: "USD" },
    { service: "claude", name: "Claude Max 5x", price: 100, currency: "USD" },
    { service: "claude", name: "Claude Max 20x", price: 200, currency: "USD" },
    { service: "codex", name: "ChatGPT Plus", price: 20, currency: "USD" },
    { service: "codex", name: "ChatGPT Pro", price: 100, currency: "USD" },
    { service: "codex", name: "ChatGPT Pro", price: 200, currency: "USD" },
  ];

  let reports = $state<PlanReport[]>([]);
  let editing = $state<Plan | null>(null);
  let isNew = $state(false);

  const m = $derived(moneyCtx());

  $effect(() => {
    app.dataVersion;
    app.settings;
    api.subscriptions().then((r) => (reports = r)).catch((e) => toast(String(e), "error"));
  });

  /**
   * ログが残っている期間だけで料金と API 換算額を比べる。Claude Code は古いログを
   * 自動削除するため、それ以前のサイクルを含めると倍率が実際より低く出てしまう。
   */
  function covered(r: PlanReport) {
    const first = app.meta?.service_first_ts?.[r.plan.service] ?? Infinity;
    // 最初に使用記録があるサイクルより前は、ログが消えた期間とみなす。
    const firstUsed = r.cycles.findIndex((c) => c.requests > 0);
    const noLog = r.cycles.map(
      (cy, i) => !cy.current && (cy.end <= first || firstUsed === -1 || i < firstUsed),
    );
    let fee = 0;
    let cost = 0;
    let cut = false;
    for (const [i, cy] of r.cycles.entries()) {
      if (noLog[i]) {
        cut = true;
        continue;
      }
      const share = cy.start < first ? (cy.end - first) / (cy.end - cy.start) : 1;
      if (share < 1) cut = true;
      fee += cy.fee * share;
      cost += cy.cost;
    }
    return { fee: convert(fee, r.plan.currency, m), cost: toDisplay(cost, m), cut, first, noLog };
  }

  const summary = $derived.by(() => {
    let fee = 0;
    let cost = 0;
    for (const r of reports) {
      const c = covered(r);
      fee += c.fee;
      cost += c.cost;
    }
    return { fee, cost, cut: reports.some((r) => covered(r).cut) };
  });

  // Codex がレート制限イベントで報告するプラン種別と、その開始時刻。
  const detected = $derived(app.meta?.plan_marks ?? []);

  function newPlan(seed: Partial<Plan> = {}) {
    editing = {
      id: crypto.randomUUID(),
      service: "claude",
      name: "",
      price: 0,
      currency: "USD",
      start: ymd(new Date(new Date().getFullYear(), new Date().getMonth(), 1)),
      end: null,
      ...seed,
    };
    isNew = true;
  }

  function edit(p: Plan) {
    editing = { ...p };
    isNew = false;
  }

  function applyPreset(e: Event) {
    const i = Number((e.target as HTMLSelectElement).value);
    const p = PRESETS[i];
    if (!p || !editing) return;
    editing.service = p.service;
    editing.name = p.name;
    editing.price = p.price;
    editing.currency = p.currency;
  }

  async function savePlan() {
    if (!app.settings || !editing) return;
    if (!editing.name.trim() || !(editing.price >= 0) || !editing.start) {
      toast("プラン名・月額・開始日を入力してください", "error");
      return;
    }
    if (editing.end && editing.end <= editing.start) {
      toast("終了日は開始日より後にしてください", "error");
      return;
    }
    const plan: Plan = { ...editing, end: editing.end || null, price: Number(editing.price) };
    const plans = isNew
      ? [...app.settings.plans, plan]
      : app.settings.plans.map((p) => (p.id === plan.id ? plan : p));
    plans.sort((a, b) => a.start.localeCompare(b.start));
    await saveSettings({ ...app.settings, plans });
    editing = null;
  }

  async function removePlan(p: Plan) {
    if (!app.settings) return;
    if (!confirm(`「${p.name}」を削除しますか？`)) return;
    await saveSettings({ ...app.settings, plans: app.settings.plans.filter((x) => x.id !== p.id) });
  }

  function current(r: PlanReport) {
    return r.cycles.find((c) => c.current) ?? null;
  }

  function cycleOption(r: PlanReport) {
    theme.dark;
    const c = chrome();
    const color = serviceColor(r.plan.service);
    const { first, noLog } = covered(r);
    const labels = r.cycles.map((cy) => date(cy.start).slice(2));
    const costs = r.cycles.map((cy) => toDisplay(cy.cost, m));
    const fees = r.cycles.map((cy) => convert(cy.fee, r.plan.currency, m));
    const partial = r.cycles.map((cy, i) => !noLog[i] && cy.start < first && cy.end > first);
    const b = base();
    return {
      ...b,
      grid: { left: 8, right: 12, top: 28, bottom: 8, containLabel: true },
      legend: { ...b.legend, top: 0, left: 0, data: ["API換算額", "支払額"] },
      tooltip: {
        ...b.tooltip,
        trigger: "axis",
        axisPointer: { type: "shadow", shadowStyle: { color: c.grid, opacity: 0.4 } },
        formatter: (ps: any[]) => {
          const i = ps[0].dataIndex;
          const cy = r.cycles[i];
          const note = noLog[i]
            ? `<div style="margin-top:4px;color:${c.muted}">この期間のログは残っていません</div>`
            : partial[i]
              ? `<div style="margin-top:4px;color:${c.muted}">${date(first)} より前のログは残っていません</div>`
              : "";
          return (
            `<div style="margin-bottom:4px;color:${c.secondary}">${date(cy.start)} 〜 ${date(cy.end - 1)}${cy.current ? "（進行中）" : ""}</div>` +
            tipRow(color, "API換算額", noLog[i] ? "—" : moneyIn(costs[i], m.currency)) +
            tipRow(c.muted, "支払額", moneyIn(fees[i], m.currency)) +
            tipRow("transparent", "倍率", fees[i] > 0 && !noLog[i] ? ratio(costs[i] / fees[i]) : "—", true) +
            note
          );
        },
      },
      xAxis: categoryAxis(labels),
      yAxis: valueAxis((v) => moneyAxis(v, m.currency)),
      series: [
        {
          name: "API換算額",
          type: "bar",
          barMaxWidth: 36,
          itemStyle: { color, borderRadius: [4, 4, 0, 0] },
          label: {
            show: r.cycles.length <= 14,
            position: "top",
            color: c.secondary,
            fontSize: 11,
            formatter: (p: any) => {
              const i = p.dataIndex;
              if (noLog[i]) return "ログなし";
              if (!(fees[i] > 0)) return "";
              return ratio(p.value / fees[i]) + (partial[i] ? "*" : "");
            },
          },
          data: costs.map((v, i) => ({
            value: noLog[i] ? 0 : v,
            itemStyle: r.cycles[i].current || partial[i] ? { color, opacity: 0.55 } : undefined,
          })),
        },
        {
          name: "支払額",
          type: "line",
          step: "middle",
          symbol: "none",
          lineStyle: { color: c.muted, width: 2 },
          itemStyle: { color: c.muted },
          data: fees,
        },
      ],
    };
  }

  function paceOption(r: PlanReport) {
    theme.dark;
    const c = chrome();
    const cy = current(r);
    if (!cy) return {};
    const color = serviceColor(r.plan.service);
    const fee = convert(cy.fee, r.plan.currency, m);
    const totalDays = Math.round((cy.end - cy.start) / 86_400_000);
    const days = Array.from({ length: totalDays }, (_, i) => {
      const d = new Date(cy.start + i * 86_400_000 + 3_600_000 * 12);
      return `${d.getMonth() + 1}/${d.getDate()}`;
    });
    let acc = 0;
    const cum = r.current_daily.map((p) => (acc += toDisplay(p.cost, m)));
    const n = cum.length;
    const projected = cy.elapsed > 0 ? acc / cy.elapsed : acc;
    const projLine = days.map((_, i) => (i < n - 1 ? null : i === n - 1 ? acc : acc + ((projected - acc) * (i - n + 1)) / Math.max(1, totalDays - n)));
    const feeLine = days.map((_, i) => (fee * (i + 1)) / totalDays);
    const b = base();
    return {
      ...b,
      grid: { left: 8, right: 12, top: 28, bottom: 8, containLabel: true },
      legend: { ...b.legend, top: 0, left: 0, data: ["累計 API換算額", "ペース予測", "料金（日割り累計）"] },
      tooltip: {
        ...b.tooltip,
        trigger: "axis",
        formatter: (ps: any[]) =>
          `<div style="margin-bottom:4px;color:${c.secondary}">${ps[0].axisValue}</div>` +
          ps
            .filter((p) => p.value !== null && p.value !== undefined)
            .map((p) => tipRow(p.color, p.seriesName, moneyIn(p.value, m.currency)))
            .join(""),
      },
      xAxis: { ...categoryAxis(days), boundaryGap: false },
      yAxis: valueAxis((v) => moneyAxis(v, m.currency)),
      series: [
        {
          name: "累計 API換算額",
          type: "line",
          symbol: "none",
          lineStyle: { width: 2, color },
          itemStyle: { color },
          areaStyle: { color, opacity: 0.14 },
          data: cum,
        },
        {
          name: "ペース予測",
          type: "line",
          symbol: "none",
          lineStyle: { width: 2, color, type: "dashed", opacity: 0.7 },
          itemStyle: { color },
          data: projLine,
        },
        {
          name: "料金（日割り累計）",
          type: "line",
          symbol: "none",
          lineStyle: { width: 1.5, color: c.muted, type: "dashed" },
          itemStyle: { color: c.muted },
          data: feeLine,
        },
      ],
    };
  }
</script>

<div class="page">
  <div class="row">
    <h1>元取り分析</h1>
    <span class="spacer"></span>
    <button class="btn primary" onclick={() => newPlan()}>＋ プランを追加</button>
  </div>
  <p class="muted lead">
    契約中のサブスクを登録すると、請求サイクルごとに API 従量課金ならいくらだったかと比べられます。料金は税込の実際の請求額で入力するのがおすすめです。
    Claude Code は既定で 30 日より古いログを削除するため、このアプリで記録を始める前の期間は少なめに表示されることがあります（最初に使用記録があるサイクルより前は「ログなし」として累計から除外します）。
  </p>

  {#if detected.length > 0}
    <div class="card detected">
      <div class="row">
        <h3>Codex のログから検出したプラン</h3>
        <span class="muted tiny">rate_limits.plan_type の変化</span>
      </div>
      <div class="row marks">
        {#each detected as [ts, plan], i}
          <span class="badge accent">{plan}</span>
          <span class="muted">{date(ts)}〜{i < detected.length - 1 ? date(detected[i + 1][0]) : "現在"}</span>
          <button class="btn small" onclick={() => newPlan({ service: "codex", name: `ChatGPT ${plan}`, start: ymd(new Date(ts)) })}>このプランを登録</button>
        {/each}
      </div>
    </div>
  {/if}

  {#if editing}
    <div class="card form">
      <div class="card-head"><h2>{isNew ? "プランを追加" : "プランを編集"}</h2></div>
      <div class="fields">
        <div class="field">
          <label for="preset">プリセット</label>
          <select id="preset" class="input" onchange={applyPreset}>
            <option value="">選択…</option>
            {#each PRESETS as p, i}<option value={i}>{p.name}（{p.currency === "USD" ? "$" : "¥"}{p.price}）</option>{/each}
          </select>
        </div>
        <div class="field">
          <label for="svc">サービス</label>
          <select id="svc" class="input" bind:value={editing.service}>
            <option value="claude">Claude Code（Claude のサブスク）</option>
            <option value="codex">Codex（ChatGPT のサブスク）</option>
          </select>
        </div>
        <div class="field">
          <label for="pname">プラン名</label>
          <input id="pname" class="input" bind:value={editing.name} placeholder="例: Claude Max 5x" />
        </div>
        <div class="field">
          <label for="price">月額</label>
          <div class="row nowrap">
            <input id="price" class="input" type="number" min="0" step="any" bind:value={editing.price} style="width:120px" />
            <select class="input" bind:value={editing.currency} aria-label="通貨">
              <option value="USD">USD</option>
              <option value="JPY">JPY</option>
            </select>
          </div>
        </div>
        <div class="field">
          <label for="start">開始日（請求日）</label>
          <input id="start" class="input" type="date" bind:value={editing.start} />
        </div>
        <div class="field">
          <label for="end">終了日（解約・変更日）</label>
          <input id="end" class="input" type="date" bind:value={editing.end} />
        </div>
      </div>
      <p class="muted tiny">プリセットの金額は目安です。プラン変更時は旧プランに終了日を入れ、新プランを変更日から追加してください。</p>
      <div class="row">
        <button class="btn primary" onclick={savePlan}>保存</button>
        <button class="btn" onclick={() => (editing = null)}>キャンセル</button>
      </div>
    </div>
  {/if}

  {#if reports.length === 0 && !editing}
    <div class="card empty">
      <h2>プランが登録されていません</h2>
      <p class="muted">「＋ プランを追加」から、契約中のサブスクを登録してください。</p>
    </div>
  {:else if reports.length > 0}
    <div class="kpis">
      <StatTile
        label="累計 支払額"
        value={moneyIn(summary.fee, m.currency)}
        sub={summary.cut ? "ログが残っている期間の分のみ" : "登録プランの請求サイクル合計"}
      />
      <StatTile label="累計 API換算額" value={moneyIn(summary.cost, m.currency)} sub="同じ期間・同じサービス" />
      <StatTile label="元取り倍率" value={summary.fee > 0 ? ratio(summary.cost / summary.fee) : "—"} sub="API換算額 ÷ 支払額" />
      <StatTile
        label={summary.cost >= summary.fee ? "サブスクで得した額" : "API の方が安かった額"}
        value={moneyIn(Math.abs(summary.cost - summary.fee), m.currency)}
        sub="API換算額 − 支払額"
      />
    </div>

    {#each reports as r (r.plan.id)}
      {@const cy = current(r)}
      {@const fee = cy ? convert(cy.fee, r.plan.currency, m) : 0}
      {@const so = cy ? toDisplay(cy.cost, m) : 0}
      {@const proj = cy && cy.elapsed > 0 ? so / cy.elapsed : so}
      {@const cov = covered(r)}
      <div class="card plan">
        <div class="card-head">
          <span class="dot" style:background={serviceColor(r.plan.service)}></span>
          <h2>{r.plan.name}</h2>
          <span class="badge">{SERVICE_LABEL[r.plan.service]}</span>
          <span class="muted">
            {r.plan.currency === "USD" ? "$" : "¥"}{r.plan.price.toLocaleString()} / 月 ・ {r.plan.start.replaceAll("-", "/")} 〜 {r.plan.end ? r.plan.end.replaceAll("-", "/") : "継続中"}
          </span>
          <button class="btn small ghost" onclick={() => edit(r.plan)}>編集</button>
          <button class="btn small ghost danger" onclick={() => removePlan(r.plan)}>削除</button>
        </div>

        {#if r.cycles.length === 0}
          <p class="muted">開始日がまだ来ていません。</p>
        {:else}
          <div class="plan-stats">
            <div>
              <div class="muted tiny">累計{cov.cut ? "（ログが残っているサイクルのみ）" : ""}</div>
              <div class="big">{cov.fee > 0 ? ratio(cov.cost / cov.fee) : "—"}</div>
              <div class="muted tiny">{moneyIn(cov.cost, m.currency)} / 支払 {moneyIn(cov.fee, m.currency)}</div>
            </div>
            {#if cy}
              <div>
                <div class="muted tiny">今サイクル（{date(cy.start)}〜{date(cy.end - 1)}・{Math.round(cy.elapsed * 100)}% 経過）</div>
                <div class="big">{moneyIn(so, m.currency)}</div>
                <div class="muted tiny">料金 {moneyIn(fee, m.currency)} に対して {fee > 0 ? ratio(so / fee) : "—"}</div>
              </div>
              <div>
                <div class="muted tiny">このペースだとサイクル終了時</div>
                <div class="big">{moneyIn(proj, m.currency)}</div>
                <div class="muted tiny">予測倍率 {fee > 0 ? ratio(proj / fee) : "—"}</div>
              </div>
            {/if}
          </div>
          <div class="charts">
            <div>
              <h3>請求サイクルごとの比較</h3>
              <Chart option={cycleOption(r)} height={240} />
            </div>
            {#if cy}
              <div>
                <h3>今サイクルの累計ペース</h3>
                <Chart option={paceOption(r)} height={240} />
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {/each}
  {/if}
</div>

<style>
  .page {
    padding: 22px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .lead {
    margin: -6px 0 0;
  }
  .tiny {
    font-size: 11.5px;
  }
  .detected .marks {
    margin-top: 8px;
    gap: 10px;
  }
  .fields {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px;
  }
  .form .row {
    margin-top: 10px;
  }
  .nowrap {
    flex-wrap: nowrap;
    margin-top: 0 !important;
  }
  .empty p {
    margin: 8px 0 0;
  }
  .kpis {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 14px;
  }
  .plan-stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 16px;
    padding: 4px 0 14px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 14px;
  }
  .big {
    font-size: 22px;
    font-weight: 650;
  }
  .charts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(420px, 1fr));
    gap: 18px;
  }
  .charts h3 {
    margin-bottom: 6px;
    color: var(--text-2);
  }
</style>
