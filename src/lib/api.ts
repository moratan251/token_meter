import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  BackendFilter,
  CatalogEntry,
  Dashboard,
  LimitReport,
  Meta,
  PlanReport,
  Row,
  ScanStats,
  Settings,
} from "./types";

export const api = {
  refresh: (force = false) => invoke<ScanStats>("refresh", { force }),
  rebuildCache: () => invoke<ScanStats>("rebuild_cache"),
  meta: () => invoke<Meta>("get_meta"),
  dashboard: (filter: BackendFilter, opts: { bucket: string; split: string; top: number }) =>
    invoke<Dashboard>("query_dashboard", { filter, opts }),
  table: (filter: BackendFilter, groupBy: string) => invoke<Row[]>("query_table", { filter, groupBy }),
  subscriptions: () => invoke<PlanReport[]>("subscription_report"),
  limits: () => invoke<LimitReport>("limit_report"),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  importLitellm: (text: string) => invoke<{ models: number; fetched_at: string }>("import_litellm", { text }),
  catalog: () => invoke<CatalogEntry[]>("price_catalog"),
  writeTextFile: (path: string, contents: string) => invoke<void>("write_text_file", { path, contents }),
};

export function onScanProgress(cb: (p: { done: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ done: number; total: number }>("scan-progress", (e) => cb(e.payload));
}

export const LITELLM_URL =
  "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

export async function fetchLitellm(): Promise<{ models: number; fetched_at: string }> {
  const res = await fetch(LITELLM_URL, { cache: "no-store" });
  if (!res.ok) throw new Error(`価格表の取得に失敗しました (HTTP ${res.status})`);
  return api.importLitellm(await res.text());
}

/** 2 つの無料 API から USD→JPY を取得する。先に応答した方を使う。 */
export async function fetchUsdJpy(): Promise<number> {
  const sources: [string, (j: any) => number | undefined][] = [
    ["https://api.frankfurter.dev/v1/latest?base=USD&symbols=JPY", (j) => j?.rates?.JPY],
    ["https://open.er-api.com/v6/latest/USD", (j) => j?.rates?.JPY],
  ];
  let lastErr: unknown;
  for (const [url, pick] of sources) {
    try {
      const res = await fetch(url, { cache: "no-store" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const v = pick(await res.json());
      if (typeof v === "number" && v > 0) return v;
      throw new Error("unexpected response");
    } catch (e) {
      lastErr = e;
    }
  }
  throw new Error(`為替レートを取得できませんでした: ${String(lastErr)}`);
}
