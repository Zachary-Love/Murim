//! Pavilion: one board over cycle issues, review PRs and agent sessions. Pure rules + store types first, runtime below.
use crate::github::{Overall, PrHealth, PrState};
use crate::hooks::{PrFlow, Status};
use crate::judge::{By, Q, Verdict};
use crate::linear::Issue;
use crate::sessions::SessionInfo;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    match v.rec.investigation.as_ref().map(|i| i.status) {
        Some(InvStatus::PlanReady) => return Decision { action: Some(Action::ApprovePlan), ..d(Column::NeedsYou, "plan ready") },
        Some(InvStatus::Failed) => return d(Column::NeedsYou, "investigation ended without a plan"),
        Some(InvStatus::Running) if v.inv.map_or(true, |s| s.status == Status::Ended) => return d(Column::NeedsYou, "investigation ended without a plan"),
        Some(InvStatus::Running) => return d(Column::Working, "investigating"),
        Some(InvStatus::Approved) if v.session.is_none() => return d(Column::Working, "starting agent"),
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
}
