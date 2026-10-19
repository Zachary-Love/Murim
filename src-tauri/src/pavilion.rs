//! Pavilion: one board over cycle issues, review PRs and agent sessions. Pure rules + store types first, runtime below.
use crate::github::{Overall, PrHealth, PrState};
use crate::hooks::{PrFlow, Status};
use crate::judge::{By, Q, Verdict};
use crate::linear::Issue;
use crate::sessions::SessionInfo;
use serde::{Deserialize, Serialize};
use crate::settings::{self, AgentKind};
use crate::{judge, sessions};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

pub const RETRY: Duration = Duration::from_secs(600);

/// Board column; declaration order = display order and precedence.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum Column {
    NeedsYou,
    Working,
    Blocked,
    Queued,
    #[default]
    Watching,
    Done,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum InvStatus {
    Running,
    PlanReady,
    Failed,
    Approved,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Investigation {
    pub status: InvStatus,
    pub plan: String,
}

/// What Pavilion knows about an issue that nothing else stores.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Record {
    pub ready: Option<Verdict>,
    pub blocker: Option<Verdict>,
    /// inferred blockers (identifiers) the user dismissed
    pub dismissed: Vec<String>,
    pub investigation: Option<Investigation>,
}

/// ~/.murim/pavilion.json, keyed by Linear issue id.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub items: HashMap<String, Record>,
    pub paused: bool,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    ApprovePlan,
    DismissBlocker,
    StartNow,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    #[default]
    Issue,
    Review,
    Agent,
}

/// Where an issue is in Murim's flow (the inspector's pipeline).
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Triage,
    Investigate,
    Plan,
    Implement,
    Review,
    Merged,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Investigator,
    Coder,
    Reviewer,
    Automation,
    Meditation,
}

#[derive(Serialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    /// issue id, review PR key, or session id
    pub key: String,
    pub column: Column,
    pub ident: String,
    pub title: String,
    pub repo: String,
    pub priority: u8,
    /// why the card is in its column
    pub reason: String,
    /// set when a judgement verdict decided the column
    pub by: Option<By>,
    pub action: Option<Action>,
    /// frontend view + id a click opens: tasks/<issueId>, reviews/<pr key>, automations|meditations/<sessionId>
    pub view: String,
    pub target: String,
    pub pr: Option<PrHealth>,
    /// investigation session shown in Pavilion's side panel
    pub session: Option<String>,
    pub plan: Option<String>,
    pub updated_at: String,
    pub source: Source,
    pub stage: Option<Stage>,
    /// who is (or was last) on it
    pub role: Option<Role>,
    /// first paragraph of the issue description
    pub summary: String,
    /// Linear issue or PR url
    pub url: String,
    /// the judgement behind the column, if any (inspector)
    pub verdict: Option<Verdict>,
}

#[derive(PartialEq, Debug)]
pub struct Decision {
    pub column: Column,
    pub reason: String,
    pub by: Option<By>,
    pub action: Option<Action>,
    /// judgement questions without a fresh verdict
    pub ask: Vec<Q>,
}

pub struct IssueView<'a> {
    pub issue: &'a Issue,
    pub session: Option<&'a SessionInfo>,
    /// the `investigation:<id>` session
    pub inv: Option<&'a SessionInfo>,
    pub pr: Option<&'a PrHealth>,
    pub rec: &'a Record,
    /// identifiers of the issues on the board (valid inferred blockers)
    pub board: &'a HashSet<String>,
}

fn d(column: Column, reason: impl Into<String>) -> Decision {
    Decision { column, reason: reason.into(), by: None, action: None, ask: vec![] }
}

fn live(s: Option<&SessionInfo>) -> bool {
    s.is_some_and(|s| matches!(s.status, Status::Starting | Status::Working))
}

fn asks(s: &SessionInfo) -> String {
    format!("asks: {}", s.questions.as_ref().and_then(|q| q[0]["question"].as_str()).unwrap_or("reply needed"))
}

/// FNV-1a over what the judgement reads; stable across builds (DefaultHasher isn't). ponytail: Linear's updatedAt is
/// assumed to move on comments; if it doesn't, a new comment alone won't trigger a re-judge.
pub fn fingerprint(i: &Issue) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in [i.updated_at.as_str(), &i.title, &i.description].join("\u{1f}").bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn inv_id(issue_id: &str) -> String {
    format!("investigation:{issue_id}")
}

pub fn stage_of(v: &IssueView) -> Stage {
    let inv = v.rec.investigation.as_ref().map(|i| i.status);
    match v.pr {
        Some(p) if p.state == PrState::Merged => Stage::Merged,
        Some(_) => Stage::Review,
        None if v.session.is_some() || inv == Some(InvStatus::Approved) => Stage::Implement,
        None if inv == Some(InvStatus::PlanReady) => Stage::Plan,
        None if inv.is_some() || v.inv.is_some() => Stage::Investigate,
        None => Stage::Triage,
    }
}

pub fn role_of(v: &IssueView) -> Option<Role> {
    if v.session.is_some() {
        Some(Role::Coder)
    } else {
        v.inv.map(|_| Role::Investigator)
    }
}

