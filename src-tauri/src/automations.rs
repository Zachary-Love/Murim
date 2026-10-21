//! Automations: scheduled agent runs. Each run is an ordinary session keyed `automation:<auto-id>:<run-id>`.
use crate::sessions;
use crate::settings;
use chrono::{DateTime, Datelike, Duration as Days, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const KEEP: usize = 20;
const MISSED_AFTER: i64 = 24 * 3600;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Daily,
    Weekdays,
    Weekly,
    Biweekly,
    Monthly,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Schedule {
    pub kind: Kind,
    /// "HH:MM" local
    pub time: String,
    /// 0 = Sunday; weekly/biweekly
    pub days: Vec<u8>,
    /// "YYYY-MM-DD"; biweekly anchor
    pub start: String,
    /// 1..31; monthly
    pub day_of_month: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Started,
    Failed,
    Missed,
    Skipped,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoRun {
    pub id: String,
    pub started: i64,
    pub status: RunStatus,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub folder: String,
    pub schedule: Schedule,
    pub enabled: bool,
    #[serde(default)]
    pub next_due: Option<i64>,
    /// "" = Claude's default, a Claude alias (opus, sonnet, haiku) or "ollama:<model>"
    #[serde(default)]
    pub model: String,
    /// newest first, ≤ KEEP
    #[serde(default)]
    pub runs: Vec<AutoRun>,
}

/// The model an automation run (`automation:<auto-id>:<run-id>`) uses; looked up at spawn so resumes keep it.
pub fn model_of(app: &AppHandle, session_id: &str) -> String {
    let id = session_id.split(':').nth(1).unwrap_or_default();
    app.state::<Automations>().0.lock().unwrap().iter().find(|a| a.id == id).map(|a| a.model.clone()).unwrap_or_default()
}

#[derive(Default)]
pub struct Automations(Mutex<Vec<Automation>>);

/// Serializes run starts (scheduler vs Run now) so one automation can't start twice.
static STARTING: Mutex<()> = Mutex::new(());

// ---- schedule ----

fn check(s: &Schedule) -> Result<NaiveTime, String> {
    let t = NaiveTime::parse_from_str(&s.time, "%H:%M").ok().filter(|_| s.time.len() == 5).ok_or("Time must be HH:MM")?;
    if matches!(s.kind, Kind::Weekly | Kind::Biweekly) && (s.days.is_empty() || s.days.iter().any(|&d| d > 6)) {
        return Err("Pick at least one day".into());
    }
    if s.kind == Kind::Biweekly && start_date(s).is_none() {
        return Err("Pick a start date".into());
    }
    if s.kind == Kind::Monthly && !(1..=31).contains(&s.day_of_month) {
        return Err("Day of month must be 1–31".into());
    }
    Ok(t)
}

fn start_date(s: &Schedule) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(&s.start, "%Y-%m-%d").ok()
}

fn sunday_of(d: NaiveDate) -> NaiveDate {
    d - Days::days(d.weekday().num_days_from_sunday() as i64)
}

fn last_day(d: NaiveDate) -> u32 {
    let (y, m) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap().pred_opt().unwrap().day()
}

fn fires_on(s: &Schedule, d: NaiveDate) -> bool {
    let dow = d.weekday().num_days_from_sunday() as u8;
    match s.kind {
        Kind::Daily => true,
        Kind::Weekdays => !matches!(d.weekday(), Weekday::Sat | Weekday::Sun),
        Kind::Weekly => s.days.contains(&dow),
        Kind::Biweekly => start_date(s).is_some_and(|st| d >= st && (sunday_of(d) - sunday_of(st)).num_days() / 7 % 2 == 0 && s.days.contains(&dow)),
        Kind::Monthly => d.day() == s.day_of_month.min(last_day(d)),
    }
}

/// Next fire time strictly after `after`; None only for an invalid schedule.
/// DST: a nonexistent local time fires at the next valid minute, an ambiguous one at its first occurrence.
pub fn next_after<Tz: TimeZone>(s: &Schedule, after: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let t = check(s).ok()?;
    let tz = after.timezone();
    let mut d = after.date_naive();
    for _ in 0..800 {
        if fires_on(s, d) {
            let at = (0..24 * 60).find_map(|m| tz.from_local_datetime(&(d.and_time(t) + Days::minutes(m))).earliest())?;
            if at > *after {
                return Some(at);
            }
        }
        d = d.succ_opt()?;
    }
    None
}

// ---- scheduler ----

#[derive(PartialEq, Debug)]
enum Action {
    Run,
    Missed,
    Skipped,
}

/// A due time missed by more than 24h (asleep / app closed) is recorded, not run; one live run at a time.
fn decide(due: i64, now: i64, alive: bool) -> Action {
    if now - due > MISSED_AFTER {
        Action::Missed
    } else if alive {
        Action::Skipped
    } else {
        Action::Run
    }
}

fn file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(settings::dir(app)?.join("automations.json"))
}

