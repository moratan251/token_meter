import type { ServiceId } from "./types";

export const SERVICE_LABEL: Record<ServiceId, string> = {
  claude: "Claude Code",
  codex: "Codex",
};

const nf0 = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 0 });
const nf1 = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 1 });
const nf2 = new Intl.NumberFormat("ja-JP", { minimumFractionDigits: 2, maximumFractionDigits: 2 });

export function int(n: number): string {
  return nf0.format(n);
}

/** 1234567 → "1.23M" */
export function compact(n: number): string {
  const a = Math.abs(n);
  if (a >= 1e9) return (n / 1e9).toFixed(a >= 1e10 ? 1 : 2) + "B";
  if (a >= 1e6) return (n / 1e6).toFixed(a >= 1e7 ? 1 : 2) + "M";
  if (a >= 1e3) return (n / 1e3).toFixed(a >= 1e4 ? 0 : 1) + "K";
  return nf0.format(n);
}

export interface Money {
  currency: "USD" | "JPY";
  rate: number; // 1 USD あたりの円
}

export function toDisplay(usd: number, m: Money): number {
  return m.currency === "JPY" ? usd * m.rate : usd;
}

/** `from` 通貨建ての金額を表示通貨に換算する。 */
export function convert(amount: number, from: string, m: Money): number {
  if (from === m.currency) return amount;
  if (from === "USD" && m.currency === "JPY") return amount * m.rate;
  if (from === "JPY" && m.currency === "USD") return amount / m.rate;
  return amount;
}

export function money(usd: number, m: Money, opts: { compact?: boolean } = {}): string {
  return moneyIn(toDisplay(usd, m), m.currency, opts);
}

/** すでに `currency` 建ての金額を整形する。 */
export function moneyIn(v: number, currency: string, opts: { compact?: boolean } = {}): string {
  if (currency === "JPY") {
    if (opts.compact && Math.abs(v) >= 1e6) return "¥" + nf1.format(v / 1e4) + "万";
    return "¥" + nf0.format(v);
  }
  if (opts.compact && Math.abs(v) >= 1e4) return "$" + compact(v);
  if (Math.abs(v) >= 100) return "$" + nf0.format(v);
  return "$" + nf2.format(v);
}

export function moneyAxis(v: number, currency: string): string {
  if (currency === "JPY") {
    if (Math.abs(v) >= 1e4) return "¥" + nf1.format(v / 1e4) + "万";
    return "¥" + nf0.format(v);
  }
  if (Math.abs(v) >= 1e3) return "$" + compact(v);
  return "$" + (Math.abs(v) < 10 && v !== 0 ? v.toFixed(1) : nf0.format(v));
}

export function pct(v: number, digits = 1): string {
  return (v * 100).toFixed(digits) + "%";
}

export function ratio(v: number): string {
  if (!isFinite(v)) return "—";
  return (v >= 10 ? v.toFixed(1) : v.toFixed(2)) + "倍";
}

export function dateTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function date(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())}`;
}

export function pad(n: number): string {
  return n < 10 ? "0" + n : String(n);
}

export function ymd(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function parseYmd(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return null;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

export function relative(ts: number): string {
  const s = (Date.now() - ts) / 1000;
  if (s < 60) return "たった今";
  if (s < 3600) return `${Math.floor(s / 60)}分前`;
  if (s < 86400) return `${Math.floor(s / 3600)}時間前`;
  return `${Math.floor(s / 86400)}日前`;
}

/** プロジェクト名の短い表示用に、パスの末尾部分を返す。 */
export function projectLabel(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length === 0) return path;
  const last = parts[parts.length - 1];
  // デスクトップアプリのスクラッチ作業フォルダは "scratch-YYYY-MM-DD-xxxxxx" という名前。
  if (/^scratch-\d{4}-\d{2}-\d{2}/.test(last)) return "スクラッチ " + last.slice(8, 18);
  return last;
}

export function shortId(id: string): string {
  return id.length > 13 ? id.slice(0, 8) + "…" : id;
}
