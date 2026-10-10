// 使用量画面のテキスト（Claude デスクトップアプリ / claude.ai / Claude Code の /usage）から
// 使用上限の消費率を読み取る。表示形式が変わっても拾えるよう、ラベル・%・リセットまでの時間を
// それぞれ緩く探して組み合わせる。

import type { LimitKind } from "./types";

export interface ParsedLimit {
  kind: LimitKind;
  model: string | null;
  percent: number;
  /** リセットまでの分数（読み取れた場合） */
  resetsInMin: number | null;
  label: string;
}

const MODELS = "fable|opus|sonnet|haiku|mythos";
const WEEK = "週間|週次|週|weekly|current\\s+week|this\\s+week";

const LABELS: { kind: LimitKind; re: RegExp }[] = [
  { kind: "weekly_model", re: new RegExp(`(?:${WEEK})[^\\n%]{0,12}?(${MODELS})`, "gi") },
  { kind: "weekly", re: new RegExp(`(?:${WEEK})[^\\n%]{0,12}?(?:すべてのモデル|すべて|全モデル|all\\s+models|all)`, "gi") },
  {
    kind: "session",
    // 「クラウドセッションクレジット」などを拾わないよう、上限を表す語に限る。
    re: /(?:セッションの上限|現在のセッション|current\s+session|session\s+limit|5\s*時間|5[\s-]?hour)/gi,
  },
];

/** 「4時間20分後」「2日3時間後」「in 4 hr 20 min」「Resets in 2d 3h」などを分に直す。 */
export function parseDuration(text: string): number | null {
  const jp = /((?:\d+\s*日)?\s*(?:\d+\s*時間)?\s*(?:\d+\s*分)?)\s*後/.exec(text);
  // 長い単位から順に試し、語の切れ目で止める（"hr" を "h" で切らないため）。
  const en = /resets?\s+in\s+((?:\d+\s*(?:days?|d|hours?|hrs?|h|minutes?|mins?|m)\b\s*)+)/i.exec(text);
  const src = jp && /\d/.test(jp[1]) ? jp[1] : en ? en[1] : null;
  if (!src) return untilClock(text) ?? untilJpClock(text);
  let min = 0;
  for (const m of src.matchAll(/(\d+)\s*(日|時間|分|days?|d|hours?|hrs?|h|minutes?|mins?|m)/gi)) {
    const n = Number(m[1]);
    const u = m[2].toLowerCase();
    if (u === "日" || u.startsWith("d")) min += n * 1440;
    else if (u === "時間" || u.startsWith("h")) min += n * 60;
    else min += n;
  }
  return min > 0 ? min : null;
}

const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/** 「Resets 3:40pm」「Resets Oct 10, 8am」のような時刻表記を、今からの分数に直す（ローカル時刻とみなす）。 */
function untilClock(text: string, now = new Date()): number | null {
  const m = /resets?\s+(?:(\w{3})\w*\s+(\d{1,2}),?\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)/i.exec(text);
  if (!m) return null;
  let hour = Number(m[3]) % 12;
  if (m[5].toLowerCase() === "pm") hour += 12;
  const t = new Date(now);
  t.setHours(hour, Number(m[4] ?? 0), 0, 0);
  if (m[1] && m[2]) {
    const mon = MONTHS.indexOf(m[1].toLowerCase());
    if (mon < 0) return null;
    t.setMonth(mon, Number(m[2]));
    if (t.getTime() < now.getTime() - 86_400_000) t.setFullYear(t.getFullYear() + 1);
  } else if (t.getTime() <= now.getTime()) {
    t.setDate(t.getDate() + 1);
  }
  const min = Math.round((t.getTime() - now.getTime()) / 60000);
  return min > 0 ? min : null;
}

const WEEKDAYS = "日月火水木金土";

/**
 * 曜日と時刻で表されたリセット時刻を、ローカル時刻の日時に直す。
 * weekday は 0 = 日曜 … 6 = 土曜。Claude は 24 時間以内になると「n時間後」と表示するので、
 * 曜日つきの表示は明日以降を指す。そのため今日と同じ曜日は翌週とみなし、今日を指すときは "today" を渡す。
 * 曜日がなければ（null）、直近の未来のその時刻とする。過去になる場合は null。
 */
