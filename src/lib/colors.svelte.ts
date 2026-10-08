// グラフの配色: 色覚多様性を検証済みの分類色（両テーマで隣接色の CVD ΔE ≥ 8）、
// 量を表す単色の連続スケール、テーマに応じた軸・文字色。

import type { ServiceId } from "./types";

const CAT_LIGHT = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#4a3aa7", "#e34948"];
const CAT_DARK = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"];

export const SEQ_BLUE = ["#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b"];

export const theme = $state({ dark: false });

export function categorical(): string[] {
  return theme.dark ? CAT_DARK : CAT_LIGHT;
}

export function otherColor(): string {
  return theme.dark ? "#6b6a65" : "#b4b2a9";
}

/** サービスの固定色: Codex = 1 番目、Claude Code = 2 番目。 */
export function serviceColor(s: ServiceId | string): string {
  const c = categorical();
  return s === "codex" ? c[0] : s === "claude" ? c[1] : otherColor();
}

/**
 * 色はグラフ内の順位ではなく対象そのものに付ける。各キーは全期間での順位
 * （`globalOrder`）の色を使い、上位 8 件に入らないキーはこのグラフで空いている色を
 * 順に使う。"__other__" は灰色。
 */
export function assignColors(keys: string[], globalOrder: string[]): string[] {
  const c = categorical();
  const rank = new Map(globalOrder.slice(0, c.length).map((k, i) => [k, i]));
  const used = new Set<number>();
  for (const k of keys) {
    const r = rank.get(k);
    if (r !== undefined) used.add(r);
  }
  const free = c.map((_, i) => i).filter((i) => !used.has(i));
  return keys.map((k) => {
    if (k === "__other__") return otherColor();
    const r = rank.get(k);
    if (r !== undefined) return c[r];
    const slot = free.shift();
    return slot !== undefined ? c[slot] : otherColor();
  });
}

export function chrome() {
  return theme.dark
    ? {
        surface: "#1a1a19",
        text: "#ffffff",
        secondary: "#c3c2b7",
        muted: "#898781",
        grid: "#2c2c2a",
        axis: "#383835",
        tooltipBg: "#242423",
        tooltipBorder: "rgba(255,255,255,0.12)",
      }
    : {
        surface: "#fcfcfb",
        text: "#0b0b0b",
        secondary: "#52514e",
        muted: "#898781",
        grid: "#e1e0d9",
        axis: "#c3c2b7",
        tooltipBg: "#ffffff",
        tooltipBorder: "rgba(11,11,11,0.10)",
      };
}