/// First non-empty paragraph, capped at 280 chars for the inspector.
pub fn summary_of(desc: &str) -> String {
    let p = desc.split("\n\n").map(str::trim).find(|p| !p.is_empty()).unwrap_or("");
    if p.chars().count() > 280 { format!("{}…", p.chars().take(279).collect::<String>()) } else { p.to_string() }
}

/// Column for a cycle issue, in spec precedence: Done > session needs > investigation > working > blocked > verdicts.
pub fn decide(v: &IssueView) -> Decision {
    let fp = fingerprint(v.issue);
    let fresh = |x: &Option<Verdict>| x.clone().filter(|x| x.fingerprint == fp);
    if v.pr.is_some_and(|p| p.state == PrState::Merged) {
        return d(Column::Done, "merged");
    }
    if let Some(s) = v.session {
        if s.status == Status::NeedsInput {
            return d(Column::NeedsYou, asks(s));
        }
        if let Some(e) = s.escalations.first() {
            return d(Column::NeedsYou, format!("Tempering: {}", e.reason));
        }
        if matches!(s.pr_flow, PrFlow::AwaitingReview | PrFlow::Opened) {
            return d(Column::NeedsYou, "draft PR ready for review");
        }
    }
    if let Some(s) = v.inv.filter(|s| s.status == Status::NeedsInput) {
        return d(Column::NeedsYou, format!("investigation {}", asks(s)));
    }
    match v.rec.investigation.as_ref().filter(|_| v.session.is_none()).map(|i| i.status) {
        Some(InvStatus::PlanReady) => return Decision { action: Some(Action::ApprovePlan), ..d(Column::NeedsYou, "plan ready") },
        Some(InvStatus::Failed) => return d(Column::NeedsYou, "investigation ended without a plan"),
        Some(InvStatus::Running) if v.inv.map_or(true, |s| s.status == Status::Ended) => return d(Column::NeedsYou, "investigation ended without a plan"),
        Some(InvStatus::Running) => return d(Column::Working, "investigating"),
        Some(InvStatus::Approved) => return d(Column::Working, "starting agent"),
        _ => {}
    }
    if live(v.session) {
        let ci = v.pr.filter(|p| p.overall == Overall::CiFailing).map(|p| format!("fixing CI: {}", p.failing_checks.join(", ")));
        return d(Column::Working, ci.unwrap_or_else(|| "agent working".into()));
    }
    if let Some(b) = v.issue.blockers.iter().find(|b| !b.done) {
        return d(Column::Blocked, format!("blocked by {}", b.identifier));
    }
    if v.session.is_some() || v.pr.is_some() {
        return d(Column::Watching, if v.pr.is_some() { "PR open" } else { "idle" });
    }
    let ready = fresh(&v.rec.ready);
    if let Some(r) = ready.as_ref().filter(|r| r.answer == "needsUser") {
        return Decision { by: Some(r.by), action: Some(Action::StartNow), ..d(Column::NeedsYou, format!("needs you: {}", r.reason)) };
    }
    let blocker = fresh(&v.rec.blocker);
    if let Some(b) = blocker.as_ref().filter(|b| b.answer != "none" && v.board.contains(&b.answer) && !v.rec.dismissed.contains(&b.answer)) {
        return Decision { by: Some(b.by), action: Some(Action::DismissBlocker), ..d(Column::Blocked, format!("inferred blocker {}: {}", b.answer, b.reason)) };
    }
    let ask: Vec<Q> = [(Q::Ready, ready.is_none()), (Q::Blocker, blocker.is_none())].into_iter().filter(|x| x.1).map(|x| x.0).collect();
    if !ask.is_empty() {
        return Decision { ask, ..d(Column::Watching, "checking…") };
    }
    Decision { by: ready.map(|r| r.by), action: Some(Action::StartNow), ..d(Column::Queued, "ready to investigate") }
}

/// Someone else's PR. Pavilion never starts review agents; it only shows them.
pub fn decide_review(s: Option<&SessionInfo>, draft: bool, requested: bool) -> Decision {
    match s {
        Some(s) if s.status == Status::NeedsInput => d(Column::NeedsYou, format!("review agent {}", asks(s))),
        s if live(s) => d(Column::Working, "review agent running"),
        Some(_) if draft => d(Column::NeedsYou, "review draft ready"),
        _ => d(Column::Watching, if requested { "review requested" } else { "team PR" }),
    }
}

/// Automation run or meditation: on the board only while it works or waits on the user.
pub fn decide_agent(s: &SessionInfo) -> Option<Decision> {
    if s.status == Status::NeedsInput {
        Some(d(Column::NeedsYou, asks(s)))
    } else if live(Some(s)) {
        Some(d(Column::Working, "running"))
    } else {
        None
    }
}

/// Sessions holding an agent slot: only ones actually working (idle PTYs and waiting questions don't).
pub fn working(map: &HashMap<String, SessionInfo>) -> usize {
    map.values().filter(|s| matches!(s.status, Status::Starting | Status::Working)).count()
}

/// Queued cards to start now: free slots, urgent first (Linear 1 urgent … 4 low, 0 none last), then oldest update.
pub fn to_start(cards: &[Card], working: usize, max: usize) -> Vec<&Card> {
    let mut q: Vec<&Card> = cards.iter().filter(|c| c.column == Column::Queued).collect();
    q.sort_by_key(|c| (if c.priority == 0 { 9 } else { c.priority }, c.updated_at.clone()));
    q.truncate(max.saturating_sub(working));
    q
}

