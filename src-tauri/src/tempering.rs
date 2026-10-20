//! Tempering: pure decision rules over a richer PR fetch + the runner that executes them.
use crate::github::{self, Ci, Overall, PrHealth, PrState};
use crate::hooks::Status;
use crate::sessions;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use crate::judge::{self, Q};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

const MAX_INVOKES: u32 = 3;
const LOG_LINES: usize = 80;
/// after a Tempering turn dies on an API error (rate limit, overloaded…), wait this long before retrying
const RETRY_SECS: u64 = 600;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReplyMode {
    Off,
    #[default]
    Terse,
    Full,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Escalation {
    pub id: String,
    pub thread_id: String,
    pub path: String,
    pub comment: String,
    pub reason: String,
    /// Pavilion's triage: only the user can answer this one (vs. the agent disagreeing)
    #[serde(default)]
    pub ask: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Ci,
    Thread,
    Conflict,
    /// a "changes requested" review's summary (no inline thread)
    Review,
}

/// Persisted per issue in sessions.json.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Memory {
    /// `ci:<sha>:<check>`, `thread:<id>:<last comment id>`, `conflict:<sha>`, `review:<id>`
    pub handled: HashSet<String>,
    pub attempts: HashMap<Kind, u32>,
    /// `<sha>:<check>` already re-run once
    pub rerun: HashSet<String>,
    pub escalated: HashSet<Kind>,
    pub ready: bool,
    /// what the running Tempering turn is fixing, so a turn that dies on an API error can be retried
    pub inflight: Vec<(Kind, Vec<String>)>,
    /// unix secs: no new Tempering turns before this
    pub retry_at: u64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Thread {
    pub id: String,
    pub path: String,
    pub line: Option<u64>,
    pub comments: Vec<Comment>,
}

/// Failing check: name + GitHub Actions workflow run id (None for non-Actions checks).
#[derive(Clone, PartialEq, Debug)]
pub struct FailCheck {
    pub name: String,
    pub run_id: Option<u64>,
}

/// A "changes requested" review by someone else, with a summary body.
#[derive(Clone, PartialEq, Debug)]
pub struct Review {
    pub id: String,
    pub author: String,
    pub body: String,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct PrDetail {
    pub sha: String,
    pub base: String,
    pub checks: Vec<FailCheck>,
    /// unresolved, non-outdated
    pub threads: Vec<Thread>,
    pub reviews: Vec<Review>,
}

/// New review feedback for Pavilion to triage before the agent sees it: (memory key, thread id or "", where, text).
pub fn feedback(d: &PrDetail, m: &Memory) -> Vec<(String, String, String, String)> {
    let threads = d.threads.iter().map(|t| {
        let at = format!("{}{}", t.path, t.line.map(|l| format!(":{l}")).unwrap_or_default());
        let text = t.comments.iter().map(|c| format!("{}: {}", c.author, c.body)).collect::<Vec<_>>().join("\n");
        (thread_key(t), t.id.clone(), at, text)
    });
    let reviews = d.reviews.iter().map(|r| (review_key(r), String::new(), "review".to_string(), format!("{}: {}", r.author, r.body)));
    threads.chain(reviews).filter(|(k, ..)| !m.handled.contains(k)).collect()
}

pub fn review_key(r: &Review) -> String {
    format!("review:{}", r.id)
}

#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    Rerun { key: String, check: String, run_id: u64 },
    /// `logs`: (check, run id) whose failed-log tail the runner appends to the prompt
    Invoke { kind: Kind, keys: Vec<String>, prompt: String, logs: Vec<(String, u64)> },
    NotifyReady,
    Escalate(Kind),
}

// ---- pure ----

/// GraphQL response (the github.rs query) -> detail for the newest PR. `h` supplies which checks failed.
pub fn parse_detail(j: &Value, h: &PrHealth) -> Option<PrDetail> {
    let n = j["data"]["repository"]["pullRequests"]["nodes"].get(0).filter(|n| n.is_object())?;
    let nodes = |v: &Value| v["nodes"].as_array().cloned().unwrap_or_default();
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let checks = nodes(&n["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"])
        .iter()
        .filter_map(|c| {
            let name = s(if c["__typename"] == "CheckRun" { &c["name"] } else { &c["context"] });
            h.failing_checks.contains(&name).then(|| FailCheck { name, run_id: c["checkSuite"]["workflowRun"]["databaseId"].as_u64() })
        })
        .collect();
    let me = &j["data"]["viewer"]["login"];
    let threads = nodes(&n["reviewThreads"])
        .iter()
        .filter(|t| t["isResolved"] == false && t["isOutdated"] != true)
        // last word is ours (incl. replies Tempering posted as the user) → nothing new to act on
        .filter(|t| t["comments"]["nodes"].as_array().and_then(|c| c.last()).is_none_or(|c| c["author"]["login"] != *me))
        .map(|t| Thread {
            id: s(&t["id"]),
            path: s(&t["path"]),
            line: t["line"].as_u64(),
            comments: nodes(&t["comments"]).iter().map(|c| Comment { id: c["databaseId"].as_u64().unwrap_or(0), author: s(&c["author"]["login"]), body: s(&c["body"]) }).collect(),
        })
        .collect();
    let reviews = nodes(&n["latestReviews"])
        .iter()
        .filter(|r| r["state"] == "CHANGES_REQUESTED" && r["author"]["login"] != *me && !s(&r["body"]).trim().is_empty())
        .map(|r| Review { id: s(&r["id"]), author: s(&r["author"]["login"]), body: s(&r["body"]) })
        .collect();
    Some(PrDetail { sha: s(&n["headRefOid"]), base: s(&n["baseRefName"]), checks, threads, reviews })
}

pub fn thread_key(t: &Thread) -> String {
    format!("thread:{}:{}", t.id, t.comments.last().map_or(0, |c| c.id))
}

pub fn reply_protocol() -> &'static str {
    "Reply protocol: after fixing each thread, append exactly one JSON line to `.murim/replies.jsonl` in the worktree (create it if missing): \
     {\"threadId\":\"<id>\",\"action\":\"reply\",\"body\":\"<short reply>\",\"resolve\":true} when fixed, or \
     {\"threadId\":\"<id>\",\"action\":\"disagree\",\"reason\":\"<why the change should not be made>\"} if you believe the comment is wrong. \
     Do NOT post replies to GitHub yourself and do NOT commit `.murim/`. Push your fixes to the PR branch."
}

