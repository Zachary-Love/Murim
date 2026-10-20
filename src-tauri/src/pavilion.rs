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
/// reconcile loop period (a poke runs it sooner)
pub const TICK: Duration = Duration::from_secs(60);

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
    /// the user moved the card by hand
    pub manual: Option<Manual>,
}

/// A hand-placed column, kept while Pavilion's own decision is still `from` (see `signature`); a new situation
/// (PR merged, CI failed, agent asks) ends it, so a manual move can't hide something new.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Manual {
    pub column: Column,
    pub from: String,
}

/// ~/.murim/pavilion.json, keyed by Linear issue id.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub items: HashMap<String, Record>,
    pub paused: bool,
    /// hand moves of review PR, adopted PR and automation cards (issue cards keep theirs in `items`).
    /// ponytail: entries of cards that left the board linger until their signature goes stale; a few bytes each
    pub moves: HashMap<String, Manual>,
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
}

/// What the local model (or Claude checking it) made of an issue: the card's robot pill.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Pick {
    /// judgement running now or waiting its turn
    Thinking,
    /// no verdict yet; asked on the next tick
    Pending,
    /// the last judgement failed; asked again when the issue changes or RETRY passes
    Retrying,
    Queued,
    NeedsYou,
    Blocked,
    // the orchestrator's plan for every other card
    /// queued and taking a free slot now
    Starting,
    /// queued behind the agent limit
    WaitingSlot,
    /// queued but auto-start is off (paused, or a setting)
    AutoOff,
    Working,
    Watching,
    Done,
    Moved,
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
    pub pick: Option<Pick>,
    /// the user placed this card (Move to → Automatic undoes it)
    pub manual: bool,
    /// `signature` of the automatic decision, for pavilion_move
    #[serde(skip)]
    pub auto: String,
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
        // escalations first: they set needsInput too, and say why
        if let Some(e) = s.escalations.first() {
            return d(Column::NeedsYou, if e.ask { format!("review needs your call: {}", e.reason) } else { format!("Tempering: {}", e.reason) });
        }
        if s.status == Status::NeedsInput {
            return d(Column::NeedsYou, asks(s));
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
        let s = v.session.unwrap();
        if s.tempering_turn && !s.tempering_note.is_empty() {
            return d(Column::Working, s.tempering_note.clone());
        }
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

/// What a manual move is pinned to: the automatic column and reason.
pub fn signature(d: &Decision) -> String {
    format!("{:?}: {}", d.column, d.reason)
}

/// The user's column while the automatic decision is unchanged; None = no move, or it no longer applies.
pub fn manual_decision(auto: &Decision, m: Option<&Manual>) -> Option<Decision> {
    let m = m.filter(|m| m.from == signature(auto))?;
    Some(Decision { action: (m.column == Column::Queued).then_some(Action::StartNow), ..d(m.column, format!("moved by you · was {}", auto.reason)) })
}

/// The robot pill: only for cards a judgement decided (or is deciding); session, PR and Linear-blocker cards get none.
pub fn pick_of(dec: &Decision, inflight: bool, retrying: bool) -> Option<Pick> {
    if !dec.ask.is_empty() {
        return Some(if inflight { Pick::Thinking } else if retrying { Pick::Retrying } else { Pick::Pending });
    }
    dec.by?;
    match dec.column {
        Column::Queued => Some(Pick::Queued),
        Column::NeedsYou => Some(Pick::NeedsYou),
        Column::Blocked => Some(Pick::Blocked),
        _ => None,
    }
}

/// The pill for every card but review PRs (those belong to someone else): the queue's plan beats the judgement it
/// came from, a hand move beats the rest.
pub fn plan_of(c: &Card, judged: Option<Pick>, auto_off: bool, starting: bool) -> Option<Pick> {
    if c.source == Source::Review {
        return judged;
    }
    Some(match c.column {
        Column::Queued => if starting { Pick::Starting } else if auto_off { Pick::AutoOff } else { Pick::WaitingSlot },
        _ if c.manual => Pick::Moved,
        _ if judged.is_some() => return judged,
        Column::Working => Pick::Working,
        Column::NeedsYou => Pick::NeedsYou,
        Column::Blocked => Pick::Blocked,
        Column::Watching => Pick::Watching,
        Column::Done => Pick::Done,
    })
}

/// Log a line once per app run (repeated failures and holds that every poll would otherwise re-log).
pub fn olog_once(msg: String) {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    if SEEN.lock().unwrap().get_or_insert_with(HashSet::new).insert(msg.clone()) {
        olog(msg);
    }
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

/// Sessions holding an agent slot: only ones actually working (idle PTYs and waiting questions don't); meditations are the user's own and never count.
pub fn working(map: &HashMap<String, SessionInfo>) -> usize {
    map.iter().filter(|(id, s)| !sessions::is_meditation(id) && matches!(s.status, Status::Starting | Status::Working)).count()
}

/// Queued cards to start now: free slots, urgent first (Linear 1 urgent … 4 low, 0 none last), then oldest update.
pub fn to_start(cards: &[Card], working: usize, max: usize) -> Vec<&Card> {
    let mut q: Vec<&Card> = cards.iter().filter(|c| c.column == Column::Queued && c.source == Source::Issue).collect();
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
    /// the saved local model (Settings → Pavilion)
    pub model: String,
    /// identifier of the issue being judged right now
    pub thinking: Option<String>,
    /// judgements running or waiting their turn
    pub judging: usize,
    /// unix secs of the next reconcile tick
    pub next_run: u64,
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
    /// identifier holding JUDGE_LOCK
    thinking: Option<String>,
    /// last board summary logged; logged again only when it changes
    summary: String,
    /// unix secs of the next scheduled tick (pokes run sooner without moving it)
    next_run: u64,
    /// issue id -> "Column: reason" last logged
    logged: HashMap<String, String>,
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

/// ~/.murim/orchestrator.log: what Pavilion decided and why (Settings → Orchestrator Log). Rotates to .log.1 past 512 KB.
pub fn olog(msg: impl AsRef<str>) {
    use std::io::Write;
    let Some(p) = store_path().map(|p| p.with_file_name("orchestrator.log")) else { return };
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > 512 * 1024) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg.as_ref());
    }
}