/// A failed judgement is retried when the issue changed or RETRY has passed.
pub fn retry_due(prev: Option<&(String, Instant)>, fp: &str, now: Instant) -> bool {
    prev.map_or(true, |(f, at)| f != fp || now.duration_since(*at) >= RETRY)
}

/// An investigation's Stop: a valid `MURIM_REPO:` last line means it finished → (status, plan without the marker, repo).
/// None: no valid marker, so it stopped to ask the user something.
pub fn investigation_outcome(last: &str, root: &Path) -> Option<(InvStatus, String, PathBuf)> {
    let repo = crate::hooks::repo_marker(last, root)?;
    let plan = last.lines().filter(|l| !l.contains("MURIM_REPO:")).collect::<Vec<_>>().join("\n").trim().to_string();
    Some((if plan.is_empty() { InvStatus::Failed } else { InvStatus::PlanReady }, plan, repo))
}

pub fn investigation_prompt(i: &Issue, repos: &[String], guess: Option<&str>) -> String {
    let guess = guess.map(|g| format!("\nLikely repo: {g} (guess)")).unwrap_or_default();
    format!("Investigate this Linear issue and write an implementation plan. You are in the folder that holds all repos. Do NOT edit, create or commit anything; only read code and docs. If something only I can decide is unclear, ask me. Your final message must be the plan itself (markdown: approach, files to change, risks, how to test), ending with the line `MURIM_REPO: <repo-name>` naming the repo the work belongs in (one of the repos below).\n\nRepos: {}{guess}\n\nLinear issue {}: {}\n{}\n\n{}", repos.join(", "), i.identifier, i.title, i.url, i.description)
}

/// Judgement input. ponytail: description capped at 4000 chars; small models lose the thread past that.
pub fn issue_text(i: &Issue, comments: &[crate::linear::Comment]) -> String {
    let c: Vec<String> = comments.iter().map(|c| format!("{}: {}", c.author, c.body)).collect();
    let desc: String = i.description.chars().take(4000).collect();
    format!("{} {}\n\n{desc}\n\nComments:\n{}", i.identifier, i.title, if c.is_empty() { "(none)".into() } else { c.join("\n") })
}

// ---- runtime ----

#[derive(Serialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub cards: Vec<Card>,
    pub working: usize,
    pub max: usize,
    pub paused: bool,
    pub local_offline: bool,
    /// unix secs of the last change
    pub updated: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    store: Store,
    board: Board,
    /// issue ids with a judgement in flight
    inflight: HashSet<String>,
    /// failed judgements: issue id -> (fingerprint, when)
    retry: HashMap<String, (String, Instant)>,
    offline: bool,
}

#[derive(Default)]
pub struct Pavilion {
    data: Mutex<Inner>,
    wake: (Mutex<bool>, Condvar),
}

/// `.inner()` ties the borrow to the app, not to a temporary `State` (same as `sessions::sessions`).
fn pv(app: &AppHandle) -> &Pavilion {
    app.state::<Pavilion>().inner()
}

// ponytail: one judgement at a time; a 4B local model is the bottleneck anyway.
static JUDGE_LOCK: Mutex<()> = Mutex::new(());

fn store_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".murim/pavilion.json"))
}

fn load_store() -> Store {
    store_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Atomic: temp file + rename, so a crash never leaves half a store.
fn save(s: &Store) {
    let Some(p) = store_path() else { return };
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Mutate the store (persisted), then reconcile soon.
fn with_store<R>(app: &AppHandle, f: impl FnOnce(&mut Store) -> R) -> R {
    let r = {
        let mut g = pv(app).data.lock().unwrap();
        let r = f(&mut g.store);
        save(&g.store);
        r
    };
    poke(app);
    r
}

pub fn poke(app: &AppHandle) {
    if let Some(p) = app.try_state::<Pavilion>() {
        *p.wake.0.lock().unwrap() = true;
        p.wake.1.notify_one();
    }
}

/// Load the store before hooks start, so an early hook can't save a default one over it.
pub fn load(app: &AppHandle) {
    pv(app).data.lock().unwrap().store = load_store();
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        reconcile(&app);
        let (m, cv) = &app.state::<Pavilion>().wake;
        drop(cv.wait_timeout_while(m.lock().unwrap(), Duration::from_secs(60), |woken| !*woken).unwrap().0);
        *m.lock().unwrap() = false;
    });
}

