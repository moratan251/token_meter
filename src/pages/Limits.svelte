<script lang="ts">
  import { untrack } from "svelte";
  import Chart from "../components/Chart.svelte";
  import ManualLimitList from "../components/ManualLimitList.svelte";
  import { api } from "../lib/api";
  import { app, money as moneyCtx, saveSettings, toast } from "../lib/state.svelte";
  import { SERVICE_LABEL, convert, dateTime, money, moneyAxis, moneyIn, ratio, relative, toDisplay } from "../lib/format";
  import { chrome, serviceColor, theme } from "../lib/colors.svelte";
  import { base, tipRow } from "../lib/echarts";
  import { parseUsageText } from "../lib/usageParse";
  import type { LimitKind, LimitReport, LimitSeries, ManualLimit, ServiceId } from "../lib/types";

  let report = $state<LimitReport | null>(null);
  let paste = $state("");
  let rows = $state<{ service: ServiceId; kind: LimitKind; model: string; percent: number | null; hours: number | null; minutes: number | null }[]>([]);

  const m = $derived(moneyCtx());

  $effect(() => {
    app.dataVersion;
    app.settings;
    api.limits().then((r) => (report = r)).catch((e) => toast(String(e), "error"));
  });

  const SOURCE_LABEL: Record<string, string> = { desktop: "デスクトップアプリ", log: "ログ", manual: "手動" };

  function kindLabel(s: { kind: string; model: string | null; window_minutes: number }): string {
    const k = s.kind.includes(":") ? s.kind.split(":").pop()! : s.kind;
    if (k === "session") return "5時間枠";
    if (k === "weekly") return "週間（すべてのモデル）";
    if (k === "weekly_model") return `週間（${s.model ?? "モデル別"}）`;
    const h = s.window_minutes / 60;
    return h >= 24 ? `${h / 24}日枠` : `${h}時間枠`;
  }

  const BASIS: Record<LimitSeries["basis"], string> = {
    current: "今の枠の実績",
    recent_window: "直近の枠の実績",
    average: "過去30日の平均",
  };

  const pct = (v: number) => `${v.toFixed(1).replace(/\.0$/, "")}%`;

  /** 月額（このサービスで今有効なプラン） */
  function monthlyFee(service: ServiceId): { amount: number; name: string } | null {
    const today = new Date().toISOString().slice(0, 10);
    const p = (app.settings?.plans ?? [])
      .filter((x) => x.service === service && x.start <= today && (!x.end || x.end > today))
      .at(-1);
    return p ? { amount: convert(p.price, p.currency, m), name: p.name } : null;
  }

  function durationText(ms: number): string {
    if (ms <= 0) return "まもなく";
    const min = Math.round(ms / 60000);
    const d = Math.floor(min / 1440);
    const h = Math.floor((min % 1440) / 60);
    const mm = min % 60;
    return d > 0 ? `${d}日${h}時間後` : h > 0 ? `${h}時間${mm}分後` : `${mm}分後`;
  }

  function chartOption(s: LimitSeries) {
    theme.dark;
    const c = chrome();
    const color = serviceColor(s.service);
    const b = base();
    const pct = s.points.map((p) => [p.ts, p.percent]);
    const cost = s.points.map((p) => [p.ts, p.window_cost == null ? null : toDisplay(p.window_cost, m)]);
    const axis = (grid: number, show: boolean) => ({
      type: "time" as const,
      gridIndex: grid,
      axisLine: { lineStyle: { color: c.axis } },
      axisTick: { show: false },
      axisLabel: { show, color: c.muted, fontSize: 11, hideOverlap: true, formatter: "{M}/{d}" },
      splitLine: { show: false },
    });
    const yAxis = (grid: number, fmt: (v: number) => string, max?: number) => ({
      type: "value" as const,
      gridIndex: grid,
      max,
      min: 0,
      axisLabel: { color: c.muted, fontSize: 11, formatter: fmt },
      splitLine: { lineStyle: { color: c.grid } },
    });
    return {
      ...b,
      axisPointer: { link: [{ xAxisIndex: "all" }] },
      grid: [
        { left: 8, right: 12, top: 22, height: 110, containLabel: true },
        { left: 8, right: 12, top: 168, height: 90, containLabel: true },
      ],
      title: [
        { text: "使用率", left: 4, top: 0, textStyle: { fontSize: 11, color: c.secondary, fontWeight: 600 } },
        { text: "枠内の API 換算額（累計）", left: 4, top: 146, textStyle: { fontSize: 11, color: c.secondary, fontWeight: 600 } },
      ],
      tooltip: {
        ...b.tooltip,
        trigger: "axis",
        formatter: (ps: any[]) => {
          const t = ps[0]?.axisValue;
          const rowsHtml = ps
            .filter((p) => p.value?.[1] != null)
            .map((p) =>
              p.seriesIndex === 0
                ? tipRow(color, "使用率", `${Number(p.value[1]).toFixed(1).replace(/\.0$/, "")}%`)
                : tipRow(color, "API換算（枠内）", moneyIn(p.value[1], m.currency)),
            )
            .join("");
          return `<div style="margin-bottom:4px;color:${c.secondary}">${t ? dateTime(t) : ""}</div>${rowsHtml}`;
        },
      },
      xAxis: [axis(0, false), axis(1, true)],
      yAxis: [yAxis(0, (v) => `${v}%`, 100), yAxis(1, (v) => moneyAxis(v, m.currency))],
      series: [
        {
          type: "line",
          step: "end",
          xAxisIndex: 0,
          yAxisIndex: 0,
          symbol: "circle",
          symbolSize: 4,
          showSymbol: s.points.length < 80,
          lineStyle: { width: 2, color },
          itemStyle: { color },
          areaStyle: { color, opacity: 0.12 },
          data: pct,
        },
        {
          type: "line",
          xAxisIndex: 1,
          yAxisIndex: 1,
          symbol: "none",
          connectNulls: false,
          lineStyle: { width: 2, color },
          itemStyle: { color },
          data: cost,
        },
      ],
    };
  }

  function readPaste() {
    const parsed = parseUsageText(paste);
    if (parsed.length === 0) {
      toast("使用率を読み取れませんでした。下の表に直接入力してください", "error");
      return;
    }
    rows = parsed.map((p) => ({
      service: "claude" as ServiceId,
      kind: p.kind,
      model: p.model ?? "",
      percent: p.percent,
      hours: p.resetsInMin != null ? Math.floor(p.resetsInMin / 60) : null,
      minutes: p.resetsInMin != null ? p.resetsInMin % 60 : null,
    }));
  }

  function addRow() {
    rows = [...rows, { service: "claude", kind: "weekly", model: "", percent: null, hours: null, minutes: null }];
  }

  async function saveRows() {
    if (!app.settings) return;
    const now = Date.now();
    const valid = rows.filter((r) => r.percent != null && r.percent >= 0 && r.percent <= 100);
    if (valid.length === 0) {
      toast("使用率（0〜100）を入力してください", "error");
      return;
    }
    if (valid.some((r) => r.kind === "weekly_model" && !r.model.trim())) {
      toast("モデル別の枠にはモデル名（例: fable）を入力してください", "error");
      return;
    }
    const added: ManualLimit[] = valid.map((r) => {
      const mins = (r.hours ?? 0) * 60 + (r.minutes ?? 0);
      return {
        id: crypto.randomUUID(),
        service: r.service,
        kind: r.kind,
        model: r.kind === "weekly_model" ? r.model.trim().toLowerCase() : null,
        percent: Number(r.percent),
        ts: now,
        resets_at: r.hours != null || r.minutes != null ? now + mins * 60000 : null,
      };
    });
    await saveSettings({ ...app.settings, manual_limits: [...app.settings.manual_limits, ...added] });
    rows = [];
    paste = "";
    toast(`${added.length} 件の使用率を記録しました`);
  }

  async function toggleDesktop(v: boolean) {
    if (!app.settings) return;
    await saveSettings({ ...app.settings, claude_desktop_usage: v });
  }

  const manual = $derived([...(app.settings?.manual_limits ?? [])].sort((a, b) => b.ts - a.ts));

  /** その枠に属する手動記録 */
  function manualOf(s: LimitSeries): ManualLimit[] {
    return manual.filter(
      (x) => x.service === s.service && x.kind === s.kind && (x.model ?? null) === (s.model ?? null),
    );
  }

  // 各カードの「今の値を記録」欄の入力値（系列ごと）
  type Quick = { percent: number | null; days: number | null; hours: number | null; minutes: number | null };
  let quick = $state<Record<string, Quick>>({});

  const seriesKey = (s: LimitSeries) => `${s.service}|${s.kind}|${s.model ?? ""}`;

  /** 手動記録で表せる枠だけ欄を出す（Codex の独自の枠などは除く）。 */
  function quickKind(s: LimitSeries): LimitKind | null {
    return s.kind === "session" || s.kind === "weekly" || s.kind === "weekly_model" ? s.kind : null;
  }

  // 描画中に状態を書き換えられないので、系列が増えたときに入力欄の初期値を用意しておく。
  $effect(() => {
    const keys = (report?.series ?? []).map(seriesKey);
    untrack(() => {
      for (const k of keys) if (!quick[k]) quick[k] = { percent: null, days: null, hours: null, minutes: null };
    });
  });

  async function saveQuick(s: LimitSeries) {
    const kind = quickKind(s);
    const q = quick[seriesKey(s)];
    if (!app.settings || !kind || !q || q.percent == null) return;
    if (!(q.percent >= 0 && q.percent <= 100)) {
      toast("使用率は 0〜100 で入力してください", "error");
      return;
    }
    const now = Date.now();
    const hasReset = q.days != null || q.hours != null || q.minutes != null;
    const mins = (q.days ?? 0) * 1440 + (q.hours ?? 0) * 60 + (q.minutes ?? 0);
    const rec: ManualLimit = {
      id: crypto.randomUUID(),
      service: s.service,
      kind,
      model: kind === "weekly_model" ? s.model : null,
      percent: Number(q.percent),
      ts: now,
      resets_at: hasReset ? now + mins * 60000 : null,
    };
    await saveSettings({ ...app.settings, manual_limits: [...app.settings.manual_limits, rec] });
    quick[seriesKey(s)] = { percent: null, days: null, hours: null, minutes: null };
    toast(`${kindLabel(s)} を ${pct(rec.percent)} で記録しました`);
  }

  // 進行中の枠を先に、古いまま更新されていない枠（解約済みプランなど）を後ろに並べる。
  const sortedSeries = $derived(
    [...(report?.series ?? [])].sort(
      (a, b) => Number(b.current_active) - Number(a.current_active) || (b.latest?.ts ?? 0) - (a.latest?.ts ?? 0),
    ),
  );
