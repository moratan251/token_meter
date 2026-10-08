import * as echarts from "echarts/core";
import { BarChart, HeatmapChart, LineChart } from "echarts/charts";
import {
  DataZoomComponent,
  GridComponent,
  LegendComponent,
  MarkLineComponent,
  TooltipComponent,
  VisualMapComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import { chrome } from "./colors.svelte";

echarts.use([
  BarChart,
  LineChart,
  HeatmapChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  VisualMapComponent,
  MarkLineComponent,
  DataZoomComponent,
  CanvasRenderer,
]);

export { echarts };

const FONT = 'system-ui, "Segoe UI", "Yu Gothic UI", Meiryo, sans-serif';

/** 全グラフで共通の軸・ツールチップのスタイル。 */
export function base() {
  const c = chrome();
  return {
    animationDuration: 300,
    textStyle: { fontFamily: FONT, color: c.secondary },
    tooltip: {
      backgroundColor: c.tooltipBg,
      borderColor: c.tooltipBorder,
      borderWidth: 1,
      padding: [8, 10],
      textStyle: { color: c.text, fontSize: 12, fontFamily: FONT },
      extraCssText: "box-shadow: 0 4px 16px rgba(0,0,0,0.12); border-radius: 8px;",
    },
    legend: {
      textStyle: { color: c.secondary, fontSize: 12 },
      itemWidth: 10,
      itemHeight: 10,
      icon: "roundRect",
      type: "scroll" as const,
      pageIconColor: c.secondary,
      pageTextStyle: { color: c.muted },
    },
  };
}

export function categoryAxis(data: string[], formatter?: (v: string) => string) {
  const c = chrome();
  return {
    type: "category" as const,
    data,
    axisLine: { lineStyle: { color: c.axis } },
    axisTick: { show: false },
    axisLabel: { color: c.muted, fontSize: 11, hideOverlap: true, formatter },
  };
}

export function valueAxis(formatter: (v: number) => string) {
  const c = chrome();
  return {
    type: "value" as const,
    axisLine: { show: false },
    axisTick: { show: false },
    splitLine: { lineStyle: { color: c.grid, width: 1 } },
    axisLabel: { color: c.muted, fontSize: 11, formatter },
  };
}

/** 色見本付きのツールチップ行。文字は系列色ではなく通常の文字色にする。 */
export function tipRow(color: string, label: string, value: string, bold = false): string {
  const w = bold ? "font-weight:600;" : "";
  return `<div style="display:flex;align-items:center;gap:8px;min-width:200px;${w}">
    <span style="width:8px;height:8px;border-radius:2px;background:${color};flex:none"></span>
    <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:260px">${escapeHtml(label)}</span>
    <span style="font-variant-numeric:tabular-nums">${value}</span></div>`;
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]!);
}
