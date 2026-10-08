// Tauri コマンドが返す Rust の構造体に対応する型（snake_case）。

export type ServiceId = "claude" | "codex";

export interface Tokens {
  input: number;
  output: number;
  cache_read: number;
  cache_write_5m: number;
  cache_write_1h: number;
  reasoning: number;
  web_search: number;
}

export interface Cost {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  web: number;
  cache_savings: number;
}

export interface Agg {
  cost: Cost;
  total_cost: number;
  tokens: Tokens;
  requests: number;
}

export interface Group {
  key: string;
  service: ServiceId | null;
  agg: Agg;
  sessions: number;
}

export interface Series {
  key: string;
  cost: number[];
  tokens: number[];
  total: number;
}

export interface PlanFee {
  plan_id: string;
  service: ServiceId;
  name: string;
  currency: string;
  amount: number;
}

export interface Dashboard {
  totals: Agg;
  subagent_cost: number;
  sessions: number;
  active_days: number;
  span_days: number;
  by_service: Group[];
  by_model: Group[];
  by_project: Group[];
  buckets: string[];
  series: Series[];
  heatmap: number[][];
  unpriced: { model: string; requests: number; tokens: number }[];
  first_ts: number | null;
  last_ts: number | null;
  range_from: number;
  range_to: number;
  fees: PlanFee[];
}

export interface Row {
  key: string;
  title: string | null;
  service: ServiceId | null;
  project: string | null;
  models: string[];
  first_ts: number;
  last_ts: number;
  agg: Agg;
  sessions: number;
  subagent_cost: number;
}

export interface Tier {
  above: number;
  input?: number | null;
  output?: number | null;
  cache_read?: number | null;
  cache_write_5m?: number | null;
  cache_write_1h?: number | null;
}

export interface PriceSpec {
  input: number;
  output: number;
  cache_read?: number | null;
  cache_write_5m?: number | null;
  cache_write_1h?: number | null;
  tiers?: Tier[];
  fast_multiplier?: number | null;
}

export type PriceSource = "user" | "curated" | "litellm" | "litellm_snapshot";

export interface Resolved {
  key: string;
  source: PriceSource;
  spec: PriceSpec;
  alias: string | null;
}

export interface ModelInfo {
  name: string;
  services: ServiceId[];
  requests: number;
  tokens: number;
  cost: number;
  last_ts: number;
  price: Resolved | null;
}

export interface ProjectInfo {
  path: string;
  services: ServiceId[];
  requests: number;
  cost: number;
  last_ts: number;
}

export interface SourceView {
  service: ServiceId;
  path: string;
  auto: boolean;
  exists: boolean;
  enabled: boolean;
  files: number;
  missing_files: number;
}

export interface RateWindow {
  used_percent: number;
  window_minutes: number | null;
  resets_at: number | null;
}

export interface RateLimitSnapshot {
  ts: number;
  plan_type: string | null;
  primary: RateWindow | null;
  secondary: RateWindow | null;
}

export interface ScanStats {
  files_total: number;
  files_parsed: number;
  files_missing: number;
  errors: string[];
  records: number;
  duration_ms: number;
  changed: boolean;
  scanned_at: number;
}

export interface Meta {
  sources: SourceView[];
  models: ModelInfo[];
  projects: ProjectInfo[];
  first_ts: number | null;
  last_ts: number | null;
  service_first_ts: Partial<Record<ServiceId, number>>;
  records: number;
  files: number;
  missing_files: number;
  plan_marks: [number, string][];
  rate_limit: RateLimitSnapshot | null;
  litellm_fetched_at: string | null;
  snapshot_date: string;
  last_scan: ScanStats;
  data_dir: string;
}

export interface Plan {
  id: string;
  service: ServiceId;
  name: string;
  price: number;
  currency: string;
  start: string;
  end: string | null;
}

export interface Settings {
  extra_dirs: { service: ServiceId; path: string }[];
  disabled_dirs: string[];
  keep_deleted: boolean;
  currency: "USD" | "JPY";
  usd_jpy: number;
  usd_jpy_updated: string | null;
  week_start: number;
  auto_refresh_secs: number;
  plans: Plan[];
  price_overrides: Record<string, PriceSpec>;
  aliases: Record<string, string>;
  price_priority: "curated" | "litellm";
  auto_update_prices: boolean;
  geo_us_multiplier: number;
  web_search_usd: number;
  theme: "system" | "light" | "dark";
}

export interface Cycle {
  start: number;
  end: number;
  fee: number;
  cost: number;
  tokens: number;
  requests: number;
  current: boolean;
  elapsed: number;
}

export interface PlanReport {
  plan: Plan;
  cycles: Cycle[];
  total_fee: number;
  total_cost: number;
  current_daily: { day: string; cost: number }[];
}

export interface CatalogEntry {
  key: string;
  curated: PriceSpec | null;
  litellm: PriceSpec | null;
  litellm_latest: boolean;
}

export interface BackendFilter {
  from?: number;
  to?: number;
  services: string[];
  models: string[];
  projects: string[];
  sessions: string[];
  exclude_subagents: boolean;
}