fn repo_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Build the board from the polled data, start queued investigations, ask missing judgements.
/// Reads every other module's state before taking the Pavilion lock; calls back into sessions only after releasing it.
fn reconcile(app: &AppHandle) {
    let Ok(dir) = settings::dir(app) else { return };
    let cfg = settings::load(&dir);
    let inbox = crate::inbox::get_inbox(app.state());
    let prs = crate::github::get_prs(app.state()).prs;
    let reviews = crate::reviews::get_reviews(app.state()).prs;
    let map = app.state::<sessions::Sessions>().snapshot();
    let board_ids: HashSet<String> = inbox.items.iter().map(|it| it.issue.identifier.clone()).collect();
    let titles: Vec<(String, String)> = inbox.items.iter().map(|it| (it.issue.identifier.clone(), it.issue.title.clone())).collect();
    let (mut cards, mut asks) = (vec![], vec![]);
    let (start, changed) = {
        let mut g = pv(app).data.lock().unwrap();
        if inbox.error.is_none() && !inbox.items.is_empty() {
            let ids: HashSet<&str> = inbox.items.iter().map(|it| it.issue.id.as_str()).collect();
            let before = g.store.items.len();
            g.store.items.retain(|k, _| ids.contains(k.as_str()));
            if g.store.items.len() != before {
                save(&g.store);
            }
        }
        let empty = Record::default();
        for it in &inbox.items {
            let i = &it.issue;
            let rec = g.store.items.get(&i.id).unwrap_or(&empty);
            let inv = map.get(&inv_id(&i.id));
            let v = IssueView { issue: i, session: map.get(&i.id), inv, pr: prs.get(&i.id), rec, board: &board_ids };
            let dec = decide(&v);
            if !dec.ask.is_empty() {
                asks.push((i.clone(), dec.ask.clone()));
            }
            cards.push(Card {
                key: i.id.clone(), column: dec.column, ident: i.identifier.clone(), title: i.title.clone(), repo: it.repo.as_ref().map(|r| repo_name(&r.path)).unwrap_or_default(),
                priority: i.priority, reason: dec.reason, by: dec.by, action: dec.action, view: "tasks".into(), target: i.id.clone(), pr: prs.get(&i.id).cloned(),
                session: inv.map(|_| inv_id(&i.id)), plan: rec.investigation.as_ref().filter(|x| x.status == InvStatus::PlanReady).map(|x| x.plan.clone()), updated_at: i.updated_at.clone(),
                source: Source::Issue, stage: Some(stage_of(&v)), role: role_of(&v), summary: summary_of(&i.description), url: i.url.clone(),
                verdict: if dec.action == Some(Action::DismissBlocker) { rec.blocker.clone() } else { rec.ready.clone() },
            });
        }
        for p in &reviews {
            let dec = decide_review(map.get(&p.session_id), crate::reviews::has_draft(p), p.requested);
            cards.push(Card { key: p.key.clone(), column: dec.column, ident: format!("{}#{}", p.repo, p.number), title: p.health.title.clone(), repo: p.repo.clone(), reason: dec.reason, view: "reviews".into(), target: p.key.clone(), pr: Some(p.health.clone()), source: Source::Review, stage: Some(Stage::Review), role: map.get(&p.session_id).map(|_| Role::Reviewer), url: p.health.url.clone(), ..Default::default() });
        }
        for (id, s) in &map {
            if !(sessions::is_automation(id) || sessions::is_meditation(id)) {
                continue;
            }
            let Some(dec) = decide_agent(s) else { continue };
            let view = if sessions::is_automation(id) { "automations" } else { "meditations" };
            cards.push(Card { key: id.clone(), column: dec.column, title: s.name.clone(), reason: dec.reason, view: view.into(), target: id.clone(), source: Source::Agent, role: Some(if sessions::is_automation(id) { Role::Automation } else { Role::Meditation }), ..Default::default() });
        }
        cards.sort_by(|a, b| (a.column, a.priority == 0, a.priority, &a.ident).cmp(&(b.column, b.priority == 0, b.priority, &b.ident)));
        let (working, max) = (working(&map), cfg.pavilion.max_agents as usize);
        let auto = !g.store.paused && cfg.pavilion.auto_investigate && !cfg.repos_root.is_empty() && cfg.agent == AgentKind::Claude;
        let start: Vec<String> = if auto { to_start(&cards, working, max).iter().map(|c| c.key.clone()).collect() } else { vec![] };
        let mut board = Board { cards, working, max, paused: g.store.paused, local_offline: g.offline, updated: g.board.updated, error: inbox.error.clone() };
        let changed = board != g.board;
        if changed {
            board.updated = now_secs();
            let _ = app.emit("pavilion-updated", &board);
            g.board = board;
        }
        (start, changed)
    };
    if changed {
        crate::tray::refresh(app, &map); // plans to approve
    }
    for id in start {
        if let Err(e) = start_investigation(app, &id) {
            sessions::notify(app, "Pavilion".into(), format!("Could not start an investigation: {e}"));
        }
    }
    for (issue, ask) in asks {
        spawn_judge(app, issue, ask, titles.clone());
    }
}

fn set_investigation(app: &AppHandle, issue_id: &str, status: InvStatus, plan: String) {
    with_store(app, |s| s.items.entry(issue_id.into()).or_default().investigation = Some(Investigation { status, plan }));
}

fn start_investigation(app: &AppHandle, issue_id: &str) -> Result<(), String> {
    let it = crate::inbox::find(app, issue_id).ok_or("unknown issue")?;
    let cfg = settings::load(&settings::dir(app)?);
    if cfg.repos_root.is_empty() {
        return Err("Choose your repos folder in Settings".into());
    }
    if sessions::pty_alive(app, &inv_id(issue_id)) {
        return Ok(());
    }
    let repos: Vec<String> = crate::repo_match::list_repos(Path::new(&cfg.repos_root)).into_iter().map(|r| r.name).collect();
    let guess = it.repo.as_ref().map(|r| repo_name(&r.path));
    set_investigation(app, issue_id, InvStatus::Running, String::new());
    let r = sessions::start_investigation_session(app, &inv_id(issue_id), format!("Investigate {}", it.issue.identifier), &cfg.repos_root, investigation_prompt(&it.issue, &repos, guess.as_deref()));
    if r.is_err() && !sessions::pty_alive(app, &inv_id(issue_id)) {
        set_investigation(app, issue_id, InvStatus::Failed, String::new());
    }
    r
}

