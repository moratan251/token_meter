//! 統合済みの記録に対する絞り込みと集計。日付の区切りはすべて PC のローカル
//! タイムゾーンで行う。

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Duration, Local, Months, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::pricing::{cost_of, Cost, PriceBook, Resolved};
use crate::settings::{Plan, Settings};
use crate::store::{Rec, Store};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub services: Vec<String>,
    pub models: Vec<String>,
    pub projects: Vec<String>,
    pub sessions: Vec<String>,
    pub exclude_subagents: bool,
}

struct Compiled {
    services: [bool; 2],
    models: Option<Vec<bool>>,
    projects: Option<Vec<bool>>,
    sessions: Option<HashSet<u32>>,
    exclude_sub: bool,
}

impl Compiled {
    fn new(st: &Store, f: &Filter) -> Compiled {
        let services = if f.services.is_empty() {
            [true, true]
        } else {
            [
                f.services.iter().any(|s| s == "claude"),
                f.services.iter().any(|s| s == "codex"),
            ]
        };
        let mask = |names: &[String], len: usize, lookup: &dyn Fn(&str) -> Option<u32>| {
            if names.is_empty() {
                return None;
            }
            let mut v = vec![false; len];
            for n in names {
                if let Some(i) = lookup(n) {
                    v[i as usize] = true;
                }
            }
            Some(v)
        };
        Compiled {
            services,
            models: mask(&f.models, st.models.names.len(), &|n| st.models.get(n)),
            projects: mask(&f.projects, st.projects.names.len(), &|n| st.projects.get(n)),
            sessions: if f.sessions.is_empty() {
                None
            } else {
                Some(f.sessions.iter().filter_map(|s| st.sessions.get(s)).collect())
            },
            exclude_sub: f.exclude_subagents,
        }
    }

    fn matches(&self, r: &Rec) -> bool {
        self.services[svc_idx(r.service)]
            && !(self.exclude_sub && r.flags & F_SUBAGENT != 0)
            && self.models.as_ref().map(|m| m[r.model as usize]).unwrap_or(true)
            && self.projects.as_ref().map(|m| m[r.project as usize]).unwrap_or(true)
            && self.sessions.as_ref().map(|s| s.contains(&r.session)).unwrap_or(true)
    }
}

fn svc_idx(s: Service) -> usize {
    match s {
        Service::Claude => 0,
        Service::Codex => 1,
    }
}

/// モデルごとに決定した単価。
pub fn price_table(st: &Store, book: &PriceBook, settings: &Settings) -> Vec<Option<Resolved>> {
    st.models
        .names
        .iter()
        .map(|m| book.resolve(m, settings))
        .collect()
}

pub(crate) fn rec_cost(prices: &[Option<Resolved>], r: &Rec, settings: &Settings) -> Cost {
    prices[r.model as usize]
        .as_ref()
        .map(|p| cost_of(&p.spec, &r.t, r.flags, settings))
        .unwrap_or_default()
}

// ---------------------------------------------------------------- ローカル時刻

/// UTC のミリ秒をローカル時刻に変換する。時差は 1 時間単位でメモ化する。
struct LocalConv {
    hour: i64,
    offset_ms: i64,
}

impl LocalConv {
    fn new() -> Self {
        LocalConv {
            hour: i64::MIN,
            offset_ms: 0,
        }
    }

    fn at(&mut self, ts: i64) -> NaiveDateTime {
        let hour = ts.div_euclid(3_600_000);
        if hour != self.hour {
            self.hour = hour;
            self.offset_ms = Local
                .timestamp_millis_opt(ts)
                .single()
                .map(|d| d.offset().fix().local_minus_utc() as i64 * 1000)
                .unwrap_or(0);
        }
        chrono::DateTime::from_timestamp_millis(ts + self.offset_ms)
            .map(|d| d.naive_utc())
            .unwrap_or_default()
    }
}

pub fn local_to_ts(ndt: NaiveDateTime) -> i64 {
    Local
        .from_local_datetime(&ndt)
        .earliest()
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| ndt.and_utc().timestamp_millis())
}