fn thread_prompt(ts: &[&Thread]) -> String {
    let mut p = String::from("Review feedback on the PR needs fixing. Unresolved threads:\n");
    for t in ts {
        p += &format!("\n- threadId {} at {}{}\n", t.id, t.path, t.line.map(|l| format!(":{l}")).unwrap_or_default());
        for c in &t.comments {
            p += &format!("    {}: {}\n", c.author, c.body.replace('\n', " "));
        }
    }
    format!("{p}\nFix each, then record a reply or a disagreement per thread.\n{}", reply_protocol())
}

/// Rules from the plan. Records what it decided into `m` (handled keys, attempts, reruns).
pub fn decide(d: &PrDetail, h: &PrHealth, m: &mut Memory) -> Vec<Action> {
    let mut out = vec![];
    if h.state != PrState::Open {
        return out;
    }
    // the attempt cap is per failure streak: green CI / a clean merge starts the count over
    for (kind, clear) in [(Kind::Ci, h.ci == Ci::Passing), (Kind::Conflict, !h.conflict)] {
        if clear {
            m.attempts.remove(&kind);
            m.escalated.remove(&kind);
        }
    }
    // CI: rerun once per (sha, check), then invoke for what still fails
    let (mut ci_keys, mut ci_logs) = (vec![], vec![]);
    if !m.escalated.contains(&Kind::Ci) {
        for c in &d.checks {
            let rk = format!("{}:{}", d.sha, c.name);
            match c.run_id.filter(|_| !m.rerun.contains(&rk)) {
                Some(run_id) => {
                    m.rerun.insert(rk.clone());
                    out.push(Action::Rerun { key: rk, check: c.name.clone(), run_id });
                }
                None => {
                    let k = format!("ci:{rk}");
                    if !m.handled.contains(&k) {
                        ci_keys.push(k);
                        ci_logs.extend(c.run_id.map(|r| (c.name.clone(), r)));
                    }
                }
            }
        }
    }
    let ci_names: Vec<_> = d.checks.iter().filter(|c| ci_keys.contains(&format!("ci:{}:{}", d.sha, c.name))).map(|c| c.name.as_str()).collect();
    let ci_prompt = format!("CI is failing on the PR: {}. Fix it and push to the PR branch.", ci_names.join(", "));
    let new_threads: Vec<&Thread> = if m.escalated.contains(&Kind::Thread) { vec![] } else { d.threads.iter().filter(|t| !m.handled.contains(&thread_key(t))).collect() };
    let new_reviews: Vec<&Review> = if m.escalated.contains(&Kind::Review) { vec![] } else { d.reviews.iter().filter(|r| !m.handled.contains(&review_key(r))).collect() };
    let review_prompt = format!(
        "A reviewer requested changes on the PR:\n{}\n\nMake the changes and push to the PR branch. (Review summaries have no thread: no reply line needed.)",
        new_reviews.iter().map(|r| format!("- {}: {}", r.author, r.body.replace('\n', " "))).collect::<Vec<_>>().join("\n")
    );
    let conflict = h.conflict && !m.escalated.contains(&Kind::Conflict) && !m.handled.contains(&format!("conflict:{}", d.sha));
    let triggers = [
        (Kind::Ci, ci_keys, ci_prompt, ci_logs),
        (Kind::Thread, new_threads.iter().map(|t| thread_key(t)).collect(), thread_prompt(&new_threads), vec![]),
        (Kind::Review, new_reviews.iter().map(|r| review_key(r)).collect(), review_prompt, vec![]),
        (Kind::Conflict, if conflict { vec![format!("conflict:{}", d.sha)] } else { vec![] }, format!("The PR has merge conflicts. Rebase onto {}, resolve the conflicts, and push.", d.base), vec![]),
    ];
    for (kind, keys, prompt, logs) in triggers {
        if keys.is_empty() {
            continue;
        }
        let n = m.attempts.entry(kind).or_default();
        if *n >= MAX_INVOKES {
            m.escalated.insert(kind);
            out.push(Action::Escalate(kind));
        } else {
            *n += 1;
            m.handled.extend(keys.iter().cloned());
            out.push(Action::Invoke { kind, keys, prompt, logs });
        }
    }
    if h.overall == Overall::ReadyToMerge {
        if !std::mem::replace(&mut m.ready, true) {
            out.push(Action::NotifyReady);
        }
    } else {
        m.ready = false;
    }
    out
}

