<script lang="ts">
  import { onMount } from "svelte";
  import { echarts } from "../lib/echarts";

  let {
    option,
    height = 280,
    onclick,
  }: {
    option: Record<string, unknown>;
    height?: number;
    onclick?: (params: any) => void;
  } = $props();

  let el: HTMLDivElement;
  let chart: ReturnType<typeof echarts.init> | null = null;

  onMount(() => {
    chart = echarts.init(el, undefined, { renderer: "canvas" });
    chart.setOption(option, { notMerge: true });
    if (onclick) chart.on("click", onclick);
    const ro = new ResizeObserver(() => chart?.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      chart?.dispose();
      chart = null;
    };
  });

  $effect(() => {
    // option が変わったら（テーマ・データ・設定）描画し直す。
    const o = option;
    chart?.setOption(o, { notMerge: true });
  });
</script>

<div bind:this={el} style:height="{height}px" class="chart"></div>

<style>
  .chart {
    width: 100%;
    min-width: 0;
  }
</style>
