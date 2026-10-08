<script lang="ts">
  import { app, persistUi, scan, type Page } from "../lib/state.svelte";
  import { relative } from "../lib/format";

  const items: { id: Page; label: string; icon: string }[] = [
    { id: "dashboard", label: "ダッシュボード", icon: "M3 13h4v8H3zM10 3h4v18h-4zM17 8h4v13h-4z" },
    { id: "explorer", label: "明細", icon: "M4 5h16M4 10h16M4 15h16M4 20h10" },
    { id: "subscription", label: "元取り分析", icon: "M4 19 9 13l4 3 7-9M15 7h5v5" },
    { id: "pricing", label: "価格表", icon: "M20 12 12 20 4 12V4h8zM8.5 8.5h.01" },
    { id: "settings", label: "設定", icon: "M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7ZM19.4 13a7.6 7.6 0 0 0 0-2l2-1.6-2-3.4-2.4 1a7 7 0 0 0-1.7-1L15 3.5h-4l-.4 2.5a7 7 0 0 0-1.7 1l-2.4-1-2 3.4L6.6 11a7.6 7.6 0 0 0 0 2l-2 1.6 2 3.4 2.4-1a7 7 0 0 0 1.7 1l.4 2.5h4l.4-2.5a7 7 0 0 0 1.7-1l2.4 1 2-3.4Z" },
  ];

  function go(p: Page) {
    app.page = p;
    persistUi();
  }

  let now = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(t);
  });

  const lastScan = $derived.by(() => {
    now;
    const t = app.meta?.last_scan.scanned_at;
    return t ? relative(t) : "—";
  });
</script>

<nav class="side">
  <div class="brand">
    <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M4 16a8 8 0 0 1 16 0" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" />
      <path d="M12 16 16 10" stroke="var(--text)" stroke-width="2" stroke-linecap="round" />
      <circle cx="12" cy="16" r="1.8" fill="var(--text)" />
    </svg>
    <span>Token Meter</span>
  </div>
  {#each items as it}
    <button class="item" class:on={app.page === it.id} onclick={() => go(it.id)}>
      <svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><path d={it.icon} fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" /></svg>
      {it.label}
    </button>
  {/each}
  <div class="spacer"></div>
  <div class="foot">
    {#if app.scanning}
      <div class="muted small">
        読み込み中{app.scanProgress && app.scanProgress.total > 0 ? ` ${app.scanProgress.done}/${app.scanProgress.total}` : "…"}
      </div>
    {:else}
      <div class="muted small">更新: {lastScan}</div>
      {#if app.meta}
        <div class="muted small num">{app.meta.records.toLocaleString()} リクエスト</div>
      {/if}
    {/if}
    <button class="btn small" onclick={() => scan()} disabled={app.scanning}>
      <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true" class:spin={app.scanning}><path d="M20 12a8 8 0 1 1-2.3-5.6M20 4v5h-5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" /></svg>
      再読み込み
    </button>
  </div>
</nav>

<style>
  .side {
    width: 196px;
    flex: none;
    background: var(--sidebar);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    padding: 14px 10px;
    gap: 2px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 700;
    font-size: 15px;
    padding: 4px 8px 16px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 0;
    background: transparent;
    padding: 8px 10px;
    border-radius: 8px;
    cursor: pointer;
    color: var(--text-2);
    text-align: left;
  }
  .item:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .item.on {
    background: var(--surface);
    color: var(--text);
    font-weight: 600;
    box-shadow: var(--shadow);
  }
  .foot {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
  }
  .small {
    font-size: 11.5px;
  }
  .spin {
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