pub(crate) fn date_ts(d: NaiveDate) -> i64 {
    local_to_ts(d.and_hms_opt(0, 0, 0).unwrap())
}

#[derive(Clone, Copy, PartialEq)]
enum Bucket {
    Hour,
    Day,
    Week,
    Month,
}

impl Bucket {
    fn parse(s: &str) -> Bucket {
        match s {
            "hour" => Bucket::Hour,
            "week" => Bucket::Week,
            "month" => Bucket::Month,
            _ => Bucket::Day,
        }
    }

    fn start(self, t: NaiveDateTime, week_start: u8) -> NaiveDateTime {
        let day = t.date();
        match self {
            Bucket::Hour => day.and_hms_opt(t.hour(), 0, 0).unwrap(),
            Bucket::Day => day.and_hms_opt(0, 0, 0).unwrap(),
            Bucket::Week => {
                let back = if week_start == 0 {
                    day.weekday().num_days_from_sunday()
                } else {
                    day.weekday().num_days_from_monday()
                };
                (day - Duration::days(back as i64)).and_hms_opt(0, 0, 0).unwrap()
            }
            Bucket::Month => NaiveDate::from_ymd_opt(day.year(), day.month(), 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        }
    }

    fn next(self, t: NaiveDateTime) -> NaiveDateTime {
        match self {
            Bucket::Hour => t + Duration::hours(1),
            Bucket::Day => t + Duration::days(1),
            Bucket::Week => t + Duration::days(7),
            Bucket::Month => t.checked_add_months(Months::new(1)).unwrap_or(t + Duration::days(31)),
        }
    }

    fn label(self, t: NaiveDateTime) -> String {
        match self {
            Bucket::Hour => t.format("%Y-%m-%d %H:00").to_string(),
            Bucket::Day | Bucket::Week => t.format("%Y-%m-%d").to_string(),
            Bucket::Month => t.format("%Y-%m").to_string(),
        }
    }
}

// ---------------------------------------------------------------- ダッシュボード

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Agg {
    pub cost: Cost,
    pub total_cost: f64,
    pub tokens: Tokens,
    pub requests: u64,
}

impl Agg {
    fn add(&mut self, c: &Cost, t: &Tokens) {
        self.cost.add(c);
        self.total_cost += c.total();
        self.tokens.add(t);
        self.requests += 1;
    }
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub key: String,
    pub service: Option<String>,
    pub agg: Agg,
    pub sessions: u64,
}

#[derive(Debug, Serialize)]
pub struct Series {
    pub key: String,
    pub cost: Vec<f64>,
    pub tokens: Vec<u64>,
    pub total: f64,
}

#[derive(Debug, Serialize)]
pub struct Unpriced {
    pub model: String,
    pub requests: u64,
    pub tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct PlanFee {
    pub plan_id: String,
    pub service: String,
    pub name: String,
    pub currency: String,
    pub amount: f64,
}

#[derive(Debug, Serialize)]
pub struct Dashboard {
    pub totals: Agg,
    pub subagent_cost: f64,
    pub sessions: u64,
    pub active_days: u64,
    pub span_days: f64,
    pub by_service: Vec<Group>,
    pub by_model: Vec<Group>,
    pub by_project: Vec<Group>,
    pub buckets: Vec<String>,
    pub series: Vec<Series>,
    /// [曜日 月..日][時] の金額
    pub heatmap: Vec<Vec<f64>>,
    pub unpriced: Vec<Unpriced>,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub range_from: i64,
    pub range_to: i64,
    pub fees: Vec<PlanFee>,
    /// 登録したプランの契約期間内の API 換算額（元取り倍率の分子。契約していない期間の利用は除く）
    pub plan_cost: f64,
    /// モデル・プロジェクト・セッションなどで絞り込んでいる（料金は絞り込めないので倍率は参考値）
    pub subset: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DashOpts {
    pub bucket: String,
    pub split: String,
    pub top: usize,
}

impl Default for DashOpts {
    fn default() -> Self {
        DashOpts {
            bucket: "day".into(),
            split: "model".into(),
            top: 8,
        }
    }
}

#[derive(Default)]
struct GroupAcc {
    agg: Agg,
    sessions: HashSet<u32>,
    service: Option<Service>,
}

impl GroupAcc {
    fn add(&mut self, r: &Rec, c: &Cost) {
        self.agg.add(c, &r.t);
        self.sessions.insert(r.session);
        self.service = Some(r.service);
    }
}

fn groups(map: HashMap<u32, GroupAcc>, name: impl Fn(u32) -> String) -> Vec<Group> {
    let mut v: Vec<Group> = map
        .into_iter()
        .map(|(k, g)| Group {
            key: name(k),
            service: g.service.map(|s| s.id().to_string()),
            agg: g.agg,
            sessions: g.sessions.len() as u64,
        })
        .collect();
    v.sort_by(|a, b| {
        b.agg
            .total_cost
            .total_cmp(&a.agg.total_cost)
            .then(b.agg.tokens.total().cmp(&a.agg.tokens.total()))
    });
    v
}

pub fn dashboard(st: &Store, book: &PriceBook, settings: &Settings, f: &Filter, opts: &DashOpts) -> Dashboard {
    let cf = Compiled::new(st, f);
    let prices = price_table(st, book, settings);
    let range = st.range(f.from, f.to);
    let now = chrono::Utc::now().timestamp_millis();
    let bucket = Bucket::parse(&opts.bucket);
    let mut conv = LocalConv::new();

    let matching: Vec<&Rec> = st.recs[range].iter().filter(|r| cf.matches(r)).collect();
    let first_ts = matching.first().map(|r| r.ts);
    let last_ts = matching.last().map(|r| r.ts);

    // グラフの範囲: 指定された期間（現在時刻まで）、指定がなければデータのある範囲。
    let span_from = f.from.or(first_ts).unwrap_or(now);
    let span_to = f
        .to
        .map(|t| t.min(now.max(last_ts.unwrap_or(0) + 1)))
        .or(last_ts.map(|t| t + 1))
        .unwrap_or(now)
        .max(span_from + 1);

    let mut buckets = Vec::new();
    let mut bucket_index: HashMap<i64, usize> = HashMap::new();
    {
        let mut b = bucket.start(conv.at(span_from), settings.week_start);
        let end = conv.at(span_to - 1);
        while b <= end && buckets.len() < 20_000 {
            bucket_index.insert(b.and_utc().timestamp(), buckets.len());
            buckets.push(bucket.label(b));
            b = bucket.next(b);
        }
    }

    let mut totals = Agg::default();
    let mut subagent_cost = 0.0;
    let mut sessions = HashSet::new();
    let mut days = HashSet::new();
    let mut by_service: HashMap<u32, GroupAcc> = HashMap::new();
    let mut by_model: HashMap<u32, GroupAcc> = HashMap::new();
    let mut by_project: HashMap<u32, GroupAcc> = HashMap::new();
    let mut split: HashMap<u32, (Vec<f64>, Vec<u64>, f64)> = HashMap::new();
    let mut heat = vec![vec![0.0; 24]; 7];
    let mut unpriced: HashMap<u32, (u64, u64)> = HashMap::new();
    let nb = buckets.len();

    for r in &matching {
        let c = rec_cost(&prices, r, settings);
        let total = c.total();
        totals.add(&c, &r.t);
        if r.flags & F_SUBAGENT != 0 {
            subagent_cost += total;
        }
        sessions.insert(r.session);
        by_service.entry(svc_idx(r.service) as u32).or_default().add(r, &c);
        by_model.entry(r.model).or_default().add(r, &c);
        by_project.entry(r.project).or_default().add(r, &c);
        if prices[r.model as usize].is_none() {
            let u = unpriced.entry(r.model).or_default();
            u.0 += 1;
            u.1 += r.t.total();
        }

        let lt = conv.at(r.ts);
        days.insert(lt.date());
        heat[lt.weekday().num_days_from_monday() as usize][lt.hour() as usize] += total;

        let bkey = bucket.start(lt, settings.week_start).and_utc().timestamp();
        if let Some(&bi) = bucket_index.get(&bkey) {
            let skey = match opts.split.as_str() {
                "service" => svc_idx(r.service) as u32,
                "model" => r.model,
                "project" => r.project,
                _ => 0,
            };
            let s = split
                .entry(skey)
                .or_insert_with(|| (vec![0.0; nb], vec![0; nb], 0.0));
            s.0[bi] += total;
            s.1[bi] += r.t.total();
            s.2 += total;
        }
    }

    let split_name = |k: u32| -> String {
        match opts.split.as_str() {
            "service" => Service::ALL[k as usize].id().to_string(),
            "model" => st.models.name(k).to_string(),
            "project" => st.projects.name(k).to_string(),
            _ => "total".to_string(),
        }
    };
    let mut ordered: Vec<(u32, (Vec<f64>, Vec<u64>, f64))> = split.into_iter().collect();
    ordered.sort_by(|a, b| b.1 .2.total_cmp(&a.1 .2).then(a.0.cmp(&b.0)));
    let top = opts.top.max(1);
    let mut series: Vec<Series> = Vec::new();
    let mut other: Option<Series> = None;
    for (i, (k, (cost, tokens, total))) in ordered.into_iter().enumerate() {
        if i < top {
            series.push(Series {
                key: split_name(k),
                cost,
                tokens,
                total,
            });
        } else {
            let o = other.get_or_insert_with(|| Series {
                key: "__other__".into(),
                cost: vec![0.0; nb],
                tokens: vec![0; nb],
                total: 0.0,
            });
            for j in 0..nb {
                o.cost[j] += cost[j];
                o.tokens[j] += tokens[j];
            }
            o.total += total;
        }
    }
    series.extend(other);

    let mut unpriced: Vec<Unpriced> = unpriced
        .into_iter()
        .map(|(m, (requests, tokens))| Unpriced {
            model: st.models.name(m).to_string(),
            requests,
            tokens,
        })
        .collect();
    unpriced.sort_by(|a, b| b.tokens.cmp(&a.tokens));

    let services: Vec<&str> = Service::ALL
        .iter()
        .filter(|s| cf.services[svc_idx(**s)])
        .map(|s| s.id())
        .collect();
    let fee_from = f.from.unwrap_or(i64::MIN);
    let fee_to = f.to.unwrap_or(now).min(now);
    let fees = settings
        .plans
        .iter()
        .filter(|p| services.contains(&p.service.as_str()))
        .map(|p| PlanFee {
            plan_id: p.id.clone(),
            service: p.service.clone(),
            name: p.name.clone(),
            currency: p.currency.clone(),
            amount: prorated_fee(p, fee_from, fee_to),
        })
        .filter(|p| p.amount > 0.0)
        .collect();

    // 元取り倍率の分子は、プランの契約期間内の利用だけにする。
    let plan_spans: Vec<(Service, i64, i64)> = settings
        .plans
        .iter()
        .filter_map(|p| {
            let svc = Service::from_id(&p.service)?;
            let start = date_ts(parse_date(&p.start)?);
            let end = p.end.as_deref().and_then(parse_date).map(date_ts).unwrap_or(i64::MAX);
            Some((svc, start, end))
        })
        .collect();
    let plan_cost: f64 = matching
        .iter()
        .filter(|r| plan_spans.iter().any(|&(s, a, b)| s == r.service && r.ts >= a && r.ts < b))
        .map(|r| rec_cost(&prices, r, settings).total())
        .sum();
    let subset = !f.models.is_empty() || !f.projects.is_empty() || !f.sessions.is_empty() || f.exclude_subagents;

    Dashboard {
        totals,
        subagent_cost,
        sessions: sessions.len() as u64,
        active_days: days.len() as u64,
        span_days: (span_to - span_from) as f64 / 86_400_000.0,
        by_service: groups(by_service, |k| Service::ALL[k as usize].id().to_string()),
        by_model: groups(by_model, |k| st.models.name(k).to_string()),
        by_project: groups(by_project, |k| st.projects.name(k).to_string()),
        buckets,
        series,
        heatmap: heat,
        unpriced,
        first_ts,
        last_ts,
        range_from: span_from,
        range_to: span_to,
        fees,
        plan_cost,
        subset,
    }
}

// ---------------------------------------------------------------- 明細

#[derive(Debug, Serialize)]
pub struct Row {
    pub key: String,
    pub title: Option<String>,
    pub service: Option<String>,
    pub project: Option<String>,
    pub models: Vec<String>,
    pub first_ts: i64,
    pub last_ts: i64,
    pub agg: Agg,
    pub sessions: u64,
    pub subagent_cost: f64,
}

#[derive(Default)]
struct RowAcc {
    agg: Agg,
    first: i64,
    last: i64,
    sessions: HashSet<u32>,
    models: HashMap<u32, f64>,
    projects: HashMap<u32, f64>,
    services: HashSet<Service>,
    subagent_cost: f64,
    label: String,
}

pub fn table(st: &Store, book: &PriceBook, settings: &Settings, f: &Filter, group_by: &str) -> Vec<Row> {
    let cf = Compiled::new(st, f);
    let prices = price_table(st, book, settings);
    let mut conv = LocalConv::new();
    let time_bucket = match group_by {
        "hour" | "day" | "week" | "month" => Some(Bucket::parse(group_by)),
        _ => None,
    };
    let mut acc: HashMap<i64, RowAcc> = HashMap::new();

    for r in st.recs[st.range(f.from, f.to)].iter().filter(|r| cf.matches(r)) {
        let c = rec_cost(&prices, r, settings);
        let (key, label) = match (time_bucket, group_by) {
            (Some(b), _) => {
                let s = b.start(conv.at(r.ts), settings.week_start);
                (s.and_utc().timestamp(), Some(s))
            }
            (None, "model") => (r.model as i64, None),
            (None, "project") => (r.project as i64, None),
            (None, "service") => (svc_idx(r.service) as i64, None),
            _ => (r.session as i64, None),
        };
        let a = acc.entry(key).or_insert_with(|| RowAcc {
            first: r.ts,
            label: match (time_bucket, label) {
                (Some(b), Some(s)) => b.label(s),
                _ => String::new(),
            },
            ..Default::default()
        });
        a.agg.add(&c, &r.t);
        a.first = a.first.min(r.ts);
        a.last = a.last.max(r.ts);
        a.sessions.insert(r.session);
        *a.models.entry(r.model).or_default() += c.total() + 1e-12 * r.t.total() as f64;
        *a.projects.entry(r.project).or_default() += c.total() + 1e-12 * r.t.total() as f64;
        a.services.insert(r.service);
        if r.flags & F_SUBAGENT != 0 {
            a.subagent_cost += c.total();
        }
    }

    let mut rows: Vec<Row> = acc
        .into_iter()
        .map(|(k, a)| {
            let mut models: Vec<(u32, f64)> = a.models.into_iter().collect();
            models.sort_by(|x, y| y.1.total_cmp(&x.1));
            let top_project = a
                .projects
                .iter()
                .max_by(|x, y| x.1.total_cmp(y.1))
                .map(|(p, _)| st.projects.name(*p).to_string());
            let k32 = k as u32;
            let key = match (time_bucket, group_by) {
                (Some(_), _) => a.label.clone(),
                (None, "model") => st.models.name(k32).to_string(),
                (None, "project") => st.projects.name(k32).to_string(),
                (None, "service") => Service::ALL[k as usize].id().to_string(),
                _ => st.sessions.name(k32).to_string(),
            };
            let title = if group_by == "session" {
                st.titles.get(&k32).cloned()
            } else {
                None
            };
            let service = if a.services.len() == 1 {
                a.services.iter().next().map(|s| s.id().to_string())
            } else {
                None
            };
            Row {
                key,
                title,
                service,
                project: top_project,
                models: models.into_iter().map(|(m, _)| st.models.name(m).to_string()).collect(),
                first_ts: a.first,
                last_ts: a.last,
                agg: a.agg,
                sessions: a.sessions.len() as u64,
                subagent_cost: a.subagent_cost,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.agg.total_cost.total_cmp(&a.agg.total_cost));
    rows
}

// ---------------------------------------------------------------- プラン

pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// プランの請求サイクルを (開始, 終了, 本来のサイクル長) の UTC ミリ秒で返す。
/// プランの終了日と `until` で打ち切る。
fn plan_cycles(p: &Plan, until: i64) -> Vec<(i64, i64, i64)> {
    let Some(start) = parse_date(&p.start) else { return Vec::new() };
    let end = p.end.as_deref().and_then(parse_date).map(date_ts).unwrap_or(i64::MAX);
    let mut out = Vec::new();
    for i in 0..1200u32 {
        let (Some(cs), Some(ce)) = (
            start.checked_add_months(Months::new(i)),
            start.checked_add_months(Months::new(i + 1)),
        ) else {
            break;
        };
        let (cs, ce) = (date_ts(cs), date_ts(ce));
        if cs >= end || cs >= until {
            break;
        }
        out.push((cs, ce.min(end), ce - cs));
    }
    out
}

/// [from, to) に割り当てられるサブスク料金。各サイクル内で時間按分する。
pub fn prorated_fee(p: &Plan, from: i64, to: i64) -> f64 {
    plan_cycles(p, to)
        .into_iter()
        .map(|(cs, ce, full)| {
            let a = cs.max(from);
            let b = ce.min(to);
            if b > a {
                p.price * (b - a) as f64 / full as f64
            } else {
                0.0
            }
        })
        .sum()
}

#[derive(Debug, Serialize)]
pub struct CycleOut {
    pub start: i64,
    pub end: i64,
    /// （打ち切られた場合は按分した）サイクルの料金。プランの通貨建て。
    pub fee: f64,
    pub cost: f64,
    pub tokens: u64,
    pub requests: u64,
    pub current: bool,
    /// サイクルの経過割合（終わったサイクルは 1.0）。
    pub elapsed: f64,
}

#[derive(Debug, Serialize)]
pub struct DailyPoint {
    pub day: String,
    pub cost: f64,
}

#[derive(Debug, Serialize)]
pub struct PlanReport {
    pub plan: Plan,
    pub cycles: Vec<CycleOut>,
    pub total_fee: f64,
    pub total_cost: f64,
    pub current_daily: Vec<DailyPoint>,
}

pub fn subscription_report(st: &Store, book: &PriceBook, settings: &Settings) -> Vec<PlanReport> {
    let prices = price_table(st, book, settings);
    let now = chrono::Utc::now().timestamp_millis();
    let mut conv = LocalConv::new();
    let mut out = Vec::new();
    for p in &settings.plans {
        let Some(svc) = Service::from_id(&p.service) else { continue };
        let mut cycles = Vec::new();
        let mut current_daily = Vec::new();
        for (cs, ce, full) in plan_cycles(p, now) {
            let mut cost = 0.0;
            let mut tokens = 0;
            let mut requests = 0;
            let current = now < ce;
            let mut daily: Vec<(NaiveDate, f64)> = Vec::new();
            for r in st.recs[st.range(Some(cs), Some(ce))].iter().filter(|r| r.service == svc) {
                let c = rec_cost(&prices, r, settings).total();
                cost += c;
                tokens += r.t.total();
                requests += 1;
                if current {
                    let d = conv.at(r.ts).date();
                    match daily.last_mut() {
                        Some((ld, v)) if *ld == d => *v += c,
                        _ => daily.push((d, c)),
                    }
                }
            }
            if current {
                // サイクル開始日から今日まで 1 日 1 点。使用のない日は 0 で埋める。
                let mut d = conv.at(cs).date();
                let today = conv.at(now).date();
                let mut i = 0;
                while d <= today {
                    let v = if i < daily.len() && daily[i].0 == d {
                        i += 1;
                        daily[i - 1].1
                    } else {
                        0.0
                    };
                    current_daily.push(DailyPoint {
                        day: d.format("%Y-%m-%d").to_string(),
                        cost: v,
                    });
                    d += Duration::days(1);
                }
            }
            cycles.push(CycleOut {
                start: cs,
                end: ce,
                fee: p.price * (ce - cs) as f64 / full as f64,
                cost,
                tokens,
                requests,
                current,
                elapsed: if current {
                    ((now - cs) as f64 / (ce - cs) as f64).clamp(0.0, 1.0)
                } else {
                    1.0
                },
            });
        }
        out.push(PlanReport {
            plan: p.clone(),
            total_fee: cycles.iter().map(|c| c.fee).sum(),
            total_cost: cycles.iter().map(|c| c.cost).sum(),
            cycles,
            current_daily,
        });
    }
    out
}

// ---------------------------------------------------------------- メタ情報

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub services: Vec<String>,
    pub requests: u64,
    pub tokens: u64,
    pub cost: f64,
    pub last_ts: i64,
    pub price: Option<Resolved>,
}

#[derive(Debug, Serialize)]
pub struct ProjectInfo {
    pub path: String,
    pub services: Vec<String>,
    pub requests: u64,
    pub cost: f64,
    pub last_ts: i64,
}

pub fn model_and_project_info(st: &Store, book: &PriceBook, settings: &Settings) -> (Vec<ModelInfo>, Vec<ProjectInfo>) {
    let prices = price_table(st, book, settings);
    let mut models: Vec<(u64, u64, f64, i64, HashSet<Service>)> =
        vec![(0, 0, 0.0, 0, HashSet::new()); st.models.names.len()];
    let mut projects: Vec<(u64, f64, i64, HashSet<Service>)> =
        vec![(0, 0.0, 0, HashSet::new()); st.projects.names.len()];
    for r in &st.recs {
        let c = rec_cost(&prices, r, settings).total();
        let m = &mut models[r.model as usize];
        m.0 += 1;
        m.1 += r.t.total();
        m.2 += c;
        m.3 = m.3.max(r.ts);
        m.4.insert(r.service);
        let p = &mut projects[r.project as usize];
        p.0 += 1;
        p.1 += c;
        p.2 = p.2.max(r.ts);
        p.3.insert(r.service);
    }
    let svc_list = |s: &HashSet<Service>| {
        let mut v: Vec<String> = s.iter().map(|x| x.id().to_string()).collect();
        v.sort();
        v
    };
    let mut mi: Vec<ModelInfo> = models
        .iter()
        .enumerate()
        .map(|(i, m)| ModelInfo {
            name: st.models.names[i].clone(),
            services: svc_list(&m.4),
            requests: m.0,
            tokens: m.1,
            cost: m.2,
            last_ts: m.3,
            price: prices[i].clone(),
        })
        .filter(|m| m.requests > 0)
        .collect();
    mi.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    let mut pi: Vec<ProjectInfo> = projects
        .iter()
        .enumerate()
        .map(|(i, p)| ProjectInfo {
            path: st.projects.names[i].clone(),
            services: svc_list(&p.3),
            requests: p.0,
            cost: p.1,
            last_ts: p.2,
        })
        .filter(|p| p.requests > 0)
        .collect();
    pi.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    (mi, pi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(start: &str, end: Option<&str>) -> Plan {
        Plan {
            id: "p".into(),
            service: "claude".into(),
            name: "Max".into(),
            price: 100.0,
            currency: "USD".into(),
            start: start.into(),
            end: end.map(|s| s.into()),
        }
    }

    #[test]
    fn fee_proration() {
        let p = plan("2026-01-31", None);
        let jan31 = date_ts(parse_date("2026-01-31").unwrap());
        let feb28 = date_ts(parse_date("2026-02-28").unwrap());
        let mar31 = date_ts(parse_date("2026-03-31").unwrap());
        // サイクルの境界は月末に丸められ、ずれていかない。
        assert!((prorated_fee(&p, jan31, feb28) - 100.0).abs() < 1e-9);
        assert!((prorated_fee(&p, jan31, mar31) - 200.0).abs() < 1e-9);
        let ended = plan("2026-01-31", Some("2026-02-14"));
        assert!(prorated_fee(&ended, jan31, mar31) < 100.0);
    }
}
