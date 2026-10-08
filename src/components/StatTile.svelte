<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    label,
    value,
    sub,
    delta,
    deltaGoodWhenUp = false,
    children,
  }: {
    label: string;
    value: string;
    sub?: string;
    /** 前の期間からの増減率（0.12 = +12%）。 */
    delta?: number | null;
    deltaGoodWhenUp?: boolean;
    children?: Snippet;
  } = $props();

  const deltaText = $derived(
    delta === undefined || delta === null || !isFinite(delta)
      ? null
      : `${delta >= 0 ? "▲" : "▼"} ${Math.abs(delta * 100).toFixed(1)}%`,
  );
</script>

<div class="card tile">
  <div class="label">{label}</div>
  <div class="value">{value}</div>
  {#if sub || deltaText}
    <div class="sub">
      {#if deltaText}
        <span class="badge" class:good={deltaGoodWhenUp ? (delta ?? 0) >= 0 : false}>{deltaText}</span>
      {/if}
      {#if sub}<span class="muted">{sub}</span>{/if}
    </div>
  {/if}
  {@render children?.()}
</div>

<style>
  .tile {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 16px;
  }
  .label {
    font-size: 12px;
    color: var(--text-2);
    font-weight: 600;
  }
  .value {
    font-size: 24px;
    font-weight: 650;
    letter-spacing: -0.01em;
    line-height: 1.2;
  }
  .sub {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12px;
    flex-wrap: wrap;
  }
</style>
