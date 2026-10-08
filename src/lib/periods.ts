import { parseYmd, ymd } from "./format";

export type PeriodId =
  | "today"
  | "7d"
  | "30d"
  | "90d"
  | "this_month"
  | "last_month"
  | "this_year"
  | "all"
  | "custom";

export const PERIODS: { id: PeriodId; label: string }[] = [
  { id: "today", label: "今日" },
  { id: "7d", label: "7日" },
  { id: "30d", label: "30日" },
  { id: "90d", label: "90日" },
  { id: "this_month", label: "今月" },
  { id: "last_month", label: "先月" },
  { id: "this_year", label: "今年" },
  { id: "all", label: "全期間" },
];

function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

/** プリセットに対応する日付範囲（両端を含むローカル日付、null は制限なし）。 */
export function presetRange(id: PeriodId, now = new Date()): { from: string | null; to: string | null } {
  const today = startOfDay(now);
  switch (id) {
    case "today":
      return { from: ymd(today), to: ymd(today) };
    case "7d":
      return { from: ymd(addDays(today, -6)), to: ymd(today) };
    case "30d":
      return { from: ymd(addDays(today, -29)), to: ymd(today) };
    case "90d":
      return { from: ymd(addDays(today, -89)), to: ymd(today) };
    case "this_month":
      return { from: ymd(new Date(today.getFullYear(), today.getMonth(), 1)), to: ymd(today) };
    case "last_month": {
      const first = new Date(today.getFullYear(), today.getMonth() - 1, 1);
      const last = new Date(today.getFullYear(), today.getMonth(), 0);
      return { from: ymd(first), to: ymd(last) };
    }
    case "this_year":
      return { from: ymd(new Date(today.getFullYear(), 0, 1)), to: ymd(today) };
    default:
      return { from: null, to: null };
  }
}

/** 両端を含むローカル日付から [from, to) のエポックミリ秒を求める。 */
export function toMillis(from: string | null, to: string | null): { from?: number; to?: number } {
  const f = from ? parseYmd(from) : null;
  const t = to ? parseYmd(to) : null;
  return {
    from: f ? f.getTime() : undefined,
    to: t ? addDays(t, 1).getTime() : undefined,
  };
}

/** [from, to) の直前にある同じ長さの期間。 */
export function previousRange(from: number, to: number): { from: number; to: number } {
  const len = to - from;
  return { from: from - len, to: from };
}