/// The last `n` log lines, newest last.
pub fn log_tail(text: &str, n: usize) -> String {
    let l: Vec<&str> = text.lines().collect();
    l[l.len().saturating_sub(n)..].join("\n")
}

/// Why Pavilion won't start investigations on its own, if it won't.
pub fn auto_off(paused: bool, cfg: &settings::Settings) -> Option<&'static str> {
    if paused {
        Some("paused (Resume agents on the Pavilion page)")
    } else if !cfg.pavilion.auto_investigate {
        Some("auto-investigate is off (Settings → Pavilion)")
    } else if cfg.repos_root.is_empty() {
        Some("no repos folder (Settings → Folders)")
    } else if cfg.agent != AgentKind::Claude {
        Some("agent isn't Claude Code (Settings → AI)")
    } else {
        None
    }
}

fn memory_path() -> Option<PathBuf> {
    store_path().map(|p| p.with_file_name("pavilion-memory.md"))
}

/// ~/.murim/pavilion-memory.md: what the orchestrator learned from being corrected (≤ judge::MEMORY_MAX chars).
pub fn memory() -> String {
    memory_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

/// Judgement prompts start with the memory.
pub fn with_memory(prompt: &str) -> String {
    match memory().trim() {
        "" => prompt.to_string(),
        m => format!("What you have learned about this user so far:\n{m}\n\n{prompt}"),
    }
}

/// A correction to learn from (user override, or Claude overruling the local model): rewrite the memory on a worker,
/// one model call at a time with judgements.
pub fn learn(app: &AppHandle, event: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _one = JUDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = settings::dir(&app).map(|d| settings::load(&d).pavilion).unwrap_or_default();
        olog(format!("learning from: {event}"));
        let old = memory();
        match judge::rewrite_memory(&cfg, &old, &event, |p| sessions::claude_print(p, Duration::from_secs(90))) {
            Ok(m) if m.trim() == old.trim() => olog("  memory unchanged"),
            Ok(m) => match memory_path().map(|p| std::fs::write(p, &m)) {
                Some(Ok(())) => olog(format!("  memory updated ({}/{} chars)", m.chars().count(), judge::MEMORY_MAX)),
                _ => olog("  memory not saved: can't write ~/.murim/pavilion-memory.md"),
            },
            Err(e) => olog(format!("  memory not updated: {e}")),
        }
    });
}