export function resolveResetTime(
  weekday: number | "today" | null,
  hour: number,
  minute: number,
  now = new Date(),
): Date | null {
  if (!(hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59)) return null;
  const t = new Date(now);
  t.setHours(hour, minute, 0, 0);
  if (weekday == null) {
    if (t.getTime() <= now.getTime()) t.setDate(t.getDate() + 1);
  } else if (weekday !== "today") {
    t.setDate(t.getDate() + ((weekday - now.getDay() + 7) % 7 || 7));
  }
  return t.getTime() > now.getTime() ? t : null;
}

// 「8:00」「午後3時」「8時30分」（「5時間」の「時」は除く）
const JP_TIME = "(?<ampm>午前|午後)?\\s*(?<h>\\d{1,2})(?::(?<m>\\d{2})|\\s*時(?!間)(?:\\s*(?<m2>\\d{1,2})\\s*分)?)";
const JP_CLOCK = [
  // 8:00 (土)にリセット
  new RegExp(`${JP_TIME}\\s*[(（]\\s*(?<wd>[${WEEKDAYS}])(?:曜日?)?\\s*[)）]`),
  // 土曜日 8時にリセット / (土) 8:00 にリセット（「6日」の「日」を拾わないよう、曜か括弧を必須にする）
  new RegExp(`(?:(?<wd>[${WEEKDAYS}])曜日?|[(（](?<wd2>[${WEEKDAYS}])[)）])\\s*の?\\s*${JP_TIME}`),
  // 15:00にリセット（曜日なし）
  new RegExp(`${JP_TIME}\\s*に?\\s*リセット`),
];

/** 「8:00 (土)にリセット」「土曜日 8時」のような日本語の時刻表記を、今からの分数に直す（ローカル時刻とみなす）。 */
function untilJpClock(text: string, now = new Date()): number | null {
  for (const re of JP_CLOCK) {
    const g = re.exec(text)?.groups;
    if (!g) continue;
    let hour = Number(g.h);
    if (g.ampm === "午後" && hour < 12) hour += 12;
    else if (g.ampm === "午前" && hour === 12) hour = 0;
    const wd = g.wd ?? g.wd2;
    const t = resolveResetTime(wd ? WEEKDAYS.indexOf(wd) : null, hour, Number(g.m ?? g.m2 ?? 0), now);
    if (!t) continue;
    const min = Math.round((t.getTime() - now.getTime()) / 60000);
    if (min > 0) return min;
  }
  return null;
}

export function parseUsageText(text: string): ParsedLimit[] {
  type Hit = { kind: LimitKind; model: string | null; index: number; end: number; label: string };
  const hits: Hit[] = [];
  for (const { kind, re } of LABELS) {
    re.lastIndex = 0;
    for (const m of text.matchAll(re)) {
      const index = m.index ?? 0;
      const end = index + m[0].length;
      // 先に見つかった（より具体的な）ラベルと重なるものは捨てる。
      if (hits.some((h) => index < h.end && end > h.index)) continue;
      hits.push({
        kind,
        model: kind === "weekly_model" ? m[1].toLowerCase() : null,
        index,
        end,
        label: m[0].replace(/\s+/g, " ").trim(),
      });
    }
  }
  hits.sort((a, b) => a.index - b.index);

  const out: ParsedLimit[] = [];
  hits.forEach((h, i) => {
    const seg = text.slice(h.index, i + 1 < hits.length ? hits[i + 1].index : text.length);
    const pm = /(\d+(?:\.\d+)?)\s*%/.exec(seg);
    if (!pm) return;
    const percent = Number(pm[1]);
    if (!(percent >= 0 && percent <= 100)) return;
    // 同じ種類が重複したら最初のものを使う（説明文などに同じ語が出る場合がある）。
    if (out.some((o) => o.kind === h.kind && o.model === h.model)) return;
    out.push({ kind: h.kind, model: h.model, percent, resetsInMin: parseDuration(seg), label: h.label });
  });
  return out;
}