/// Mutate under the lock, then persist + emit the full list. Never call into sessions from `f`.
fn update<R>(app: &AppHandle, f: impl FnOnce(&mut Vec<Automation>) -> R) -> R {
    let mut g = app.state::<Automations>().inner().0.lock().unwrap();
    let r = f(&mut g);
    if let Ok(p) = file(app) {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string_pretty(&*g).unwrap_or_default());
    }
    let _ = app.emit("automations-updated", &*g);
    r
}

fn hex() -> String {
    format!("{:08x}", rand::random::<u32>())
}

/// Newest first; returns the dropped runs' session ids.
fn trim(runs: &mut Vec<AutoRun>) -> Vec<String> {
    runs.split_off(KEEP.min(runs.len())).into_iter().filter_map(|r| r.session_id).collect()
}

fn run_record(status: RunStatus, started: i64) -> AutoRun {
    AutoRun { id: hex(), started, status, error: None, session_id: None }
}

/// Start (or record) one run without holding our lock while sessions runs; `due` = Some for scheduled runs.
fn fire(app: &AppHandle, auto_id: &str, due: Option<i64>) -> Result<AutoRun, String> {
    let _g = STARTING.lock().unwrap();
    let a = app.state::<Automations>().0.lock().unwrap().iter().find(|a| a.id == auto_id).cloned().ok_or("unknown automation")?;
    let now = Local::now();
    let alive = a.runs.first().and_then(|r| r.session_id.as_deref()).is_some_and(|s| sessions::pty_alive(app, s));
    let run = match (due, decide(due.unwrap_or(now.timestamp()), now.timestamp(), alive)) {
        (None, Action::Skipped) => return Err("Already running".into()),
        (Some(d), Action::Missed) => run_record(RunStatus::Missed, d),
        (_, Action::Skipped) => run_record(RunStatus::Skipped, now.timestamp()),
        _ => {
            let mut r = run_record(RunStatus::Started, now.timestamp());
            let sid = format!("automation:{}:{}", a.id, r.id);
            match sessions::start_automation_session(app, &sid, a.name.clone(), &a.folder, &a.prompt) {
                Ok(()) => r.session_id = Some(sid),
                Err(e) => (r.status, r.error) = (RunStatus::Failed, Some(e)),
            }
            r
        }
    };
    let rec = run.clone();
    let gone = update(app, |list| {
        let Some(a) = list.iter_mut().find(|a| a.id == auto_id) else { return rec.session_id.into_iter().collect() }; // deleted meanwhile
        if due.is_some() {
            a.next_due = next_after(&a.schedule, &Local::now()).map(|t| t.timestamp());
        }
        a.runs.insert(0, rec);
        trim(&mut a.runs)
    });
    for s in gone {
        sessions::forget(app, &s);
    }
    Ok(run)
}

fn tick(app: &AppHandle) {
    let now = Local::now().timestamp();
    let due: Vec<(String, i64)> = app.state::<Automations>().0.lock().unwrap().iter().filter_map(|a| a.next_due.filter(|&d| a.enabled && d <= now).map(|d| (a.id.clone(), d))).collect();
    for (id, d) in due {
        let _ = fire(app, &id, Some(d));
    }
}

/// Load automations.json and tick every 30s. Wall-clock comparison covers launch and wake: a due time
/// ≤ 24h old runs once, older becomes one `missed` record.
pub fn start(app: AppHandle) {
    let list: Vec<Automation> = file(&app).ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    *app.state::<Automations>().0.lock().unwrap() = list;
    std::thread::spawn(move || loop {
        tick(&app);
        std::thread::sleep(Duration::from_secs(30));
    });
}

// ---- commands ----

fn validate(a: &Automation) -> Result<(), String> {
    if a.name.trim().is_empty() {
        return Err("Name can't be empty".into());
    }
    check(&a.schedule).map(|_| ())
}

