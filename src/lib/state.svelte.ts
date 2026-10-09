import { api } from "./api";
import type { Money } from "./format";
import { presetRange, toMillis, type PeriodId } from "./periods";
import type { BackendFilter, Meta, ServiceId, Settings } from "./types";

export type Page = "dashboard" | "explorer" | "subscription" | "limits" | "pricing" | "settings";

export interface FilterState {
  period: PeriodId;
  from: string | null;
  to: string | null;
  services: ServiceId[];
  models: string[];
  projects: string[];
  sessions: { id: string; label: string }[];
  excludeSubagents: boolean;
}

const UI_KEY = "token-meter.ui.v1";

function loadUi(): Partial<FilterState> & { page?: Page } {
  try {
    return JSON.parse(localStorage.getItem(UI_KEY) ?? "{}");
  } catch {
    return {};
  }
}

const saved = loadUi();
const initialPeriod: PeriodId = saved.period ?? "30d";
const initialRange =
  initialPeriod === "custom" ? { from: saved.from ?? null, to: saved.to ?? null } : presetRange(initialPeriod);

export const app = $state({
  page: (saved.page ?? "dashboard") as Page,
  settings: null as Settings | null,
  meta: null as Meta | null,
  /** データが変わるスキャンのたびに増やす。各画面はこれを見て再取得する。 */
  dataVersion: 0,
  scanning: false,
  scanProgress: null as { done: number; total: number } | null,
  error: null as string | null,
  toast: null as { text: string; kind: "info" | "error" } | null,
});

export const filter = $state<FilterState>({
  period: initialPeriod,
  from: initialRange.from,
  to: initialRange.to,
  services: saved.services ?? [],
  models: saved.models ?? [],
  projects: saved.projects ?? [],
  sessions: [],
  excludeSubagents: saved.excludeSubagents ?? false,
});

export function persistUi() {
  try {
    localStorage.setItem(
      UI_KEY,
      JSON.stringify({
        page: app.page,
        period: filter.period,
        from: filter.from,
        to: filter.to,
        services: filter.services,
        models: filter.models,
        projects: filter.projects,
        excludeSubagents: filter.excludeSubagents,
      }),
    );
  } catch {
    // ストレージが使えない場合は画面の状態を保存しないだけ
  }
}

export function setPeriod(id: PeriodId) {
  filter.period = id;
  if (id !== "custom") {
    const r = presetRange(id);
    filter.from = r.from;
    filter.to = r.to;
  }
}

/** 相対的な期間（日付が変わった後の「7日」など）を計算し直す。 */
export function refreshPeriod() {
  if (filter.period !== "custom") setPeriod(filter.period);
}

export function backendFilter(f: FilterState = filter): BackendFilter {
  return {
    ...toMillis(f.from, f.to),
    services: [...f.services],
    models: [...f.models],
    projects: [...f.projects],
    sessions: f.sessions.map((s) => s.id),
    exclude_subagents: f.excludeSubagents,
  };
}

export function money(): Money {
  return {
    currency: app.settings?.currency ?? "JPY",
    rate: app.settings?.usd_jpy ?? 150,
  };
}

export function toast(text: string, kind: "info" | "error" = "info") {
  app.toast = { text, kind };
  const current = app.toast;
  setTimeout(() => {
    if (app.toast === current) app.toast = null;
  }, kind === "error" ? 8000 : 3500);
}

export async function saveSettings(next: Settings) {
  app.settings = await api.saveSettings($state.snapshot(next) as Settings);
}

export async function loadMeta() {
  app.meta = await api.meta();
}

export async function scan(force = false) {
  if (app.scanning) return;
  app.scanning = true;
  try {
    const stats = await api.refresh(force);
    if (stats.changed || !app.meta) {
      await loadMeta();
      app.dataVersion++;
    } else if (app.meta) {
      app.meta.last_scan = stats;
    }
    if (stats.errors.length) toast(`${stats.errors.length} 件のファイルを読めませんでした`, "error");
  } catch (e) {
    toast(String(e), "error");
  } finally {
    app.scanning = false;
    app.scanProgress = null;
  }
}