/// Judge one issue's missing questions on a worker thread; verdicts land in the store, then a poke re-evaluates.
fn spawn_judge(app: &AppHandle, issue: Issue, ask: Vec<Q>, board: Vec<(String, String)>) {
    let fp = fingerprint(&issue);
    {
        let mut g = pv(app).data.lock().unwrap();
        if !retry_due(g.retry.get(&issue.id), &fp, Instant::now()) || !g.inflight.insert(issue.id.clone()) {
            return;
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _one = JUDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = settings::dir(&app).map(|d| settings::load(&d).pavilion).unwrap_or_default();
        let comments = crate::linear::api_key().and_then(|k| crate::linear::recent_comments(&k, &issue.id, 10)).unwrap_or_default();
        let text = issue_text(&issue, &comments);
        let ids: Vec<String> = board.iter().map(|b| b.0.clone()).collect();
        let claude = |p: &str| sessions::claude_print(p, Duration::from_secs(90));
        let (mut out, mut local_ok, mut failed) = (vec![], true, false);
        for q in ask {
            let allowed = judge::allowed(q, &ids, &issue.identifier);
            let o = judge::ask(&cfg, q, &judge::prompt(q, &text, &board, &issue.identifier), &allowed, &fp, claude);
            local_ok &= o.local_ok;
            match o.verdict {
                Some(v) => out.push((q, v)),
                None => failed = true,
            }
        }
        {
            let mut g = pv(&app).data.lock().unwrap();
            g.inflight.remove(&issue.id);
            g.offline = !local_ok;
            if failed {
                g.retry.insert(issue.id.clone(), (fp.clone(), Instant::now()));
            } else {
                g.retry.remove(&issue.id);
            }
            let r = g.store.items.entry(issue.id.clone()).or_default();
            for (q, v) in out {
                match q {
                    Q::Ready => r.ready = Some(v),
                    Q::Blocker => r.blocker = Some(v),
                }
            }
            save(&g.store);
        }
        poke(&app);
    });
}

/// Investigation Stop (sessions::on_hook): record the repo and the plan. No valid marker = it asked the user; nothing to do.
pub fn investigation_done(app: &AppHandle, session_id: &str, last: &str) {
    let Some(issue_id) = session_id.strip_prefix("investigation:") else { return };
    // a late Stop after approve must not flip the record back to PlanReady
    if approved_plan(app, issue_id).is_some() {
        return;
    }
    let root = settings::dir(app).map(|d| settings::load(&d).repos_root).unwrap_or_default();
    let Some((status, plan, repo)) = investigation_outcome(last, Path::new(&root)) else { return };
    // a manual pick, so Approve starts straight in the repo's worktree
    let _ = crate::inbox::set_issue_repo(app.clone(), app.state(), issue_id.into(), repo.to_string_lossy().into());
    set_investigation(app, issue_id, status, plan);
    let ident = crate::inbox::find(app, issue_id).map(|i| i.issue.identifier).unwrap_or_default();
    let body = if status == InvStatus::PlanReady { "Plan ready for your approval" } else { "Investigation ended without a plan" };
    sessions::notify(app, format!("Investigate {ident}"), body.into());
}

/// The plan the user approved for this issue, if any; start_task appends it to the issue text.
pub fn approved_plan(app: &AppHandle, issue_id: &str) -> Option<String> {
    let g = pv(app).data.lock().unwrap();
    g.store.items.get(issue_id)?.investigation.as_ref().filter(|i| i.status == InvStatus::Approved).map(|i| i.plan.clone())
}

/// Tray: (issue id, "IDENT  title") for plans waiting on approval.
pub fn plans_ready(app: &AppHandle) -> Vec<(String, String)> {
    let Some(p) = app.try_state::<Pavilion>() else { return vec![] };
    let g = p.data.lock().unwrap();
    g.board.cards.iter().filter(|c| c.action == Some(Action::ApprovePlan)).map(|c| (c.key.clone(), format!("{}  {}", c.ident, c.title))).collect()
}

#[tauri::command]
pub fn get_pavilion(state: State<Pavilion>) -> Board {
    state.data.lock().unwrap().board.clone()
}

#[tauri::command]
pub fn pavilion_approve(app: AppHandle, issue_id: String, plan: String) -> Result<(), String> {
    let plan = plan.trim().to_string();
    if plan.is_empty() {
        return Err("The plan is empty".into());
    }
    if sessions::pty_alive(&app, &issue_id) {
        return Ok(());
    }
    set_investigation(&app, &issue_id, InvStatus::Approved, plan.clone());
    let mode = settings::load(&settings::dir(&app)?).default_permission_mode;
    if let Err(e) = sessions::start_task(app.clone(), issue_id.clone(), mode) {
        set_investigation(&app, &issue_id, InvStatus::PlanReady, plan);
        return Err(e);
    }
    sessions::forget(&app, &inv_id(&issue_id));
    Ok(())
}