/// Last `n` lines.
pub fn tail(s: &str, n: usize) -> String {
    let l: Vec<_> = s.lines().collect();
    l[l.len().saturating_sub(n)..].join("\n")
}

// ---- review replies (pure) ----

#[derive(Deserialize, PartialEq, Debug)]
#[serde(tag = "action", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Line {
    Reply {
        #[serde(alias = "thread_id")]
        thread_id: String,
        body: String,
        #[serde(default)]
        resolve: bool,
    },
    Disagree {
        #[serde(alias = "thread_id")]
        thread_id: String,
        reason: String,
    },
}

/// One JSON object per line; malformed lines are skipped.
pub fn parse_replies(text: &str) -> Vec<Line> {
    text.lines().filter_map(|l| serde_json::from_str(l.trim()).ok()).collect()
}

/// What to post for a reply body under the PR's mode (None = drop).
pub fn reply_body(body: &str, mode: ReplyMode) -> Option<String> {
    match mode {
        ReplyMode::Off => None,
        ReplyMode::Terse if body.split_whitespace().count() > 5 || body.trim().is_empty() => Some("Fixed".into()),
        _ => Some(body.to_string()),
    }
}

// ---- io ----

fn details() -> &'static Mutex<HashMap<String, PrDetail>> {
    static D: OnceLock<Mutex<HashMap<String, PrDetail>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

pub fn log(app: &AppHandle, id: &str, message: impl Into<String>) {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let _ = app.emit("tempering-log", json!({"issueId": id, "at": at, "message": message.into()}));
}

