<script lang="ts" module>
  export interface Option {
    value: string;
    label: string;
    hint?: string;
    color?: string;
  }
</script>

<script lang="ts">
  let {
    label,
    options,
    selected = $bindable([]),
    onchange,
    width = 320,
  }: {
    label: string;
    options: Option[];
    selected: string[];
    onchange?: (v: string[]) => void;
    width?: number;
  } = $props();

  let open = $state(false);
  let query = $state("");
  let root: HTMLDivElement;
  let search: HTMLInputElement | undefined = $state();

  const shown = $derived(
    query.trim()
      ? options.filter((o) =>
          (o.label + " " + o.value + " " + (o.hint ?? "")).toLowerCase().includes(query.trim().toLowerCase()),
        )
      : options,
  );

  const summary = $derived.by(() => {
    if (selected.length === 0) return "すべて";
    if (selected.length === 1) return options.find((o) => o.value === selected[0])?.label ?? selected[0];
    return `${selected.length}件`;
  });

  function toggle(v: string) {
    const next = selected.includes(v) ? selected.filter((x) => x !== v) : [...selected, v];
    selected = next;
    onchange?.(next);
  }

  function clear() {
    selected = [];
    onchange?.([]);
  }

  function onWindowClick(e: MouseEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }

  function openMenu() {
    open = !open;
    if (open) {
      query = "";
      queueMicrotask(() => search?.focus());
    }
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={(e) => e.key === "Escape" && (open = false)} />

<div class="ms" bind:this={root}>
  <button class="btn" class:active={selected.length > 0} onclick={openMenu} aria-haspopup="listbox" aria-expanded={open}>
    <span class="secondary">{label}:</span>
    <span class="val ellipsis">{summary}</span>
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
  </button>
  {#if open}
    <div class="menu card" style:width="{width}px" role="listbox" aria-multiselectable="true">
      <div class="row top">
        <input class="input" style="flex:1" placeholder="検索" bind:value={query} bind:this={search} />
        <button class="btn small ghost" onclick={clear} disabled={selected.length === 0}>解除</button>
      </div>
      <div class="list">
        {#each shown as o (o.value)}
          <label class="opt">
            <input type="checkbox" checked={selected.includes(o.value)} onchange={() => toggle(o.value)} />
            {#if o.color}<span class="dot" style:background={o.color}></span>{/if}
            <span class="ellipsis lbl" title={o.value}>{o.label}</span>
            {#if o.hint}<span class="muted hint num">{o.hint}</span>{/if}
          </label>
        {:else}
          <div class="muted empty">該当なし</div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .ms {
    position: relative;
  }
  .btn.active {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .val {
    max-width: 160px;
  }
  .menu {
    position: absolute;
    top: 34px;
    left: 0;
    z-index: 30;
    padding: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.18);
  }
  .top {
    margin-bottom: 6px;
    flex-wrap: nowrap;
  }
  .list {
    max-height: 320px;
    overflow: auto;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 6px;
    border-radius: 6px;
    cursor: pointer;
  }
  .opt:hover {
    background: var(--surface-hover);
  }
  .lbl {
    flex: 1;
    min-width: 0;
  }
  .hint {
    font-size: 11px;
  }
  .empty {
    padding: 8px;
  }
</style>