#[tauri::command]
pub fn list_automations(state: State<Automations>) -> Vec<Automation> {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_automation(app: AppHandle, mut automation: Automation) -> Result<Automation, String> {
    validate(&automation)?;
    automation.name = automation.name.trim().into();
    automation.next_due = if automation.enabled { next_after(&automation.schedule, &Local::now()).map(|t| t.timestamp()) } else { None };
    update(&app, |list| {
        if automation.id.is_empty() {
            automation.id = hex();
            automation.runs = vec![];
            list.push(automation.clone());
        } else {
            let a = list.iter_mut().find(|a| a.id == automation.id).ok_or("unknown automation")?;
            automation.runs = std::mem::take(&mut a.runs); // ours are authoritative
            *a = automation.clone();
        }
        Ok(automation)
    })
}

#[tauri::command]
pub fn delete_automation(app: AppHandle, id: String) {
    let gone = update(&app, |list| {
        let i = list.iter().position(|a| a.id == id)?;
        Some(list.remove(i).runs)
    });
    for s in gone.into_iter().flatten().filter_map(|r| r.session_id) {
        sessions::forget(&app, &s);
    }
}

#[tauri::command]
pub fn run_automation_now(app: AppHandle, id: String) -> Result<String, String> {
    let r = fire(&app, &id, None)?;
    match r.error {
        Some(e) => Err(e),
        None => Ok(r.id),
    }
}

#[tauri::command]
pub fn delete_run(app: AppHandle, auto_id: String, run_id: String) {
    let sid = update(&app, |list| {
        let runs = &mut list.iter_mut().find(|a| a.id == auto_id)?.runs;
        let i = runs.iter().position(|r| r.id == run_id)?;
        runs.remove(i).session_id
    });
    if let Some(s) = sid {
        sessions::forget(&app, &s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, MappedLocalTime, NaiveDateTime, Offset, Timelike};

    fn sch(kind: Kind, time: &str, days: &[u8], start: &str, dom: u32) -> Schedule {
        Schedule { kind, time: time.into(), days: days.into(), start: start.into(), day_of_month: dom }
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(3600).unwrap().with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn next(s: &Schedule, from: DateTime<FixedOffset>) -> String {
        next_after(s, &from).unwrap().format("%Y-%m-%d %a %H:%M").to_string()
    }

    #[test]
    fn kinds() {
        let daily = sch(Kind::Daily, "09:00", &[], "", 0);
        assert_eq!(next(&daily, at(2026, 10, 13, 8, 0)), "2026-10-13 Tue 09:00");
        assert_eq!(next(&daily, at(2026, 10, 13, 9, 0)), "2026-10-14 Wed 09:00"); // strictly after
        assert_eq!(next(&sch(Kind::Weekdays, "08:30", &[], "", 0), at(2026, 10, 16, 9, 0)), "2026-10-19 Mon 08:30"); // Fri → Mon
        let weekly = sch(Kind::Weekly, "10:00", &[0, 3], "", 0);
        assert_eq!(next(&weekly, at(2026, 10, 13, 12, 0)), "2026-10-14 Wed 10:00");
        assert_eq!(next(&weekly, at(2026, 10, 14, 12, 0)), "2026-10-18 Sun 10:00");
        assert_eq!(next(&sch(Kind::Monthly, "07:00", &[], "", 1), at(2026, 10, 13, 0, 0)), "2026-11-01 Sun 07:00");
    }

    #[test]
    fn biweekly() {
        // start Wed 2026-12-23 (week of Sun 12-20); Mon + Thu; parity crosses the year boundary
        let s = sch(Kind::Biweekly, "09:00", &[1, 4], "2026-12-23", 0);
        assert_eq!(next(&s, at(2026, 12, 1, 0, 0)), "2026-12-24 Thu 09:00"); // never before start (Mon 12-21 is)
        assert_eq!(next(&s, at(2026, 12, 24, 10, 0)), "2027-01-04 Mon 09:00"); // skips the odd week
        assert_eq!(next(&s, at(2027, 1, 4, 10, 0)), "2027-01-07 Thu 09:00");
        assert_eq!(next(&s, at(2027, 1, 8, 0, 0)), "2027-01-18 Mon 09:00");
    }

    #[test]
    fn monthly_clamps() {
        let s = sch(Kind::Monthly, "09:00", &[], "", 31);
        assert_eq!(next(&s, at(2027, 2, 1, 0, 0)), "2027-02-28 Sun 09:00");
        assert_eq!(next(&s, at(2028, 2, 1, 0, 0)), "2028-02-29 Tue 09:00"); // leap
        assert_eq!(next(&s, at(2027, 3, 1, 0, 0)), "2027-03-31 Wed 09:00");
        assert_eq!(next(&s, at(2027, 4, 1, 0, 0)), "2027-04-30 Fri 09:00");
    }

    /// UTC-5, UTC-4 from 2026-03-08 07:00Z to 2026-11-01 06:00Z (US Eastern rules for 2026), so tests don't use the machine zone.
    #[derive(Clone, Copy, Debug)]
    struct Dst;
    fn spring() -> NaiveDateTime { NaiveDate::from_ymd_opt(2026, 3, 8).unwrap().and_hms_opt(7, 0, 0).unwrap() }
    fn fall() -> NaiveDateTime { NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(6, 0, 0).unwrap() }
    impl TimeZone for Dst {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self { Dst }
        fn offset_from_utc_date(&self, d: &NaiveDate) -> FixedOffset { self.offset_from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()) }
        fn offset_from_utc_datetime(&self, u: &NaiveDateTime) -> FixedOffset {
            FixedOffset::west_opt(if *u >= spring() && *u < fall() { 4 } else { 5 } * 3600).unwrap()
        }
        fn offset_from_local_date(&self, d: &NaiveDate) -> MappedLocalTime<FixedOffset> { self.offset_from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()) }
        fn offset_from_local_datetime(&self, l: &NaiveDateTime) -> MappedLocalTime<FixedOffset> {
            // earlier instant first: the -4h reading of a local time happens before the -5h one
            let ok: Vec<FixedOffset> = [4, 5].iter().map(|h| FixedOffset::west_opt(h * 3600).unwrap()).filter(|o| self.offset_from_utc_datetime(&(*l - *o)).fix() == *o).collect();
            match ok[..] {
                [] => MappedLocalTime::None,
                [o] => MappedLocalTime::Single(o),
                [a, b] => MappedLocalTime::Ambiguous(a, b),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn dst() {
        let s = sch(Kind::Daily, "02:30", &[], "", 0);
        let from = Dst.with_ymd_and_hms(2026, 3, 7, 12, 0, 0).unwrap();
        let n = next_after(&s, &from).unwrap(); // 02:30 doesn't exist on 03-08: next valid minute is 03:00 EDT
        assert_eq!((n.day(), n.hour(), n.minute(), n.offset().local_minus_utc()), (8, 3, 0, -4 * 3600));
        let s = sch(Kind::Daily, "01:30", &[], "", 0);
        let from = Dst.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let n = next_after(&s, &from).unwrap(); // 01:30 happens twice on 11-01: first (EDT) one, once
        assert_eq!((n.day(), n.hour(), n.offset().local_minus_utc()), (1, 1, -4 * 3600));
        assert_eq!(next_after(&s, &n).unwrap().day(), 2);
    }

    #[test]
    fn decide_policy() {
        let now = 1_000_000;
        assert_eq!(decide(now - 30, now, false), Action::Run);
        assert_eq!(decide(now - MISSED_AFTER, now, false), Action::Run); // exactly 24h still runs
        assert_eq!(decide(now - MISSED_AFTER - 1, now, false), Action::Missed);
        assert_eq!(decide(now - MISSED_AFTER - 1, now, true), Action::Missed);
        assert_eq!(decide(now - 30, now, true), Action::Skipped);
    }

    #[test]
    fn retention() {
        let mut runs: Vec<AutoRun> = (0..23).map(|i| AutoRun { id: i.to_string(), started: i, status: RunStatus::Started, error: None, session_id: (i != 21).then(|| format!("s{i}")) }).collect();
        assert_eq!(trim(&mut runs), ["s20", "s22"]);
        assert_eq!(runs.len(), 20);
        assert!(trim(&mut runs).is_empty());
    }

    #[test]
    fn validation() {
        let ok = Automation { id: String::new(), name: "Inbox zero".into(), prompt: "p".into(), folder: String::new(), schedule: sch(Kind::Weekly, "09:00", &[1], "", 0), enabled: true, next_due: None, model: String::new(), runs: vec![] };
        assert!(validate(&ok).is_ok());
        let bad = |f: fn(&mut Automation)| {
            let mut a = ok.clone();
            f(&mut a);
            validate(&a).unwrap_err()
        };
        assert_eq!(bad(|a| a.name = "  ".into()), "Name can't be empty");
        assert_eq!(bad(|a| a.schedule.days.clear()), "Pick at least one day");
        assert_eq!(bad(|a| a.schedule.days = vec![7]), "Pick at least one day");
        for t in ["9:00", "24:00", "09:60", "nope"] {
            assert_eq!(bad_time(&ok, t), "Time must be HH:MM", "{t}");
        }
        assert_eq!(bad(|a| a.schedule = sch(Kind::Biweekly, "09:00", &[1], "2026-13-01", 0)), "Pick a start date");
        assert_eq!(bad(|a| a.schedule = sch(Kind::Monthly, "09:00", &[], "", 0)), "Day of month must be 1–31");
        assert_eq!(bad(|a| a.schedule = sch(Kind::Monthly, "09:00", &[], "", 32)), "Day of month must be 1–31");
        assert!(next_after(&sch(Kind::Weekly, "09:00", &[], "", 0), &at(2026, 1, 1, 0, 0)).is_none());
        let json = serde_json::to_value(&ok).unwrap();
        assert!(json["schedule"]["dayOfMonth"].is_number() && json["nextDue"].is_null() && json["schedule"]["kind"] == "weekly");
    }

    fn bad_time(a: &Automation, t: &str) -> String {
        let mut a = a.clone();
        a.schedule.time = t.into();
        validate(&a).unwrap_err()
    }

    #[test]
    fn automation_ids_are_agent_only() {
        assert!(sessions::no_pr_flow("automation:0a1b2c3d:00ff00ff") && sessions::is_automation("automation:x:y"));
    }
}