</script>

<div class="page">
  <h1>使用上限との比較</h1>
  <p class="muted lead">
    サブスクの使用上限（%）と、同じ期間に使った分の API 換算額を並べて、「1% が API 換算でいくら分か」「上限まで使うといくら分か」を推定します。
    使用率にはチャットや他の端末での利用も含まれますが、API 換算額はこの PC の Claude Code / Codex のログだけから計算します。
    そのため、ログに記録がない時間帯に増えた使用率は「Code 以外の利用」とみなして除いています。同じ時間帯に Code とチャットを併用した分は分けられないため、1% あたりの額はまだ少し低めに出ることがあります。
  </p>

  {#if report}
    {#each sortedSeries as s (s.service + s.kind + (s.model ?? ""))}
      {@const p = s.latest}
      {@const per = s.usd_per_percent}
      {@const cur = s.current_active ? s.windows[0] : null}
      {@const fee = monthlyFee(s.service)}
      {@const weekly = s.window_minutes === 10080}
      <section class="card series">
        <div class="card-head">
          <span class="dot" style:background={serviceColor(s.service)}></span>
          <h2>{SERVICE_LABEL[s.service]} · {kindLabel(s)}</h2>
          {#each s.sources as src}<span class="badge">{SOURCE_LABEL[src] ?? src}</span>{/each}
          {#if p}<span class="muted tiny">最終記録 {relative(p.ts)}（{dateTime(p.ts)}）</span>{/if}
        </div>

        <div class="stats">
          <div>
            <div class="muted tiny">現在の使用率</div>
            <div class="big">
              {#if p && s.current_active}{p.percent.toFixed(1).replace(/\.0$/, "")}%{:else if p}<span class="muted">リセット済み</span>{:else}—{/if}
            </div>
            {#if p && s.current_active}
              <div class="meter"><div class="fill" style:width="{Math.min(100, p.percent)}%" style:background={serviceColor(s.service)}></div></div>
            {/if}
            <div class="muted tiny">
              {#if s.current_active && s.current_end}リセット: {durationText(s.current_end - Date.now())}{:else if p && !s.current_active}最後の記録は {p.percent}%{/if}
            </div>
            {#if cur && cur.other_percent > 0}
              <div class="muted tiny">うち Code 以外の利用 約 {pct(cur.other_percent)}</div>
            {/if}
          </div>
          <div>
            <div class="muted tiny">この枠の API 換算額</div>
            <div class="big">{s.current_cost != null ? money(s.current_cost, m) : "—"}</div>
            <div class="muted tiny">{s.current_cost != null && s.current_start ? `${dateTime(s.current_start)} から` : "枠の開始時刻が不明です"}</div>
          </div>
          <div>
            <div class="muted tiny">1% あたり</div>
            <div class="big">{per != null ? money(per, m) : "—"}</div>
            <div class="muted tiny">{per != null ? `Code 以外を除き、${BASIS[s.basis]}から推定` : "データ不足"}</div>
            {#if per != null && s.raw_usd_per_percent != null && s.raw_usd_per_percent < per * 0.995}
              <div class="muted tiny">除かない場合 {money(s.raw_usd_per_percent, m)}</div>
            {/if}
          </div>
          <div>
            <div class="muted tiny">上限（100%）まで使うと</div>
            <div class="big">{per != null ? money(per * 100, m) : "—"}</div>
            <div class="muted tiny">
              {#if per != null && p && s.current_active}残り {Math.max(0, 100 - p.percent).toFixed(0)}% ≈ {money(per * Math.max(0, 100 - p.percent), m)}{/if}
            </div>
          </div>
          {#if weekly && per != null}
            <div>
              <div class="muted tiny">毎週上限まで使った場合（月あたり）</div>
              <div class="big">{money(per * 100 * (30.44 / 7), m)}</div>
              <div class="muted tiny">
                {#if fee}{fee.name} の月額 {moneyIn(fee.amount, m.currency)} の {ratio(toDisplay(per * 100 * (30.44 / 7), m) / fee.amount)}{:else}プランを登録すると月額と比較できます{/if}
              </div>
            </div>
          {/if}
        </div>

        {#if quickKind(s) && quick[seriesKey(s)]}
          {@const q = quick[seriesKey(s)]}
          <div class="quick">
            <span class="secondary">今の値を記録:</span>
            <input class="input" type="number" min="0" max="100" step="any" bind:value={q.percent} placeholder="使用率" style="width:84px" aria-label="使用率" />
            <span class="muted">%</span>
            <span class="secondary">リセットまで</span>
            <input class="input" type="number" min="0" step="1" bind:value={q.days} placeholder="0" style="width:56px" aria-label="日" /><span class="muted">日</span>
            <input class="input" type="number" min="0" step="1" bind:value={q.hours} placeholder="0" style="width:56px" aria-label="時間" /><span class="muted">時間</span>
            <input class="input" type="number" min="0" max="59" step="1" bind:value={q.minutes} placeholder="0" style="width:56px" aria-label="分" /><span class="muted">分</span>
            <button class="btn small primary" onclick={() => saveQuick(s)} disabled={q.percent == null}>記録</button>
            <span class="muted tiny">リセットまでの時間は空欄でも記録できます（入れると枠の開始時刻が正確になります）</span>
          </div>
          {@const own = manualOf(s)}
          {#if own.length > 0}
            <details class="own">
              <summary>この枠の手動記録（{own.length} 件）</summary>
              <ManualLimitList records={own} />
            </details>
          {/if}
        {/if}

        {#if s.points.length > 1}
          <Chart option={chartOption(s)} height={270} />
        {/if}

        {#if s.windows.length > 0}
          <details>
            <summary>枠ごとの実績（{s.windows.length} 件）</summary>
            <table class="data">
              <thead>
                <tr>
                  <th>期間</th>
                  <th class="r">最大使用率</th>
                  <th class="r" title="ログに {SERVICE_LABEL[s.service]} の記録がない時間帯に増えた使用率">うち Code 以外（推定）</th>
                  <th class="r">API換算（{SERVICE_LABEL[s.service]}）</th>
                  <th class="r" title="API 換算額を、Code 以外の利用を除いた使用率で割った額">1% あたり</th>
                  <th class="r">100% 換算</th>
                </tr>
              </thead>
              <tbody>
                {#each s.windows as w}
                  <tr>
                    <td>{w.start ? dateTime(w.start) : "開始不明"} 〜 {w.end ? dateTime(w.end) : "?"}</td>
                    <td class="r">{pct(w.max_percent)}</td>
                    <td class="r">{w.other_percent > 0 ? pct(w.other_percent) : "—"}</td>
                    <td class="r">{w.cost != null ? money(w.cost, m) : "—"}</td>
                    <td class="r">{w.usd_per_percent != null ? money(w.usd_per_percent, m) : "—"}</td>
                    <td class="r">{w.usd_per_percent != null ? money(w.usd_per_percent * 100, m) : "—"}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
            <p class="muted tiny">
              過去30日の平均: {s.avg_usd_per_percent != null ? `1% ≈ ${money(s.avg_usd_per_percent, m)}` : "—"}
              （Code を使っていた {s.basis_intervals} 区間・合計 {s.basis_points.toFixed(0)} ポイントから算出。
              Code 以外の利用 {s.other_points.toFixed(0)} ポイントを除外{s.raw_avg_usd_per_percent != null ? `、除かない場合は 1% ≈ ${money(s.raw_avg_usd_per_percent, m)}` : ""}）。
              「Code 以外」は、ログに {SERVICE_LABEL[s.service]} の記録がない時間帯（反映の遅れとして直前の約 10 分を含む）に増えた使用率です。
            </p>
          </details>
        {/if}
      </section>
    {:else}
      <div class="card"><p class="muted">使用率のデータがまだありません。下のフォームから記録してください。</p></div>
    {/each}
  {/if}

  <section class="card">
    <div class="card-head"><h2>使用率を記録する</h2></div>
    <p class="muted tiny">
      Claude デスクトップアプリの「プランの使用量上限」や claude.ai の使用量ページ、Claude Code の <code>/usage</code> の内容をそのまま貼り付けると読み取ります。モデル別の枠（週次・Fable など）はここで記録してください。
    </p>
    <textarea class="input paste" bind:value={paste} placeholder={"例:\nセッションの上限 4時間20分後にリセット 5%\n週間・すべてのモデル 21時間50分後にリセット 42%\n週次・Fable 21時間50分後にリセット 20%"}></textarea>
    <div class="row">
      <button class="btn" onclick={readPaste} disabled={!paste.trim()}>読み取る</button>
      <button class="btn ghost" onclick={addRow}>＋ 手入力の行を追加</button>
    </div>
    {#if rows.length > 0}
      <table class="data rows">
        <thead><tr><th>サービス</th><th>枠</th><th>モデル</th><th>使用率</th><th>リセットまで</th><th></th></tr></thead>
        <tbody>
          {#each rows as r, i}
            <tr>
              <td>
                <select class="input" bind:value={r.service}>
                  <option value="claude">Claude</option>
                  <option value="codex">Codex</option>
                </select>
              </td>
              <td>
                <select class="input" bind:value={r.kind}>
                  <option value="session">5時間枠</option>
                  <option value="weekly">週間（すべてのモデル）</option>
                  <option value="weekly_model">週間（モデル別）</option>
                </select>
              </td>
              <td><input class="input" bind:value={r.model} placeholder="fable" disabled={r.kind !== "weekly_model"} style="width:110px" /></td>
              <td><input class="input" type="number" min="0" max="100" step="any" bind:value={r.percent} style="width:80px" /> %</td>
              <td class="nowrap">
                <input class="input" type="number" min="0" step="1" bind:value={r.hours} style="width:70px" /> 時間
                <input class="input" type="number" min="0" max="59" step="1" bind:value={r.minutes} style="width:64px" /> 分後
              </td>
              <td><button class="btn small ghost danger" onclick={() => (rows = rows.filter((_, j) => j !== i))}>削除</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="row"><button class="btn primary" onclick={saveRows}>記録する（現在時刻で保存）</button></div>
    {/if}

    {#if manual.length > 0}
      <details class="manual">
        <summary>手動で記録した使用率（すべて・{manual.length} 件）</summary>
        <ManualLimitList records={manual} showKind />
      </details>
    {/if}
  </section>

  {#if report}
    <section class="card">
      <div class="card-head"><h2>データの取得元</h2></div>
      <table class="data">
        <tbody>
          <tr>
            <td>
              <label class="row nowrap">
                <input type="checkbox" checked={app.settings?.claude_desktop_usage} onchange={(e) => toggleDesktop(e.currentTarget.checked)} />
                Claude デスクトップアプリの使用率の履歴
              </label>
            </td>
            <td class="muted tiny path">{report.desktop_path ?? "—"}{report.desktop_exists ? "" : "（見つかりません）"}</td>
            <td class="r">{report.desktop_samples.toLocaleString()} 件</td>
            <td class="muted tiny">{report.desktop_last_ts ? `最新 ${dateTime(report.desktop_last_ts)}` : ""}</td>
          </tr>
          <tr>
            <td>Codex のログ（rate_limits）</td>
            <td class="muted tiny">セッションログから自動で取得</td>
            <td class="r">{report.codex_samples.toLocaleString()} 件</td>
            <td></td>
          </tr>
          <tr>
            <td>手動記録</td>
            <td class="muted tiny">設定に保存</td>
            <td class="r">{report.manual_samples.toLocaleString()} 件</td>
            <td></td>
          </tr>
        </tbody>
      </table>
      <p class="muted tiny">
        デスクトップアプリはアプリを開いている間しか使用率を記録しないため、間隔が空くことがあります。読み込んだ履歴はこのアプリ側にも保存するので、元のファイルから消えても残ります。
      </p>
    </section>
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
  .own {
    margin: -2px 0 10px;
  }
  .quick {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 4px 0 10px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 8px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: 16px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 8px;
  }
  .big {
    font-size: 22px;
    font-weight: 650;
    line-height: 1.3;
  }
  .meter {
    height: 6px;
    border-radius: 3px;
    background: var(--surface-2);
    overflow: hidden;
    margin: 4px 0;
  }
  .meter .fill {
    height: 100%;
    border-radius: 3px;
  }
  details summary {
    cursor: pointer;
    color: var(--text-2);
    font-weight: 600;
    margin: 8px 0;
  }
  details p {
    margin: 8px 0 0;
  }
  .paste {
    width: 100%;
    min-height: 96px;
    height: auto;
    padding: 8px;
    resize: vertical;
    margin: 8px 0;
    font-family: inherit;
  }
  .rows {
    margin: 10px 0;
  }
  .nowrap {
    white-space: nowrap;
    flex-wrap: nowrap;
  }
  .manual {
    margin-top: 14px;
  }
  .path {
    word-break: break-all;
  }
  code {
    font-size: 12px;
    background: var(--surface-2);
    padding: 1px 4px;
    border-radius: 4px;
  }
</style>