pub fn gh(wt: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new("gh").args(args).current_dir(wt).envs(sessions::shell_env().0.iter().map(|(k, v)| (k, v))).stdin(std::process::Stdio::null()).output().map_err(|e| format!("gh: {e}"))?;
    let (out, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    if o.status.success() { Ok(out.into_owned()) } else { Err(format!("gh {}: {}", args.join(" "), err.trim())) }
}

/// Post a reply (and optionally resolve). Records the new comment as handled so it doesn't re-trigger.
fn post(app: &AppHandle, id: &str, thread_id: &str, body: &str, resolve: bool) -> Result<(), String> {
    let tok = github::token()?;
    let r = github::gql(&tok, "mutation($t:ID!,$b:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$t,body:$b}){comment{databaseId}}}", json!({"t": thread_id, "b": body}))?;
    if let Some(c) = r["data"]["addPullRequestReviewThreadReply"]["comment"]["databaseId"].as_u64() {
        sessions::with_session(app, id, |s| s.memory.handled.insert(format!("thread:{thread_id}:{c}")));
    }
    if resolve {
        github::gql(&tok, "mutation($t:ID!){resolveReviewThread(input:{threadId:$t}){thread{id}}}", json!({"t": thread_id}))?;
    }
    log(app, id, format!("Replied on thread{}: {body}", if resolve { " (resolved)" } else { "" }));
    Ok(())
}

/// Read + truncate `.murim/replies.jsonl`, apply the reply mode, post; disagreements become escalations.
pub fn consume(app: &AppHandle, id: &str) {
    if sessions::no_pr_flow(id) {
        return; // review agents write review.json, meditations have no PR
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let Some(wt) = sessions::with_session(app, id, |s| s.worktree.clone()).filter(|w| !w.is_empty()) else { return }; // "" = repo-pick session
    let mode = crate::settings::dir(app).map(|d| crate::settings::load(&d).reply_mode).unwrap_or_default();
    let f = Path::new(&wt).join(".murim/replies.jsonl");
    let Ok(text) = std::fs::read_to_string(&f) else { return };
    let _ = std::fs::write(&f, "");
    for l in parse_replies(&text) {
        match l {
            Line::Reply { thread_id, body, resolve } => match reply_body(&body, mode) {
                Some(b) => {
                    if let Err(e) = post(app, id, &thread_id, &b, resolve) {
                        log(app, id, format!("Reply failed: {e}"));
                    }
                }
                None => log(app, id, "Reply dropped (reply mode off)"),
            },
            Line::Disagree { thread_id, reason } => {
                let t = details().lock().unwrap().get(id).and_then(|d| d.threads.iter().find(|t| t.id == thread_id).cloned());
                let (path, comment) = t.map(|t| (t.path, t.comments.last().map(|c| c.body.clone()).unwrap_or_default())).unwrap_or_default();
                let e = Escalation { id: format!("{thread_id}-{}", rand::random::<u32>()), thread_id, path: path.clone(), comment, reason, ask: false };
                let title = sessions::with_session(app, id, |s| {
                    s.escalations.push(e);
                    s.status = Status::NeedsInput;
                    sessions::title_of(s)
                });
                if let Some(t) = title {
                    sessions::notify(app, t, "Claude disagrees with a review comment".into());
                }
                log(app, id, format!("Claude disagrees with the comment on {path}: needs your decision"));
            }
        }
    }
}

/// After a PR poll: consume replies, decide, execute. Acts only on idle/ended sessions with tempering on.
pub fn run(app: &AppHandle, id: &str, d: &PrDetail, h: &PrHealth) {
    details().lock().unwrap().insert(id.into(), d.clone());
    consume(app, id);
    let Some((on, wt, status, mut mem, title)) = sessions::with_session(app, id, |s| (s.tempering, s.worktree.clone(), s.status, s.memory.clone(), sessions::title_of(s))) else { return };
    if !on || !matches!(status, Status::Idle | Status::Ended) || now() < mem.retry_at {
        return;
    }
    // an adopted PR's Claude starts fresh (no PTY yet): that takes an agent slot like any new agent
    let fresh = sessions::is_adopted(id) && sessions::with_session(app, id, |s| !s.pty_alive).unwrap_or(false);
    if fresh {
        let map = app.state::<sessions::Sessions>().snapshot();
        let max = crate::settings::dir(app).map(|d| crate::settings::load(&d).pavilion.max_agents).unwrap_or(10) as usize;
        if crate::pavilion::working(&map) >= max {
            return;
        }
    }
    if triage(app, id, d, &mut mem) {
        if let Some(t) = sessions::with_session(app, id, |s| {
            s.status = Status::NeedsInput;
            sessions::title_of(s)
        }) {
            sessions::notify(app, t, "A review comment needs your call".into());
        }
    }
    let (mut invokes, mut prompts, mut logs) = (vec![], vec![], vec![]);
    if sessions::is_adopted(id) && sessions::with_session(app, id, |s| s.session_id.is_none()).unwrap_or(false) {
        prompts.push(format!("This worktree is the branch of your PR {} ({}). Pull the latest first (`git pull --ff-only`).", h.url, h.title));
    }
    for a in decide(d, h, &mut mem) {
        match a {
            Action::Rerun { key, check, run_id } => match gh(&wt, &["run", "rerun", &run_id.to_string(), "--failed"]) {
                Ok(_) => log(app, id, format!("Re-ran failed jobs of {check}")),
                Err(e) => {
                    mem.rerun.remove(&key);
                    log(app, id, format!("Re-run of {check} failed: {e}"));
                }
            },
            Action::Invoke { kind, keys, prompt, logs: l } => {
                log(app, id, format!("Invoking Claude: {kind:?} ({} trigger(s))", keys.len()));
                prompts.push(prompt);
                logs.extend(l);
                invokes.push((kind, keys));
            }
            Action::NotifyReady => {
                sessions::notify(app, "Ready to merge".into(), title.clone());
                log(app, id, "PR is ready to merge");
            }
            Action::Escalate(kind) => {
                sessions::notify(app, title.clone(), format!("Tempering gave up on {kind:?} after {MAX_INVOKES} attempts"));
                log(app, id, format!("Escalated: {kind:?} still failing after {MAX_INVOKES} attempts"));
                sessions::with_session(app, id, |s| s.status = Status::NeedsInput);
            }
        }
    }
    if !invokes.is_empty() {
        for (name, run) in logs {
            let t = gh(&wt, &["run", "view", &run.to_string(), "--log-failed"]).map(|o| tail(&o, LOG_LINES)).unwrap_or_else(|e| e);
            prompts.push(format!("Failed log tail for {name} (last {LOG_LINES} lines):\n{t}"));
        }
        let _ = std::fs::create_dir_all(Path::new(&wt).join(".murim"));
        // memory is saved before the turn starts: its hooks (Stop/StopFailure) may arrive any time after
        mem.inflight = invokes;
        let note = note(&mem.inflight);
        sessions::with_session(app, id, |s| {
            s.tempering_turn = true;
            s.tempering_note = note;
            s.memory = mem;
        });
        if let Err(e) = sessions::resume_task(app.clone(), id.into(), Some(prompts.join("\n\n"))) {
            sessions::with_session(app, id, |s| {
                s.tempering_turn = false;
                rollback(&mut s.memory);
            });
            log(app, id, format!("Could not reach Claude: {e}"));
        }
        return;
    }
    sessions::with_session(app, id, |s| s.memory = mem);
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Un-mark what the Tempering turn was fixing so a later poll invokes it again.
pub fn rollback(m: &mut Memory) {
    for (kind, keys) in std::mem::take(&mut m.inflight) {
        for k in keys {
            m.handled.remove(&k);
        }
        if let Some(n) = m.attempts.get_mut(&kind) {
            *n = n.saturating_sub(1);
        }
    }
}

/// A Tempering turn ended on an API error (StopFailure): retry the same work after RETRY_SECS.
pub fn turn_failed(m: &mut Memory) {
    rollback(m);
    m.retry_at = now() + RETRY_SECS;
}

/// Pavilion's card while a Tempering turn runs.
fn note(invokes: &[(Kind, Vec<String>)]) -> String {
    let n = |k: Kind| invokes.iter().filter(|(x, _)| *x == k).map(|(_, keys)| keys.len()).sum::<usize>();
    let mut parts = vec![];
    if n(Kind::Ci) > 0 {
        parts.push("fixing CI".to_string());
    }
    match n(Kind::Thread) {
        0 => {}
        1 => parts.push("fixing 1 review comment".into()),
        c => parts.push(format!("fixing {c} review comments")),
    }
    if n(Kind::Review) > 0 {
        parts.push("addressing requested changes".into());
    }
    if n(Kind::Conflict) > 0 {
        parts.push("resolving merge conflicts".into());
    }
    parts.join(", ")
}

/// Pavilion: the local model sorts new review feedback before the agent sees it. "skip" is dropped (logged), "ask"
/// becomes an escalation for the user, "act" (or no answer at all) is left for `decide` as before. Returns whether it asked.
/// ponytail: runs on the PR poll thread, one judgement per new comment (each only once); a slow model delays that poll.
fn triage(app: &AppHandle, id: &str, d: &PrDetail, mem: &mut Memory) -> bool {
    let items = feedback(d, mem);
    if items.is_empty() {
        return false;
    }
    let cfg = crate::settings::dir(app).map(|d| crate::settings::load(&d).pavilion).unwrap_or_default();
    let allowed = judge::allowed(Q::Feedback, &[], "");
    let mut asked = false;
    for (key, thread_id, at, text) in items {
        let o = judge::ask(&cfg, Q::Feedback, &judge::prompt(Q::Feedback, &format!("{at}\n{text}"), &[], ""), &allowed, &key, |p| sessions::claude_print(p, Duration::from_secs(90)));
        for l in &o.log {
            crate::pavilion::olog(format!("  {id} feedback on {at}: {l}"));
        }
        let Some(v) = o.verdict else { continue };
        match v.answer.as_str() {
            "skip" => {
                mem.handled.insert(key);
                log(app, id, format!("Skipped feedback on {at}: {}", v.reason));
            }
            "ask" => {
                mem.handled.insert(key.clone());
                let e = Escalation { id: format!("{key}-{}", rand::random::<u32>()), thread_id, path: at.clone(), comment: text, reason: v.reason, ask: true };
                sessions::with_session(app, id, |s| s.escalations.push(e));
                log(app, id, format!("Feedback on {at} needs your call"));
                asked = true;
            }
            _ => {}
        }
    }
    asked
}

/// Pavilion's "Handle PR feedback automatically": Tempering on for a Claude session once its PR is open,
/// unless the user switched it off for that session.
pub fn auto_enable(app: &AppHandle, id: &str, h: &PrHealth) {
    if h.state != PrState::Open || !crate::settings::dir(app).is_ok_and(|d| crate::settings::load(&d).pavilion.auto_temper) {
        return;
    }
    let flip = |s: &mut sessions::SessionInfo| {
        let on = !s.tempering && !s.tempering_off && s.agent != crate::settings::AgentKind::Custom && !s.worktree.is_empty();
        s.tempering |= on;
        on.then(|| s.worktree.clone())
    };
    if let Some(wt) = sessions::with_session(app, id, flip).flatten() {
        let _ = std::fs::create_dir_all(Path::new(&wt).join(".murim"));
        let _ = sessions::exclude_settings(Path::new(&wt));
        log(app, id, "Tempering on: Pavilion handles PR feedback");
    }
}

// ---- commands ----

#[tauri::command]
pub fn set_tempering(app: AppHandle, issue_id: String, on: bool) -> Result<(), String> {
    if on && sessions::with_session(&app, &issue_id, |s| s.agent) == Some(crate::settings::AgentKind::Custom) {
        return Err("Tempering needs Claude Code's hooks; it isn't available for custom agents".into());
    }
    let wt = sessions::with_session(&app, &issue_id, |s| {
        s.tempering = on;
        s.tempering_off = !on;
        s.worktree.clone()
    })
    .ok_or("unknown session")?;
    if on {
        let _ = std::fs::create_dir_all(Path::new(&wt).join(".murim"));
        let _ = sessions::exclude_settings(Path::new(&wt)); // sessions created before .murim/ was excluded
    }
    Ok(())
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Choice {
    DoIt,
    PostPushback,
    OwnReply,
    Ignore,
}

#[tauri::command]
pub async fn resolve_escalation(app: AppHandle, issue_id: String, escalation_id: String, choice: Choice, text: Option<String>) -> Result<(), String> {
    let e = sessions::with_session(&app, &issue_id, |s| s.escalations.iter().find(|e| e.id == escalation_id).cloned()).flatten().ok_or("unknown escalation")?;
    match choice {
        Choice::DoIt => {
            let p = if e.ask {
                format!("The PR owner wants this review feedback addressed ({}):\n{}\n\nMake the change and push.{}", e.path, e.comment, if e.thread_id.is_empty() { String::new() } else { format!(" (threadId {})\n{}", e.thread_id, reply_protocol()) })
            } else { format!("Make the change requested in thread {}: {} even though you disagreed\n\n(threadId {})\n{}", e.path, e.comment, e.thread_id, reply_protocol()) };
            sessions::with_session(&app, &issue_id, |s| s.tempering_turn = true);
            sessions::resume_task(app.clone(), issue_id.clone(), Some(p)).inspect_err(|_| {
                sessions::with_session(&app, &issue_id, |s| s.tempering_turn = false);
            })?;
        }
        Choice::PostPushback => post(&app, &issue_id, &e.thread_id, &e.reason, false)?, // explicit user choice: ignores replyMode
        Choice::OwnReply => post(&app, &issue_id, &e.thread_id, text.as_deref().filter(|t| !t.trim().is_empty()).ok_or("empty reply")?, false)?,
        Choice::Ignore => {}
    }
    sessions::with_session(&app, &issue_id, |s| {
        s.escalations.retain(|x| x.id != escalation_id);
        if s.escalations.is_empty() && s.status == Status::NeedsInput && choice_idles(choice) {
            s.status = Status::Idle;
        }
    });
    Ok(())
}

/// doIt resumes Claude (hooks drive the status); the others leave the session waiting.
fn choice_idles(c: Choice) -> bool {
    !matches!(c, Choice::DoIt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Ci, PrState};

    fn health(over: impl Fn(&mut PrHealth)) -> PrHealth {
        let mut h = PrHealth { number: 1, url: "u".into(), title: "t".into(), state: PrState::Open, is_draft: false, ci: Ci::Passing, failing_checks: vec![], unresolved_threads: 0, changes_requested: false, conflict: false, review_decision: Some("APPROVED".into()), overall: Overall::ReadyToMerge };
        over(&mut h);
        h.overall = github::derive_overall(&h);
        h
    }
    fn failing(sha: &str) -> PrDetail {
        PrDetail { sha: sha.into(), base: "main".into(), checks: vec![FailCheck { name: "build".into(), run_id: Some(9) }], threads: vec![], reviews: vec![] }
    }
    fn red() -> PrHealth {
        health(|h| (h.ci, h.failing_checks) = (Ci::Failing, vec!["build".into()]))
    }
    fn thread(id: &str, last: u64) -> Thread {
        Thread { id: id.into(), path: "a.rs".into(), line: Some(3), comments: vec![Comment { id: last, author: "bob".into(), body: "rename".into() }] }
    }
    fn invokes(a: &[Action]) -> usize {
        a.iter().filter(|a| matches!(a, Action::Invoke { .. })).count()
    }

    #[test]
    fn ci_rerun_then_invoke_once() {
        let (d, h, mut m) = (failing("s1"), red(), Memory::default());
        let a = decide(&d, &h, &mut m);
        assert!(matches!(&a[..], [Action::Rerun { run_id: 9, .. }]));
        let a = decide(&d, &h, &mut m); // still failing after the rerun
        match &a[..] {
            [Action::Invoke { kind: Kind::Ci, prompt, logs, .. }] => assert!(prompt.contains("build") && logs == &[("build".to_string(), 9)]),
            x => panic!("{x:?}"),
        }
        assert!(decide(&d, &h, &mut m).is_empty()); // same sha/check never re-invoked
        let a = decide(&failing("s2"), &h, &mut m); // new sha: rerun again
        assert!(matches!(&a[..], [Action::Rerun { .. }]));
    }

    #[test]
    fn ci_without_workflow_run_invokes_directly() {
        let mut d = failing("s");
        d.checks[0].run_id = None;
        assert_eq!(invokes(&decide(&d, &red(), &mut Memory::default())), 1);
    }

    #[test]
    fn threads_new_handled_and_new_comment() {
        let mut d = PrDetail { sha: "s".into(), base: "main".into(), ..Default::default() };
        let h = health(|h| h.unresolved_threads = 1);
        let mut m = Memory::default();
        d.threads = vec![thread("T1", 10)];
        let a = decide(&d, &h, &mut m);
        match &a[..] {
            [Action::Invoke { kind: Kind::Thread, prompt, .. }] => assert!(prompt.contains("T1") && prompt.contains(".murim/replies.jsonl") && prompt.contains("rename")),
            x => panic!("{x:?}"),
        }
        assert!(decide(&d, &h, &mut m).is_empty()); // already handled
        d.threads[0].comments.push(Comment { id: 11, author: "bob".into(), body: "still wrong".into() });
        assert_eq!(invokes(&decide(&d, &h, &mut m)), 1); // new comment on a handled thread
    }

    #[test]
    fn conflict_once_per_sha() {
        let d = PrDetail { sha: "s".into(), base: "develop".into(), ..Default::default() };
        let h = health(|h| h.conflict = true);
        let mut m = Memory::default();
        match &decide(&d, &h, &mut m)[..] {
            [Action::Invoke { kind: Kind::Conflict, prompt, .. }] => assert!(prompt.contains("develop")),
            x => panic!("{x:?}"),
        }
        assert!(decide(&d, &h, &mut m).is_empty());
        assert_eq!(invokes(&decide(&PrDetail { sha: "s2".into(), ..d }, &h, &mut m)), 1);
    }

    #[test]
    fn ready_notifies_once_until_it_drops() {
        let (d, h, mut m) = (PrDetail::default(), health(|_| {}), Memory::default());
        assert_eq!(decide(&d, &h, &mut m), vec![Action::NotifyReady]);
        assert!(decide(&d, &h, &mut m).is_empty());
        decide(&d, &red(), &mut m);
        assert_eq!(decide(&d, &h, &mut m), vec![Action::NotifyReady]);
    }

    #[test]
    fn fourth_invoke_escalates_then_silence() {
        let (h, mut m) = (health(|h| h.unresolved_threads = 1), Memory::default());
        let mut d = PrDetail { sha: "s".into(), ..Default::default() };
        for i in 1..=3 {
            d.threads = vec![thread("T", i)];
            assert_eq!(invokes(&decide(&d, &h, &mut m)), 1);
        }
        d.threads = vec![thread("T", 4)];
        assert_eq!(decide(&d, &h, &mut m), vec![Action::Escalate(Kind::Thread)]);
        d.threads = vec![thread("T", 5)];
        assert!(decide(&d, &h, &mut m).is_empty());
        // other kinds are unaffected
        assert!(m.escalated.contains(&Kind::Thread) && !m.escalated.contains(&Kind::Ci));
    }

    #[test]
    fn ci_cap_resets_when_ci_goes_green() {
        let mut m = Memory::default();
        for sha in ["a", "b", "c"] {
            decide(&failing(sha), &red(), &mut m); // rerun
            assert_eq!(invokes(&decide(&failing(sha), &red(), &mut m)), 1);
        }
        decide(&failing("d"), &red(), &mut m);
        assert_eq!(decide(&failing("d"), &red(), &mut m), vec![Action::Escalate(Kind::Ci)]);
        decide(&PrDetail::default(), &health(|_| {}), &mut m); // green
        decide(&failing("e"), &red(), &mut m);
        assert_eq!(invokes(&decide(&failing("e"), &red(), &mut m)), 1);
    }

    #[test]
    fn failed_turn_is_retried() {
        let (d, mut m) = (failing("s"), Memory::default());
        decide(&d, &red(), &mut m);
        for a in decide(&d, &red(), &mut m) {
            if let Action::Invoke { kind, keys, .. } = a {
                m.inflight.push((kind, keys));
            }
        }
        turn_failed(&mut m);
        assert!(m.retry_at > now() && m.inflight.is_empty() && m.attempts[&Kind::Ci] == 0);
        assert_eq!(invokes(&decide(&d, &red(), &mut m)), 1); // same sha/check invoked again
    }

    #[test]
    fn closed_pr_does_nothing_and_memory_roundtrips() {
        let h = health(|h| h.state = PrState::Merged);
        assert!(decide(&failing("s"), &h, &mut Memory::default()).is_empty());
        let mut m = Memory::default();
        decide(&failing("s"), &red(), &mut m);
        decide(&failing("s"), &red(), &mut m);
        let back: Memory = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(serde_json::from_str::<Memory>("{}").unwrap(), Memory::default());
    }

    #[test]
    fn parses_detail() {
        let j = json!({"data": {"viewer": {"login": "me"}, "repository": {"pullRequests": {"nodes": [{
            "headRefOid": "abc", "baseRefName": "main",
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [
                {"__typename": "CheckRun", "name": "build", "databaseId": 5, "checkSuite": {"workflowRun": {"databaseId": 99}}},
                {"__typename": "CheckRun", "name": "ok", "checkSuite": {"workflowRun": {"databaseId": 1}}},
                {"__typename": "StatusContext", "context": "legacy"}]}}}}]},
            "reviewThreads": {"nodes": [
                {"id": "T1", "path": "a.rs", "line": 4, "isResolved": false, "isOutdated": false, "comments": {"nodes": [{"databaseId": 7, "author": {"login": "bob"}, "body": "x"}]}},
                {"id": "T2", "isResolved": true, "isOutdated": false, "comments": {"nodes": []}},
                {"id": "T3", "isResolved": false, "isOutdated": true, "comments": {"nodes": []}},
                {"id": "T4", "isResolved": false, "isOutdated": false, "comments": {"nodes": [
                    {"databaseId": 8, "author": {"login": "bob"}, "body": "rename"}, {"databaseId": 9, "author": {"login": "me"}, "body": "Looking into it"}]}}]},
            "latestReviews": {"nodes": [
                {"id": "R1", "state": "CHANGES_REQUESTED", "body": "Split this function", "author": {"login": "bob"}},
                {"id": "R2", "state": "CHANGES_REQUESTED", "body": "", "author": {"login": "amy"}},
                {"id": "R3", "state": "APPROVED", "body": "lgtm", "author": {"login": "cat"}},
                {"id": "R4", "state": "CHANGES_REQUESTED", "body": "self", "author": {"login": "me"}}]}}]}}}});
        let h = health(|h| h.failing_checks = vec!["build".into(), "legacy".into()]);
        let d = parse_detail(&j, &h).unwrap();
        assert_eq!((d.sha.as_str(), d.base.as_str()), ("abc", "main"));
        assert_eq!(d.checks, vec![FailCheck { name: "build".into(), run_id: Some(99) }, FailCheck { name: "legacy".into(), run_id: None }]);
        assert_eq!(d.threads.len(), 1);
        assert_eq!((thread_key(&d.threads[0]).as_str(), d.threads[0].comments[0].author.as_str()), ("thread:T1:7", "bob"));
        // only others' change requests with something to read
        assert_eq!(d.reviews, vec![Review { id: "R1".into(), author: "bob".into(), body: "Split this function".into() }]);
    }

    #[test]
    fn review_summary_is_a_trigger_and_feedback_lists_new_items() {
        let mut d = PrDetail { sha: "s".into(), base: "main".into(), ..Default::default() };
        d.reviews = vec![Review { id: "R1".into(), author: "bob".into(), body: "Split this function".into() }];
        d.threads = vec![thread("T1", 10)];
        let h = health(|h| h.changes_requested = true);
        let mut m = Memory::default();
        let f = feedback(&d, &m);
        assert_eq!(f.iter().map(|x| (x.0.as_str(), x.1.as_str(), x.2.as_str())).collect::<Vec<_>>(), [("thread:T1:10", "T1", "a.rs:3"), ("review:R1", "", "review")]);
        m.handled.insert("thread:T1:10".into()); // triage skipped or asked about it
        assert_eq!(feedback(&d, &m).len(), 1);
        match &decide(&d, &h, &mut m)[..] {
            [Action::Invoke { kind: Kind::Review, keys, prompt, .. }] => assert!(keys == &["review:R1"] && prompt.contains("Split this function")),
            x => panic!("{x:?}"),
        }
        assert!(decide(&d, &h, &mut m).is_empty() && feedback(&d, &m).is_empty()); // handled once
        assert_eq!(note(&[(Kind::Thread, vec!["a".into(), "b".into()]), (Kind::Review, vec!["r".into()])]), "fixing 2 review comments, addressing requested changes");
    }

    #[test]
    fn replies_parse_and_modes() {
        let t = "{\"threadId\":\"T1\",\"action\":\"reply\",\"body\":\"Fixed\",\"resolve\":true}\nnot json\n{\"threadId\":\"T2\",\"action\":\"disagree\",\"reason\":\"breaks Y\"}\n{\"thread_id\":\"T3\",\"action\":\"reply\",\"body\":\"ok\"}\n{\"action\":\"bogus\"}\n";
        let l = parse_replies(t);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0], Line::Reply { thread_id: "T1".into(), body: "Fixed".into(), resolve: true });
        assert_eq!(l[1], Line::Disagree { thread_id: "T2".into(), reason: "breaks Y".into() });
        assert_eq!(l[2], Line::Reply { thread_id: "T3".into(), body: "ok".into(), resolve: false });
        let long = "Fixed by renaming the helper and updating callers";
        assert_eq!(reply_body(long, ReplyMode::Off), None);
        assert_eq!(reply_body(long, ReplyMode::Terse).as_deref(), Some("Fixed"));
        assert_eq!(reply_body("Done, thanks a lot", ReplyMode::Terse).as_deref(), Some("Done, thanks a lot"));
        assert_eq!(reply_body(long, ReplyMode::Full).as_deref(), Some(long));
    }

    #[test]
    fn tail_keeps_last_lines() {
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
        assert_eq!(tail("a", 80), "a");
    }
}