fn card(app: &AppHandle, key: &str) -> Option<Card> {
    pv(app).data.lock().unwrap().board.cards.iter().find(|c| c.key == key).cloned()
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

pub fn is_paused(app: &AppHandle) -> bool {
    pv(app).data.lock().unwrap().store.paused
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
    std::thread::spawn(move || {
        let mut due = Instant::now();
        loop {
            if Instant::now() >= due {
                due = Instant::now() + TICK;
                pv(&app).data.lock().unwrap().next_run = now_secs() + TICK.as_secs();
            }
            heal(&app);
            reconcile(&app);
            let (m, cv) = &app.state::<Pavilion>().wake;
            drop(cv.wait_timeout_while(m.lock().unwrap(), due.saturating_duration_since(Instant::now()), |woken| !*woken).unwrap().0);
            *m.lock().unwrap() = false;
        }
    });
}

/// "local model offline" otherwise only clears on the next judgement, and unchanged issues aren't re-judged:
/// while offline, test the model in the background (at most one probe at a time, each loop tick) and clear it.
fn heal(app: &AppHandle) {
    static PROBING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !pv(app).data.lock().unwrap().offline || PROBING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let cfg = settings::dir(&app).map(|d| settings::load(&d).pavilion).unwrap_or_default();
        if !cfg.local_model || judge::test_model(&cfg.ollama_url, &cfg.model).is_ok() {
            olog(format!("{} answers again", judge::model_name(&cfg)));
            pv(&app).data.lock().unwrap().offline = false;
            poke(&app);
        }
        PROBING.store(false, std::sync::atomic::Ordering::SeqCst);
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
        let mut stale = vec![];
        for it in &inbox.items {
            let i = &it.issue;
            let rec = g.store.items.get(&i.id).unwrap_or(&empty);
            let inv = map.get(&inv_id(&i.id));
            let v = IssueView { issue: i, session: map.get(&i.id), inv, pr: prs.get(&i.id), rec, board: &board_ids };
            let auto = decide(&v);
            let sig = signature(&auto);
            let moved = manual_decision(&auto, rec.manual.as_ref());
            if rec.manual.is_some() && moved.is_none() {
                stale.push(i.id.clone());
            }
            let manual = moved.is_some();
            let dec = moved.unwrap_or(auto); // a moved card asks no judgement: a verdict landing would undo the move
            let pick = pick_of(&dec, g.inflight.contains(&i.id), g.retry.contains_key(&i.id));
            if !dec.ask.is_empty() {
                asks.push((i.clone(), dec.ask.clone()));
            }
            cards.push(Card {
                key: i.id.clone(), column: dec.column, ident: i.identifier.clone(), title: i.title.clone(), repo: it.repo.as_ref().map(|r| repo_name(&r.path)).unwrap_or_default(),
                priority: i.priority, reason: dec.reason, by: dec.by, action: dec.action, view: "tasks".into(), target: i.id.clone(), pr: prs.get(&i.id).cloned(),
                session: if v.session.is_some() { Some(i.id.clone()) } else { inv.map(|_| inv_id(&i.id)) }, plan: rec.investigation.as_ref().filter(|x| x.status == InvStatus::PlanReady).map(|x| x.plan.clone()), updated_at: i.updated_at.clone(),
                source: Source::Issue, stage: Some(stage_of(&v)), role: role_of(&v), summary: summary_of(&i.description), url: i.url.clone(),
                verdict: dec.by.and(if dec.action == Some(Action::DismissBlocker) { rec.blocker.clone() } else { rec.ready.clone() }),
                pick, manual, auto: sig,
            });
        }
        // one log line per issue whenever its column or reason changes: why nothing starts is in the log
        for c in cards.iter().filter(|c| c.source == Source::Issue) {
            let line = format!("{:?}: {}", c.column, c.reason);
            if g.logged.get(&c.key) != Some(&line) {
                olog(format!("  {} {line}{}", c.ident, c.by.map_or(String::new(), |b| format!(" (decided by {b:?})"))));
                g.logged.insert(c.key.clone(), line);
            }
        }
        for id in stale.drain(..) {
            if let Some(r) = g.store.items.get_mut(&id) {
                olog(format!("manual move on {id} ended: Pavilion's decision changed"));
                r.manual = None;
            }
            save(&g.store);
        }
        for p in &reviews {
            let dec = decide_review(map.get(&p.session_id), crate::reviews::has_draft(p), p.requested);
            cards.push(Card { key: p.key.clone(), column: dec.column, ident: format!("{}#{}", p.repo, p.number), title: p.health.title.clone(), repo: p.repo.clone(), reason: dec.reason, view: "reviews".into(), target: p.key.clone(), pr: Some(p.health.clone()), source: Source::Review, stage: Some(Stage::Review), role: map.get(&p.session_id).map(|_| Role::Reviewer), session: map.contains_key(&p.session_id).then(|| p.session_id.clone()), url: p.health.url.clone(), ..Default::default() });
        }
        for (id, s) in &map {
            if sessions::is_adopted(id) {
                // one of the user's PRs Pavilion picked up: on the board while open, Tempering keeps it moving
                let Some(p) = prs.get(id).filter(|p| p.state == crate::github::PrState::Open) else { continue };
                let dec = decide_agent(s).unwrap_or_else(|| d(Column::Watching, "Tempering watches this PR"));
                let reason = if s.tempering_turn && !s.tempering_note.is_empty() && dec.column == Column::Working { s.tempering_note.clone() } else { dec.reason };
                let repo = id.trim_start_matches("pr:").split('/').nth(1).unwrap_or_default().to_string();
                cards.push(Card { key: id.clone(), column: dec.column, ident: format!("{repo}#{}", p.number), title: p.title.clone(), repo, reason, view: "pavilion".into(), target: id.clone(), pr: Some(p.clone()), session: Some(id.clone()), source: Source::Agent, role: Some(Role::Coder), url: p.url.clone(), ..Default::default() });
                continue;
            }
            if !sessions::is_automation(id) {
                continue;
            }
            let Some(dec) = decide_agent(s) else { continue };
            cards.push(Card { key: id.clone(), column: dec.column, title: s.name.clone(), reason: dec.reason, view: "automations".into(), target: id.clone(), session: Some(id.clone()), source: Source::Agent, role: Some(Role::Automation), ..Default::default() });
        }
        // hand moves on the other cards: same rule as issues, kept while the automatic decision is unchanged
        let mut stale_moves = vec![];
        for c in cards.iter_mut().filter(|c| c.source != Source::Issue) {
            c.auto = format!("{:?}: {}", c.column, c.reason); // == signature() of the decision that built it
            match g.store.moves.get(&c.key) {
                Some(m) if m.from == c.auto => (c.column, c.reason, c.manual) = (m.column, format!("moved by you · was {}", c.reason), true),
                Some(_) => stale_moves.push(c.key.clone()),
                None => {}
            }
        }
        if !stale_moves.is_empty() {
            stale_moves.iter().for_each(|k| drop(g.store.moves.remove(k)));
            save(&g.store);
        }
        cards.sort_by(|a, b| (a.column, a.priority == 0, a.priority, &a.ident).cmp(&(b.column, b.priority == 0, b.priority, &b.ident)));
        let (working, max) = (working(&map), cfg.pavilion.max_agents as usize);
        let off = auto_off(g.store.paused, &cfg);
        let start: Vec<String> = if off.is_none() { to_start(&cards, working, max).iter().map(|c| c.key.clone()).collect() } else { vec![] };
        for c in cards.iter_mut() {
            c.pick = plan_of(c, c.pick, off.is_some(), start.contains(&c.key));
        }
        let n = |c: Column| cards.iter().filter(|x| x.source == Source::Issue && x.column == c).count();
        let summary = match &inbox.error {
            Some(e) => format!("Linear error: {e}"),
            None => format!(
                "board: {} cycle issues ({} need judging, {} queued, {} blocked, {} need you, {} working); agents {working}/{max}; model {}{}; auto-start {}",
                inbox.items.len(), asks.len(), n(Column::Queued), n(Column::Blocked), n(Column::NeedsYou), n(Column::Working), judge::model_name(&cfg.pavilion),
                if g.offline { " (offline)" } else { "" }, off.map_or("on".into(), |o| format!("off: {o}")),
            ),
        };
        if summary != g.summary {
            olog(&summary);
            g.summary = summary;
        }
        let (thinking, judging) = (g.thinking.clone(), g.inflight.len());
        let mut board = Board { cards, working, max, paused: g.store.paused, local_offline: g.offline && cfg.pavilion.local_model, updated: g.board.updated, error: inbox.error.clone(), model: judge::model_name(&cfg.pavilion), thinking, judging, next_run: g.board.next_run };
        let changed = board != g.board;
        // `updated` only moves on a real change, not when the countdown rolls over
        board.next_run = g.next_run;
        if changed {
            board.updated = now_secs();
        }
        if changed || board.next_run != g.board.next_run {
            let _ = app.emit("pavilion-updated", &board);
        }
        g.board = board;
        (start, changed)
    };
    if changed {
        crate::tray::refresh(app, &map); // plans to approve
    }
    for id in start {
        olog(format!("starting investigation for {}", crate::inbox::find(app, &id).map(|i| i.issue.identifier).unwrap_or(id.clone())));
        if let Err(e) = start_investigation(app, &id) {
            olog(format!("could not start investigation: {e}"));
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
        pv(&app).data.lock().unwrap().thinking = Some(issue.identifier.clone());
        poke(&app); // header shows what the model is on
        let cfg = settings::dir(&app).map(|d| settings::load(&d).pavilion).unwrap_or_default();
        let comments = crate::linear::api_key().and_then(|k| crate::linear::recent_comments(&k, &issue.id, 10)).unwrap_or_default();
        let text = issue_text(&issue, &comments);
        let ids: Vec<String> = board.iter().map(|b| b.0.clone()).collect();
        olog(format!("judging {} ({:?}) with {}", issue.identifier, ask, judge::model_name(&cfg)));
        let claude = |p: &str| sessions::claude_print(p, Duration::from_secs(90));
        let (mut out, mut local_ok, mut failed) = (vec![], true, false);
        for q in ask {
            let allowed = judge::allowed(q, &ids, &issue.identifier);
            let o = judge::ask(&cfg, q, &with_memory(&judge::prompt(q, &text, &board, &issue.identifier)), &allowed, &fp, claude);
            if let (Some((a, r)), Some(v)) = (&o.local, o.verdict.as_ref().filter(|v| v.by == By::Claude)) {
                if *a != v.answer {
                    learn(&app, format!("Asked {q:?} about {} \"{}\", you answered {a} ({r}); Claude overruled you with {}: {}", issue.identifier, issue.title, v.answer, v.reason));
                }
            }
            local_ok &= o.local_ok;
            for l in &o.log {
                olog(format!("  {} {q:?}: {l}", issue.identifier));
            }
            match o.verdict {
                Some(v) => out.push((q, v)),
                None => failed = true,
            }
        }
        {
            let mut g = pv(&app).data.lock().unwrap();
            g.inflight.remove(&issue.id);
            g.thinking = None;
            g.offline = !local_ok;
            if failed {
                olog(format!("  {} no verdict; retrying when it changes or in 10 min", issue.identifier));
                g.retry.insert(issue.id.clone(), (fp.clone(), Instant::now()));
            } else {
                g.retry.remove(&issue.id);
            }
            let r = g.store.items.entry(issue.id.clone()).or_default();
            for (q, v) in out {
                match q {
                    Q::Ready => r.ready = Some(v),
                    Q::Blocker => r.blocker = Some(v),
                    Q::Feedback => {} // asked by tempering::triage, never from here
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
pub fn get_pavilion_memory() -> String {
    memory()
}

/// Settings → Orchestrator Log: the user edits the memory directly.
#[tauri::command]
pub fn set_pavilion_memory(text: String) -> Result<(), String> {
    let n = text.trim().chars().count();
    if n > judge::MEMORY_MAX {
        return Err(format!("{n} characters; the memory holds at most {}", judge::MEMORY_MAX));
    }
    let p = memory_path().ok_or("no home folder")?;
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    std::fs::write(p, text.trim()).map_err(|e| e.to_string())?;
    olog(format!("memory edited by the user ({n}/{} chars)", judge::MEMORY_MAX));
    Ok(())
}

#[tauri::command]
pub fn orchestrator_log() -> String {
    store_path().and_then(|p| std::fs::read_to_string(p.with_file_name("orchestrator.log")).ok()).map(|t| log_tail(&t, 500)).unwrap_or_default()
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

/// Move any card by hand; None = back to automatic. Only issues can be queued (the queue starts investigations).
#[tauri::command]
pub fn pavilion_move(app: AppHandle, key: String, column: Option<Column>) -> Result<(), String> {
    let c = card(&app, &key).ok_or("That card is no longer on the board")?;
    if column == Some(Column::Queued) && c.source != Source::Issue {
        return Err("Only Linear issues can be queued".into());
    }
    if let Some(col) = column {
        learn(&app, format!("The user moved {} \"{}\" by hand to {col:?}; Pavilion had decided {}", c.ident, c.title, c.auto));
    }
    let m = column.map(|column| Manual { column, from: c.auto.clone() });
    with_store(&app, |s| match c.source {
        Source::Issue => s.items.entry(key).or_default().manual = m,
        _ => drop(match m {
            Some(m) => s.moves.insert(key, m),
            None => s.moves.remove(&key),
        }),
    });
    Ok(())
}

#[tauri::command]
pub fn pavilion_dismiss_blocker(app: AppHandle, issue_id: String) {
    if let Some(c) = card(&app, &issue_id) {
        learn(&app, format!("The user dismissed the blocker you inferred on {} \"{}\": {}", c.ident, c.title, c.reason));
    }
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
    if let Some(c) = card(&app, &issue_id).filter(|c| c.column == Column::NeedsYou && c.verdict.is_some()) {
        learn(&app, format!("The user started {} \"{}\" although it was judged to need them first: {}", c.ident, c.title, c.reason));
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

#[tauri::command]
pub async fn ollama_models(url: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || judge::models(&url)).await.map_err(|e| e.to_string())?
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
        let mut s = sess("1", "needsInput");
        s.escalations = vec![serde_json::from_value(json!({"id": "e", "threadId": "", "path": "review", "comment": "", "reason": "API naming is a product call", "ask": true})).unwrap()];
        assert_eq!(col(&run(&i, Some(&s), None, None, &r)), (Column::NeedsYou, "review needs your call: API naming is a product call"));
        let mut s = sess("1", "working");
        (s.tempering_turn, s.tempering_note) = (true, "fixing 2 review comments".into());
        assert_eq!(col(&run(&i, Some(&s), None, Some(&pr(Overall::CiFailing, PrState::Open)), &r)), (Column::Working, "fixing 2 review comments"));
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
        let map: HashMap<String, SessionInfo> = [("a", "working"), ("b", "starting"), ("c", "idle"), ("d", "needsInput"), ("e", "ended"), ("meditation:1", "working")].iter().map(|(k, s)| (k.to_string(), sess(k, s))).collect();
        assert_eq!(working(&map), 2);
    }

    #[test]
    fn pick_follows_the_judgement() {
        let ask = Decision { ask: vec![Q::Ready], ..d(Column::Watching, "checking…") };
        assert_eq!([pick_of(&ask, true, true), pick_of(&ask, false, true), pick_of(&ask, false, false)], [Some(Pick::Thinking), Some(Pick::Retrying), Some(Pick::Pending)]);
        let by = |c| Decision { by: Some(By::Local), ..d(c, "") };
        assert_eq!([pick_of(&by(Column::Queued), false, false), pick_of(&by(Column::NeedsYou), false, false), pick_of(&by(Column::Blocked), false, false)], [Some(Pick::Queued), Some(Pick::NeedsYou), Some(Pick::Blocked)]);
        assert_eq!(pick_of(&d(Column::Blocked, "blocked by MUR-2"), false, false), None); // Linear relation, not the model
    }

    #[test]
    fn log_tail_and_auto_off() {
        assert_eq!(log_tail("a\nb\nc", 2), "b\nc");
        assert_eq!(log_tail("a", 5), "a");
        let mut s = settings::Settings { repos_root: "/r".into(), ..Default::default() };
        assert_eq!(auto_off(false, &s), None);
        assert!(auto_off(true, &s).unwrap().starts_with("paused"));
        s.repos_root.clear();
        assert!(auto_off(false, &s).unwrap().contains("repos folder"));
    }

    #[test]
    fn manual_move_lasts_until_the_situation_changes() {
        let auto = d(Column::NeedsYou, "draft PR ready for review");
        let m = Manual { column: Column::Watching, from: signature(&auto) };
        let got = manual_decision(&auto, Some(&m)).unwrap();
        assert_eq!((got.column, got.reason.as_str(), got.ask.is_empty()), (Column::Watching, "moved by you · was draft PR ready for review", true));
        assert_eq!(manual_decision(&d(Column::Working, "fixing CI: build"), Some(&m)), None); // new situation: automatic again
        assert_eq!(manual_decision(&auto, None), None);
        assert_eq!(manual_decision(&auto, Some(&Manual { column: Column::Queued, ..m })).unwrap().action, Some(Action::StartNow));
    }

    #[test]
    fn every_card_gets_a_plan() {
        let c = |column, source| Card { column, source, ..Default::default() };
        assert_eq!(plan_of(&c(Column::Queued, Source::Issue), Some(Pick::Queued), false, true), Some(Pick::Starting));
        assert_eq!(plan_of(&c(Column::Queued, Source::Issue), Some(Pick::Queued), false, false), Some(Pick::WaitingSlot));
        assert_eq!(plan_of(&c(Column::Queued, Source::Issue), None, true, false), Some(Pick::AutoOff));
        assert_eq!(plan_of(&c(Column::Watching, Source::Issue), Some(Pick::Thinking), false, false), Some(Pick::Thinking));
        assert_eq!(plan_of(&c(Column::Watching, Source::Agent), None, false, false), Some(Pick::Watching));
        assert_eq!(plan_of(&Card { manual: true, ..c(Column::Done, Source::Issue) }, None, false, false), Some(Pick::Moved));
        assert_eq!(plan_of(&c(Column::Watching, Source::Review), None, false, false), None);
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
