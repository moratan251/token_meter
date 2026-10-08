<script lang="ts">
  import MultiSelect, { type Option } from "./MultiSelect.svelte";
  import { app, filter, persistUi, saveSettings, setPeriod } from "../lib/state.svelte";
  import { PERIODS } from "../lib/periods";
  import { SERVICE_LABEL, compact, projectLabel } from "../lib/format";
  import { assignColors, serviceColor } from "../lib/colors.svelte";
  import type { ServiceId } from "../lib/types";

  let { showSessions = true }: { showSessions?: boolean } = $props();

  const services: ServiceId[] = ["claude", "codex"];

  const modelOptions = $derived.by((): Option[] => {
    const models = (app.meta?.models ?? []).filter(
      (m) => filter.services.length === 0 || m.services.some((s) => filter.services.includes(s)),
    );
    const order = (app.meta?.models ?? []).map((m) => m.name);
    const colors = assignColors(
      models.map((m) => m.name),
      order,
    );
    return models.map((m, i) => ({
      value: m.name,
      label: m.name,
      hint: compact(m.tokens),
      color: colors[i],
    }));
  });

  const projectOptions = $derived(
    (app.meta?.projects ?? [])
      .filter((p) => filter.services.length === 0 || p.services.some((s) => filter.services.includes(s)))
      .map((p) => ({ value: p.path, label: projectLabel(p.path), hint: p.path.length > 40 ? "…" + p.path.slice(-38) : p.path })),
  );

  function toggleService(s: ServiceId) {
    filter.services = filter.services.includes(s) ? filter.services.filter((x) => x !== s) : [...filter.services, s];
    persistUi();
  }

  function onPeriod(id: (typeof PERIODS)[number]["id"]) {
    setPeriod(id);
    persistUi();
  }

  function onDate() {
    filter.period = "custom";
    if (filter.from && filter.to && filter.from > filter.to) [filter.from, filter.to] = [filter.to, filter.from];
    persistUi();
  }

  async function setCurrency(c: "USD" | "JPY") {
    if (!app.settings || app.settings.currency === c) return;
    await saveSettings({ ...app.settings, currency: c });
  }

  const active = $derived(
    filter.services.length + filter.models.length + filter.projects.length + filter.sessions.length > 0 ||
      filter.excludeSubagents,
  );

  function reset() {
    filter.services = [];
    filter.models = [];
    filter.projects = [];
    filter.sessions = [];
    filter.excludeSubagents = false;
    persistUi();
  }
</script>

<div class="bar">
  <div class="row line">
    <div class="seg" role="group" aria-label="期間">
      {#each PERIODS as p}
        <button class:on={filter.period === p.id} onclick={() => onPeriod(p.id)}>{p.label}</button>
      {/each}
    </div>
    <div class="dates row">
      <input class="input" type="date" bind:value={filter.from} onchange={onDate} aria-label="開始日" />
      <span class="muted">〜</span>
      <input class="input" type="date" bind:value={filter.to} onchange={onDate} aria-label="終了日" />
    </div>
    <div class="spacer"></div>
    <div class="seg" role="group" aria-label="通貨">
      <button class:on={app.settings?.currency === "JPY"} onclick={() => setCurrency("JPY")}>¥ 円</button>
      <button class:on={app.settings?.currency === "USD"} onclick={() => setCurrency("USD")}>$ USD</button>
    </div>
  </div>
  <div class="row line">
    {#each services as s}
      <button class="chip" class:on={filter.services.includes(s)} onclick={() => toggleService(s)}>
        <span class="dot" style:background={serviceColor(s)}></span>{SERVICE_LABEL[s]}
      </button>
    {/each}
    <MultiSelect label="モデル" options={modelOptions} bind:selected={filter.models} onchange={persistUi} />
    <MultiSelect label="プロジェクト" options={projectOptions} bind:selected={filter.projects} onchange={persistUi} width={420} />
    <label class="toggle">
      <input type="checkbox" bind:checked={filter.excludeSubagents} onchange={persistUi} />
      サブエージェントを除く
    </label>
    {#if showSessions}
      {#each filter.sessions as s (s.id)}
        <span class="chip on" title={s.id}>
          セッション: <span class="ellipsis" style="max-width:220px">{s.label}</span>
          <button class="x" aria-label="セッションの絞り込みを解除" onclick={() => (filter.sessions = filter.sessions.filter((x) => x.id !== s.id))}>×</button>
        </span>
      {/each}
    {/if}
    {#if active}
      <button class="btn small ghost" onclick={reset}>条件をクリア</button>
    {/if}
  </div>
</div>

<style>
  .bar {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 24px;
    border-bottom: 1px solid var(--border);
    background: var(--page);
    position: sticky;
    top: 0;
    z-index: 20;
  }
  .line {
    gap: 8px;
  }
  .dates .input {
    width: 140px;
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    cursor: pointer;
    margin-left: 4px;
  }
  .x {
    border: 0;
    background: transparent;
    cursor: pointer;
    color: inherit;
    padding: 0 0 0 2px;
    font-size: 14px;
    line-height: 1;
  }
</style>