#[tauri::command]
pub fn pavilion_dismiss_blocker(app: AppHandle, issue_id: String) {
    with_store(&app, |s| {
        let r = s.items.entry(issue_id).or_default();
        if let Some(b) = r.blocker.as_ref().map(|b| b.answer.clone()) {
            r.dismissed.push(b);
        }
    });
}

/// Investigate now, even over the agent limit.
#[tauri::command]
pub fn pavilion_start_now(app: AppHandle, issue_id: String) -> Result<(), String> {
    if settings::load(&settings::dir(&app)?).agent == AgentKind::Custom {
        return Err("Investigations need Claude Code (Settings → AI)".into());
    }
    start_investigation(&app, &issue_id)
}

#[tauri::command]
pub fn pavilion_set_paused(app: AppHandle, paused: bool) {
    with_store(&app, |s| s.paused = paused);
}

#[tauri::command]
pub async fn pavilion_test_model(url: String, model: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || judge::test_model(&url, &model)).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Ci;
    use crate::linear::Blocker;
    use serde_json::json;

    fn issue(id: &str) -> Issue {
        Issue { id: id.into(), identifier: format!("MUR-{id}"), title: "t".into(), description: "d".into(), url: "u".into(), branch_name: "".into(), state_name: "".into(), state_type: "unstarted".into(), priority: 2, labels: vec![], team_key: "MUR".into(), updated_at: "2026-10-19T00:00:00Z".into(), blockers: vec![] }
    }
    fn sess(id: &str, status: &str) -> SessionInfo {
        serde_json::from_value(json!({"issueId": id, "worktree": "", "sessionId": null, "status": status, "questions": null, "ptyAlive": true, "permissionMode": "normal"})).unwrap()
    }
    fn pr(overall: Overall, state: PrState) -> PrHealth {
        PrHealth { number: 4, url: "u".into(), title: "t".into(), state, is_draft: false, ci: Ci::Failing, failing_checks: vec!["lint".into()], unresolved_threads: 0, changes_requested: false, conflict: false, review_decision: None, overall }
    }
    fn verdict(answer: &str, fp: &str) -> Verdict {
        Verdict { answer: answer.into(), reason: "r".into(), confidence: 0.9, by: By::Claude, fingerprint: fp.into() }
    }
    fn inv(status: InvStatus) -> Record {
        Record { investigation: Some(Investigation { status, plan: "p".into() }), ..Default::default() }
    }
    fn run(i: &Issue, session: Option<&SessionInfo>, inv: Option<&SessionInfo>, pr: Option<&PrHealth>, rec: &Record) -> Decision {
        let board: HashSet<String> = ["MUR-1".to_string(), "MUR-2".to_string()].into();
        decide(&IssueView { issue: i, session, inv, pr, rec, board: &board })
    }
    fn view<'a>(i: &'a Issue, board: &'a HashSet<String>, session: Option<&'a SessionInfo>, inv: Option<&'a SessionInfo>, pr: Option<&'a PrHealth>, rec: &'a Record) -> IssueView<'a> {
        IssueView { issue: i, session, inv, pr, rec, board }
    }
    fn col(d: &Decision) -> (Column, &str) {
        (d.column, d.reason.as_str())
    }

    #[test]
    fn fingerprint_tracks_judged_fields() {
        let a = issue("1");
        assert_eq!(fingerprint(&a), fingerprint(&a.clone()));
        assert_eq!(fingerprint(&a).len(), 16);
        let mut b = a.clone();
        b.description = "edited".into();
        assert_ne!(fingerprint(&a), fingerprint(&b));
        let mut c = a.clone();
        c.priority = 1; // not read by the judgement
        assert_eq!(fingerprint(&a), fingerprint(&c));
    }

    #[test]
    fn session_states_come_first() {
        let i = issue("1");
        let r = Record::default();
        assert_eq!(col(&run(&i, None, None, Some(&pr(Overall::Merged, PrState::Merged)), &r)), (Column::Done, "merged"));
        let mut s = sess("1", "needsInput");
        s.questions = Some(json!([{"question": "Which auth?"}]));
        let mut blocked = issue("1");
        blocked.blockers = vec![Blocker { id: "b".into(), identifier: "MUR-9".into(), done: false }];
        assert_eq!(col(&run(&blocked, Some(&s), None, None, &r)), (Column::NeedsYou, "asks: Which auth?")); // Needs you beats Blocked
        let mut s = sess("1", "idle");
        s.escalations = vec![serde_json::from_value(json!({"id": "e", "threadId": "t", "path": "", "comment": "", "reason": "unclear ask"})).unwrap()];
        assert_eq!(col(&run(&i, Some(&s), None, None, &r)), (Column::NeedsYou, "Tempering: unclear ask"));
        let mut s = sess("1", "idle");
        s.pr_flow = PrFlow::AwaitingReview;
        assert_eq!(col(&run(&i, Some(&s), None, None, &r)).0, Column::NeedsYou);
        let s = sess("1", "working");
        assert_eq!(col(&run(&i, Some(&s), None, Some(&pr(Overall::CiFailing, PrState::Open)), &r)), (Column::Working, "fixing CI: lint"));
        assert_eq!(col(&run(&i, Some(&sess("1", "idle")), None, Some(&pr(Overall::AwaitingReview, PrState::Open)), &r)).0, Column::Watching);
    }

    #[test]
    fn investigation_lifecycle() {
        let i = issue("1");
        let d = run(&i, None, None, None, &inv(InvStatus::PlanReady));
        assert_eq!((col(&d), d.action), ((Column::NeedsYou, "plan ready"), Some(Action::ApprovePlan)));
        assert_eq!(col(&run(&i, None, Some(&sess("investigation:1", "working")), None, &inv(InvStatus::Running))), (Column::Working, "investigating"));
        // crash / restart: the session loads as ended, or is gone
        assert_eq!(col(&run(&i, None, Some(&sess("investigation:1", "ended")), None, &inv(InvStatus::Running))), (Column::NeedsYou, "investigation ended without a plan"));
        assert_eq!(col(&run(&i, None, None, None, &inv(InvStatus::Running))).0, Column::NeedsYou);
        assert_eq!(col(&run(&i, None, None, None, &inv(InvStatus::Failed))), (Column::NeedsYou, "investigation ended without a plan"));
        // stopped to ask the user
        assert_eq!(col(&run(&i, None, Some(&sess("investigation:1", "needsInput")), None, &inv(InvStatus::Running))), (Column::NeedsYou, "investigation asks: reply needed"));
        // approved, coding session not started yet: never re-queued
        assert_eq!(col(&run(&i, None, None, None, &inv(InvStatus::Approved))), (Column::Working, "starting agent"));
    }

    #[test]
    fn leftover_investigation_record_yields_to_a_coding_session() {
        let i = issue("1");
        for st in [InvStatus::Failed, InvStatus::PlanReady, InvStatus::Running] {
            let ended = sess("investigation:1", "ended");
            let d = run(&i, Some(&sess("1", "working")), Some(&ended), None, &inv(st));
            assert_eq!((col(&d), d.action), ((Column::Working, "agent working"), None));
            let d = run(&i, Some(&sess("1", "idle")), Some(&ended), None, &inv(st));
            assert_eq!((col(&d), d.action), ((Column::Watching, "idle"), None));
        }
    }

    #[test]
    fn blockers_and_verdicts() {
        let mut i = issue("1");
        let fp = fingerprint(&i);
        let r = Record::default();
        let d = run(&i, None, None, None, &r);
        assert_eq!((col(&d), d.ask.clone()), ((Column::Watching, "checking…"), vec![Q::Ready, Q::Blocker]));
        let ready = Record { ready: Some(verdict("ready", &fp)), blocker: Some(verdict("none", &fp)), ..Default::default() };
        let d = run(&i, None, None, None, &ready);
        assert_eq!((col(&d), d.action, d.by), ((Column::Queued, "ready to investigate"), Some(Action::StartNow), Some(By::Claude)));
        // edited issue: stale verdicts are asked again
        let mut edited = i.clone();
        edited.description = "new".into();
        assert_eq!(run(&edited, None, None, None, &ready).ask, vec![Q::Ready, Q::Blocker]);
        let needs = Record { ready: Some(verdict("needsUser", &fp)), ..Default::default() };
        let d = run(&i, None, None, None, &needs);
        assert_eq!((col(&d), d.action), ((Column::NeedsYou, "needs you: r"), Some(Action::StartNow)));
        let inferred = Record { ready: Some(verdict("ready", &fp)), blocker: Some(verdict("MUR-2", &fp)), ..Default::default() };
        let d = run(&i, None, None, None, &inferred);
        assert_eq!((col(&d), d.action), ((Column::Blocked, "inferred blocker MUR-2: r"), Some(Action::DismissBlocker)));
        let dismissed = Record { dismissed: vec!["MUR-2".into()], ..inferred.clone() };
        assert_eq!(run(&i, None, None, None, &dismissed).column, Column::Queued);
        let gone = Record { blocker: Some(verdict("MUR-7", &fp)), ..inferred.clone() }; // no longer on the board
        assert_eq!(run(&i, None, None, None, &gone).column, Column::Queued);
        i.blockers = vec![Blocker { id: "b".into(), identifier: "MUR-9".into(), done: false }, Blocker { id: "c".into(), identifier: "MUR-8".into(), done: true }];
        assert_eq!(col(&run(&i, None, None, None, &r)), (Column::Blocked, "blocked by MUR-9"));
        i.blockers[0].done = true;
        assert_ne!(run(&i, None, None, None, &r).column, Column::Blocked);
    }

    #[test]
    fn reviews_and_agents() {
        assert_eq!(col(&decide_review(None, false, true)), (Column::Watching, "review requested"));
        assert_eq!(col(&decide_review(Some(&sess("review:o/r/1", "working")), false, true)), (Column::Working, "review agent running"));
        assert_eq!(col(&decide_review(Some(&sess("review:o/r/1", "idle")), true, false)), (Column::NeedsYou, "review draft ready"));
        assert_eq!(decide_review(Some(&sess("review:o/r/1", "needsInput")), true, false).reason, "review agent asks: reply needed");
        assert_eq!(decide_agent(&sess("automation:a:1", "needsInput")).unwrap().column, Column::NeedsYou);
        assert_eq!(decide_agent(&sess("meditation:1", "working")).unwrap().column, Column::Working);
        assert!(decide_agent(&sess("meditation:1", "idle")).is_none());
    }

    #[test]
    fn queue_fills_free_slots_in_priority_order() {
        let c = |key: &str, priority: u8, at: &str| Card { key: key.into(), column: Column::Queued, priority, updated_at: at.into(), ..Default::default() };
        let cards = vec![c("none", 0, "1"), c("low", 4, "1"), c("urgent-new", 1, "9"), c("urgent-old", 1, "2"), Card { column: Column::Watching, ..c("w", 1, "0") }];
        let keys = |v: Vec<&Card>| v.iter().map(|c| c.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(to_start(&cards, 0, 3)), ["urgent-old", "urgent-new", "low"]);
        assert_eq!(keys(to_start(&cards, 2, 3)), ["urgent-old"]);
        assert!(to_start(&cards, 4, 3).is_empty());
        // only working sessions hold a slot; idle PTYs and questions don't
        let map: HashMap<String, SessionInfo> = [("a", "working"), ("b", "starting"), ("c", "idle"), ("d", "needsInput"), ("e", "ended")].iter().map(|(k, s)| (k.to_string(), sess(k, s))).collect();
        assert_eq!(working(&map), 2);
    }

    #[test]
    fn retry_waits() {
        let t = Instant::now();
        assert!(retry_due(None, "fp", t));
        let prev = ("fp".to_string(), t);
        assert!(!retry_due(Some(&prev), "fp", t + Duration::from_secs(60)));
        assert!(retry_due(Some(&prev), "fp", t + RETRY));
        assert!(retry_due(Some(&prev), "changed", t));
    }

    #[test]
    fn outcome_needs_marker() {
        let root = std::env::temp_dir().join(format!("murim-pv-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/.git")).unwrap();
        let (st, plan, repo) = investigation_outcome("## Plan\n1. do x\n\nMURIM_REPO: app", &root).unwrap();
        assert_eq!((st, plan.as_str(), repo), (InvStatus::PlanReady, "## Plan\n1. do x", root.join("app")));
        assert_eq!(investigation_outcome("MURIM_REPO: app", &root).unwrap().0, InvStatus::Failed); // marker, no plan
        assert!(investigation_outcome("Which auth provider should I plan for?", &root).is_none()); // asking the user
        assert!(investigation_outcome("plan\nMURIM_REPO: nope", &root).is_none()); // not a repo
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn store_defaults_and_round_trip() {
        assert_eq!(serde_json::from_str::<Store>("{}").unwrap(), Store::default());
        let mut s = Store { paused: true, ..Default::default() };
        s.items.insert("1".into(), Record { ready: Some(verdict("ready", "f")), dismissed: vec!["MUR-2".into()], ..inv(InvStatus::PlanReady) });
        assert_eq!(serde_json::from_str::<Store>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
    }

    #[test]
    fn prompts_carry_the_issue() {
        let i = issue("1");
        let p = investigation_prompt(&i, &["app".into(), "web".into()], Some("app"));
        assert!(p.contains("MUR-1") && p.contains("Repos: app, web") && p.contains("Likely repo: app") && p.contains("MURIM_REPO"));
        let c = crate::linear::Comment { author: "Ann".into(), body: "wait for design".into(), created_at: "x".into() };
        assert!(issue_text(&i, &[c]).contains("Ann: wait for design"));
        assert!(issue_text(&i, &[]).contains("(none)"));
    }

    #[test]
    fn stage_and_role_follow_the_flow() {
        let i = issue("1");
        let board: HashSet<String> = HashSet::new();
        let r = Record::default();
        let (s, iv) = (sess("1", "working"), sess("investigation:1", "working"));
        let (open, merged) = (pr(Overall::AwaitingReview, PrState::Open), pr(Overall::Merged, PrState::Merged));
        assert_eq!(stage_of(&view(&i, &board, None, None, None, &r)), Stage::Triage);
        assert_eq!(stage_of(&view(&i, &board, None, Some(&iv), None, &inv(InvStatus::Running))), Stage::Investigate);
        assert_eq!(stage_of(&view(&i, &board, None, None, None, &inv(InvStatus::Failed))), Stage::Investigate);
        assert_eq!(stage_of(&view(&i, &board, None, None, None, &inv(InvStatus::PlanReady))), Stage::Plan);
        assert_eq!(stage_of(&view(&i, &board, None, None, None, &inv(InvStatus::Approved))), Stage::Implement);
        assert_eq!(stage_of(&view(&i, &board, Some(&s), None, None, &r)), Stage::Implement);
        assert_eq!(stage_of(&view(&i, &board, Some(&s), None, Some(&open), &r)), Stage::Review);
        assert_eq!(stage_of(&view(&i, &board, None, None, Some(&merged), &r)), Stage::Merged);
        assert_eq!(role_of(&view(&i, &board, Some(&s), Some(&iv), None, &r)), Some(Role::Coder));
        assert_eq!(role_of(&view(&i, &board, None, Some(&iv), None, &r)), Some(Role::Investigator));
        assert_eq!(role_of(&view(&i, &board, None, None, None, &r)), None);
    }

    #[test]
    fn summary_is_first_paragraph_capped() {
        assert_eq!(summary_of("\n\nFirst para\nstill first.\n\nSecond"), "First para\nstill first.");
        assert_eq!(summary_of(""), "");
        let long = "x".repeat(300);
        let s = summary_of(&long);
        assert_eq!((s.chars().count(), s.ends_with('…')), (280, true));
    }
}
