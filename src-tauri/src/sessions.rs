use crate::hooks::{self, Hooks, PrFlow, Status, StopOutcome};
use crate::repo_match::Source;
use crate::settings::{self, AgentKind, PermissionMode};
use crate::tempering::{Escalation, Memory};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;

const RING: usize = 1 << 20;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub issue_id: String,
    pub worktree: String,
    pub session_id: Option<String>,
    pub status: Status,
    pub questions: Option<Value>,
    pub pty_alive: bool,
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub tempering: bool,
    /// the user switched Tempering off here: Pavilion's auto-enable leaves it alone
    #[serde(default)]
    pub tempering_off: bool,
    /// what the current Tempering turn is fixing, for Pavilion's card ("fixing 2 review comments")
    #[serde(default)]
    pub tempering_note: String,
    #[serde(default)]
    pub escalations: Vec<Escalation>,
    #[serde(default)]
    pub memory: Memory,
    #[serde(default)]
    pub pr_flow: PrFlow,
    /// agent this session was started with (custom command captured so Resume reruns the same one)
    #[serde(default)]
    pub agent: AgentKind,
    #[serde(default)]
    pub agent_command: String,
    /// meditation display name
    #[serde(default)]
    pub name: String,
    /// set when tempering resumes the session; cleared on the next Stop
    #[serde(skip)]
    pub tempering_turn: bool,
}

struct Pty {
    gen: u64,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// The last RING bytes of a PTY's output, plus how many bytes it has ever written: a remounted terminal replays the
/// snapshot and drops the live chunks it already holds (those ending at or before the snapshot's `end`).
#[derive(Default)]
struct Ring {
    bytes: VecDeque<u8>,
    end: u64,
}

impl Ring {
    /// Append a chunk; returns the stream offset just past it.
    fn push(&mut self, b: &[u8]) -> u64 {
        self.bytes.extend(b);
        let over = self.bytes.len().saturating_sub(RING);
        self.bytes.drain(..over);
        self.end += b.len() as u64;
        self.end
    }
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, SessionInfo>,
    ptys: HashMap<String, Pty>,
    bufs: HashMap<String, Ring>,
    gen: u64,
}

#[derive(Default)]
pub struct Sessions(Mutex<Inner>);

fn sessions(app: &AppHandle) -> &Sessions {
    app.state::<Sessions>().inner()
}

/// Mutate under the lock, then persist + emit the full map.
fn update<R>(app: &AppHandle, f: impl FnOnce(&mut Inner) -> R) -> R {
    let (r, map) = {
        let mut g = sessions(app).0.lock().unwrap();
        let r = f(&mut g);
        if let Ok(d) = settings::dir(app) {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("sessions.json"), serde_json::to_string_pretty(&g.map).unwrap_or_default());
        }
        let _ = app.emit("sessions-updated", &g.map);
        (r, g.map.clone())
    };
    crate::tray::refresh(app, &map); // outside the lock: it reads the inbox
    crate::pavilion::poke(app); // sessions changed: re-evaluate the board
    r
}

impl Sessions {
    pub fn snapshot(&self) -> HashMap<String, SessionInfo> {
        self.0.lock().unwrap().map.clone()
    }
}

/// Mutate one session (persist + emit); None if unknown.
pub fn with_session<R>(app: &AppHandle, id: &str, f: impl FnOnce(&mut SessionInfo) -> R) -> Option<R> {
    update(app, |g| g.map.get_mut(id).map(f))
}

/// On startup every persisted session is ended/resumable (no PTY survives a restart).
pub fn load(app: &AppHandle) {
    let Ok(d) = settings::dir(app) else { return };
    let mut map: HashMap<String, SessionInfo> = std::fs::read_to_string(d.join("sessions.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for s in map.values_mut() {
        (s.status, s.pty_alive, s.questions) = (Status::Ended, false, None);
    }
    sessions(app).0.lock().unwrap().map = map;
}

/// App exit: hang everything up, give agents up to 1.5s to exit cleanly, then kill what's left.
pub fn kill_all(app: &AppHandle) {
    let mut g = sessions(app).0.lock().unwrap();
    g.ptys.values().for_each(|p| sighup(p.child.as_ref()));
    for _ in 0..15 {
        if g.ptys.values_mut().all(|p| matches!(p.child.try_wait(), Ok(Some(_)))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for p in g.ptys.values_mut() {
        let _ = p.child.kill();
    }
}

// Graceful stops. portable-pty's kill() escalates SIGHUP -> SIGKILL after 250ms, but Claude needs ~700ms to exit
// cleanly; killed sooner (within ~10s of launch) it leaves its fullscreen boot-canary record behind, and its next launch
// in ANY terminal falls back to the classic renderer (two strikes disable fullscreen for that version).
fn sighup(c: &(dyn portable_pty::Child + Send + Sync)) {
    if let Some(pid) = c.process_id() {
        let _ = std::process::Command::new("/bin/kill").args(["-HUP", &pid.to_string()]).status();
    }
}

/// Stop a PTY still in the map (its reader thread marks the session ended); SIGKILL after 3s if it ignores SIGHUP.
fn hangup(app: &AppHandle, id: &str) {
    let gen = {
        let g = sessions(app).0.lock().unwrap();
        let Some(p) = g.ptys.get(id) else { return };
        sighup(p.child.as_ref());
        p.gen
    };
    let (app, id) = (app.clone(), id.to_string());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        if let Some(p) = sessions(&app).0.lock().unwrap().ptys.get_mut(&id).filter(|p| p.gen == gen) {
            let _ = p.child.kill();
        }
    });
}

/// Stop a PTY already taken out of the map.
fn hangup_owned(mut p: Pty) {
    std::thread::spawn(move || {
        sighup(p.child.as_ref());
        for _ in 0..30 {
            if matches!(p.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = p.child.kill();
    });
}

// ---- hooks -> status ----

/// Agent-only sessions (review, meditation, automation): no PR polling, no Tempering.
pub fn no_pr_flow(id: &str) -> bool {
    id.starts_with("review:") || agent_only(id)
}

pub fn is_meditation(id: &str) -> bool {
    id.starts_with("meditation:")
}

pub fn is_automation(id: &str) -> bool {
    id.starts_with("automation:")
}

/// One of the user's PRs Pavilion picked up (adopt.rs), keyed `pr:<owner>/<repo>/<n>`.
pub fn is_adopted(id: &str) -> bool {
    id.starts_with("pr:")
}

/// Pavilion's read-only planning session keyed `investigation:<issueId>`.
pub fn is_investigation(id: &str) -> bool {
    id.starts_with("investigation:")
}

/// Status + notifications only: no repo pick, markers, PR flow or Tempering; hooks via --settings.
fn agent_only(id: &str) -> bool {
    is_meditation(id) || is_automation(id) || is_investigation(id)
}

pub fn title_of(s: &SessionInfo) -> String {
    if agent_only(&s.issue_id) {
        return s.name.clone();
    }
    if let Some(pr) = s.issue_id.strip_prefix("review:") {
        return format!("Review {pr}"); // "Review owner/repo/12"
    }
    if let Some((repo, n)) = s.issue_id.strip_prefix("pr:").and_then(|p| p.split('/').nth(1).zip(p.rsplit('/').next())) {
        return format!("{repo}#{n}");
    }
    Path::new(&s.worktree).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| s.issue_id.clone())
}

pub fn notify(app: &AppHandle, title: String, body: String) {
    let _ = app.notification().builder().title(title).body(body).show();
}

const DONE_SUFFIX: &str = "\n\nWhen the issue is fully implemented, end your final message with the line MURIM_DONE.";

fn quality_prompt(ident: &str, base: &str) -> String {
    format!("Quality pass. Run /ponytail:ponytail-review on this branch's diff vs {base} (if that skill is not available, say so and do an equivalent self-review for bugs, needless complexity and dead code), and apply its fixes. Run the project's tests and linters and fix failures. Commit all changes (no AI attribution or Co-Authored-By lines), push the branch, then open a draft PR with `gh pr create --draft` with a clear title and description; the body must include `Fixes {ident}`. Do NOT mark it ready: the user reviews it first. End your final message with the line MURIM_PR_OPENED.")
}

/// Claude runs in repos_root (worktree "") until it names the repo; see hooks::repo_marker.
fn pick_prompt(issue: &str, repos: &[crate::repo_match::Repo], guess: Option<&crate::repo_match::RepoMatch>) -> String {
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    let guess = guess.and_then(|g| Path::new(&g.path).file_name()).map(|n| format!("\nLikely repo: {} (guess)", n.to_string_lossy())).unwrap_or_default();
    format!("Before work starts on this issue, decide which repo it belongs in. You are in the folder that holds all repos. Do NOT edit, create or commit anything; only read (READMEs, code) as needed. If it is unclear, ask me. When you know, end your final message with the line `MURIM_REPO: <repo-name>` (one of the repos below); Murim then opens the task in that repo.\n\nRepos: {}{guess}\n\n{issue}", names.join(", "))
}

/// "Request changes" during review: fix, push, keep the PR description accurate.
pub fn changes_prompt(feedback: &str) -> String {
    format!("Review feedback on your PR: {feedback}\n\nApply it, commit (no AI attribution), and push. If the change affects the PR title or description, update them with `gh pr edit`. Keep the PR a draft. End your final message with the line MURIM_PR_OPENED.")
}

pub fn on_hook(app: &AppHandle, task: &str, ev: &Value) {
    let stop = ev["hook_event_name"] == "Stop";
    // the turn died on an API error (rate limit, overloaded, auth…): Claude Code sends this instead of Stop
    let failed = ev["hook_event_name"] == "StopFailure";
    let error = ev["error"].as_str().unwrap_or("unknown").to_string();
    let mut retry = false;
    let med = agent_only(task);
    let has_pr = stop && !med && crate::github::get_prs(app.state()).prs.contains_key(task);
    let (mut quality, mut ready, mut picked) = (None, None, None);
    let root = settings::dir(app).map(|d| settings::load(&d).repos_root).unwrap_or_default();
    let note = update(app, |g| {
        let s = g.map.get_mut(task)?;
        if ev["hook_event_name"] == "SessionEnd" && ev["session_id"].as_str() != s.session_id.as_deref() {
            return None; // a replaced Claude (e.g. the repo-pick session) exiting late; the PTY reader handles our own exit
        }
        if let Some(id) = ev["session_id"].as_str() {
            s.session_id = Some(id.to_string());
        }
        let (mut st, q) = hooks::next_status(s.status, ev);
        let was = s.status;
        if stop && !med && s.worktree.is_empty() {
            // repo-pick session in repos_root: a valid MURIM_REPO line moves the task into its worktree, else ask the user
            picked = hooks::repo_marker(ev["last_assistant_message"].as_str().unwrap_or_default(), Path::new(&root)).map(|p| (p, s.permission_mode));
            if picked.is_none() {
                st = Status::NeedsInput;
            }
        } else if failed {
            retry = std::mem::take(&mut s.tempering_turn);
            if retry {
                crate::tempering::turn_failed(&mut s.memory);
            } else {
                st = Status::NeedsInput;
            }
        } else if stop && task.starts_with("review:") {
            // a review agent opens no PR: done = it wrote its draft (Pavilion shows "review draft ready"); otherwise it asks
            if !Path::new(&s.worktree).join(".murim/review.json").exists() {
                st = Status::NeedsInput;
            }
        } else if stop && !med {
            let tt = std::mem::take(&mut s.tempering_turn);
            if tt {
                s.memory.inflight.clear();
            }
            match hooks::on_stop(s.pr_flow, ev["last_assistant_message"].as_str().unwrap_or_default(), has_pr, tt) {
                StopOutcome::Idle => {}
                StopOutcome::NeedsInput => st = Status::NeedsInput,
                StopOutcome::Quality => (s.pr_flow, quality) = (PrFlow::Reviewing, Some((s.worktree.clone(), title_of(s)))),
                StopOutcome::AwaitingReview => {
                    s.pr_flow = PrFlow::AwaitingReview;
                    ready = Some(title_of(s));
                }
            }
        }
        if stop && is_investigation(task) && crate::pavilion::investigation_outcome(ev["last_assistant_message"].as_str().unwrap_or_default(), Path::new(&root)).is_none() {
            st = Status::NeedsInput; // stopped without a plan: it's asking the user something
        }
        s.status = st;
        if st != Status::NeedsInput {
            s.questions = None;
        } else if q.is_some() {
            s.questions = q; // permission_prompt after AskUserQuestion keeps the question
        }
        (st == Status::NeedsInput && was != Status::NeedsInput).then(|| {
            let text = if failed { format!("Claude stopped on an API error ({error})") } else if stop { "Claude stopped — reply needed?".into() } else { s.questions.as_ref().and_then(|q| q[0]["question"].as_str()).or(ev["message"].as_str()).unwrap_or("Needs your input").to_string() };
            (if is_automation(&s.issue_id) { format!("{} needs you", s.name) } else { title_of(s) }, text)
        })
    });
    if let Some((t, b)) = note {
        notify(app, t, b);
    }
    if retry {
        crate::tempering::log(app, task, format!("Claude hit an API error ({error}); Tempering retries in 10 min"));
    }
    if let Some(t) = ready {
        notify(app, t, "Draft PR ready for your review".into());
    }
    if stop && is_investigation(task) {
        let (a, t, m) = (app.clone(), task.to_string(), ev["last_assistant_message"].as_str().unwrap_or_default().to_string());
        std::thread::spawn(move || crate::pavilion::investigation_done(&a, &t, &m));
    }
    if let Some((wt, title)) = quality {
        let ident = crate::inbox::find(app, task).map(|i| i.issue.identifier).unwrap_or(title); // worktree dir is the identifier
        let base = git(Path::new(&wt), &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).unwrap_or_else(|_| "the default branch".into());
        let (a, t) = (app.clone(), task.to_string());
        std::thread::spawn(move || {
            if let Err(e) = resume_task(a.clone(), t.clone(), Some(quality_prompt(&ident, &base))) {
                with_session(&a, &t, |s| s.pr_flow = PrFlow::None);
                crate::tempering::log(&a, &t, format!("Could not start quality pass: {e}"));
            }
        });
    }
    if let Some((repo, mode)) = picked {
        let (a, t) = (app.clone(), task.to_string());
        std::thread::spawn(move || {
            // take the PTY out first so its reader thread doesn't mark the new session ended
            if let Some(p) = update(&a, |g| g.ptys.remove(&t)) {
                hangup_owned(p);
            }
            let r = crate::inbox::set_issue_repo(a.clone(), a.state(), t.clone(), repo.to_string_lossy().into()).and_then(|_| start_task(a.clone(), t.clone(), mode));
            if let Err(e) = r {
                with_session(&a, &t, |s| s.status = Status::NeedsInput);
                notify(&a, t, format!("Could not start in the picked repo: {e}"));
            }
        });
    }
    if stop && !med {
        let (a, t) = (app.clone(), task.to_string());
        std::thread::spawn(move || crate::tempering::consume(&a, &t));
        crate::github::poke(app);
    }
}

// ---- environment ----

/// (PATH, absolute claude path) from the login shell, once. GUI apps on macOS don't inherit shell PATH.
/// The user's interactive login-shell environment (GUI apps don't inherit it): PATH, GITHUB_TOKEN, etc.
/// Passed whole to `claude` and `gh` so they behave like they do in the user's terminal.
pub fn shell_env() -> &'static (Vec<(String, String)>, Option<String>) {
    static E: OnceLock<(Vec<(String, String)>, Option<String>)> = OnceLock::new();
    E.get_or_init(|| {
        let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let out = Command::new(sh)
            .args(["-lic", r#"echo "MURIM_CLAUDE=$(command -v claude)"; echo __MURIM_ENV__; env -0"#])
            .stdin(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        parse_shell_env(&out)
    })
}

/// One-shot `claude -p --model haiku <prompt>` with the login-shell env. None on spawn failure, error exit or timeout.
pub fn claude_print(prompt: &str, timeout: Duration) -> Option<String> {
    use std::process::Stdio;
    let (vars, path) = shell_env();
    let mut cmd = Command::new(path.as_deref().unwrap_or("claude"));
    cmd.args(["-p", "--model", "haiku", prompt, "--disallowedTools", "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE") || k.starts_with("AI_AGENT") {
            cmd.env_remove(k);
        }
    }
    cmd.envs(vars.iter().filter(|(k, _)| !(k.starts_with("CLAUDE") || k.starts_with("AI_AGENT"))).map(|(k, v)| (k, v)));
    let mut child = cmd.spawn().map_err(|e| eprintln!("claude spawn failed: {e}")).ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait().ok()? {
            Some(s) if s.success() => break,
            Some(_) => return None,
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    // ponytail: answers are a word or a small JSON object, so reading after exit can't fill the pipe.
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout?, &mut out).ok()?;
    Some(out)
}

/// Falls back to Murim's own env if the shell produced nothing usable.
fn parse_shell_env(out: &str) -> (Vec<(String, String)>, Option<String>) {
    let claude = out.lines().find_map(|l| l.strip_prefix("MURIM_CLAUDE=")).map(str::trim).filter(|p| p.starts_with('/')).map(String::from);
    let vars: Vec<(String, String)> = out
        .split_once("__MURIM_ENV__\n")
        .map(|(_, env)| env.split('\0').filter_map(|kv| kv.split_once('=')).filter(|(k, _)| !k.is_empty() && !k.contains('\n')).map(|(k, v)| (k.into(), v.into())).collect())
        .unwrap_or_default();
    let mut vars: Vec<(String, String)> = if vars.iter().any(|(k, _)| k == "PATH") { vars } else { std::env::vars().collect() };
    vars.retain(|(k, _)| !editor_var(k));
    // git asks `gh` for GitHub credentials (after any existing helper, e.g. osxkeychain), so no editor or TTY prompt is needed.
    // ponytail: clobbers a user-set GIT_CONFIG_COUNT; append to it if anyone ever relies on that.
    vars.extend([("GIT_CONFIG_COUNT", "1"), ("GIT_CONFIG_KEY_0", "credential.https://github.com.helper"), ("GIT_CONFIG_VALUE_0", "!gh auth git-credential"), ("GIT_TERMINAL_PROMPT", "0")].map(|(k, v)| (k.into(), v.into())));
    (vars, claude)
}

/// Vars an editor's terminal injects (VS Code's GIT_ASKPASS pops a VS Code auth dialog); Murim must not inherit them.
pub fn editor_var(k: &str) -> bool {
    matches!(k, "GIT_ASKPASS" | "SSH_ASKPASS") || k.starts_with("VSCODE_") || k.starts_with("TERM_PROGRAM")
}

/// What a real terminal sets for its children. Without COLORTERM, chalk-style color detection (Claude Code, etc.) drops
/// truecolor to the 256-color cube; a GUI-launched app has no LANG, so zsh/CLIs fall back to ASCII/C locale.
fn term_env(c: &mut CommandBuilder) {
    c.env("TERM", "xterm-256color");
    c.env("COLORTERM", "truecolor");
    c.env("TERM_PROGRAM", "Murim"); // set on the child only; editor_var still strips the inherited one
    if ["LANG", "LC_ALL", "LC_CTYPE"].iter().all(|k| c.get_env(k).is_none()) {
        c.env("LANG", "en_US.UTF-8");
    }
}

// ---- worktree + hook settings ----

pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("git").arg("-C").arg(repo).args(args).envs(shell_env().0.iter().map(|(k, v)| (k, v))).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

/// Create (or reuse) ~/.murim/worktrees/<repo>/<IDENT> on `branch`.
fn ensure_worktree(home: &Path, repo: &Path, ident: &str, branch: &str) -> Result<PathBuf, String> {
    let name = repo.file_name().ok_or("bad repo path")?.to_string_lossy().into_owned();
    let wt = home.join(".murim/worktrees").join(name).join(ident);
    if wt.exists() {
        return Ok(wt);
    }
    std::fs::create_dir_all(wt.parent().unwrap()).map_err(|e| e.to_string())?;
    let _ = git(repo, &["fetch", "origin"]); // best effort
    let base = git(repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]).ok().and_then(|r| r.strip_prefix("refs/remotes/").map(String::from)).unwrap_or_else(|| "HEAD".into());
    let w = wt.to_string_lossy();
    if git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        git(repo, &["worktree", "add", &w, branch])?;
    } else {
        git(repo, &["worktree", "add", "--no-track", "-b", branch, &w, &base])?;
    }
    Ok(wt)
}

const HOOK_CMD: &str = r#"curl -s -m 5 -X POST http://127.0.0.1:$MURIM_PORT/hook -H "X-Murim-Token: $MURIM_TOKEN" -H "X-Murim-Task: $MURIM_TASK" --data-binary @- || true"#;
const EVENTS: [&str; 7] = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop", "StopFailure", "SessionEnd"];

/// Merge our hooks into an existing settings.local.json value, replacing earlier Murim entries
/// and keeping everything else.
fn merge_hooks(mut v: Value) -> Value {
    if !v.is_object() {
        v = json!({});
    }
    if !v["hooks"].is_object() {
        v["hooks"] = json!({});
    }
    for e in EVENTS {
        let mut arr = v["hooks"][e].as_array().cloned().unwrap_or_default();
        arr.retain(|m| !m.to_string().contains("MURIM_TOKEN"));
        arr.push(json!({"matcher": if e == "PreToolUse" || e == "PostToolUse" || e == "Notification" { "*" } else { "" }, "hooks": [{"type": "command", "command": HOOK_CMD}]}));
        v["hooks"][e] = Value::Array(arr);
    }
    v
}

pub(crate) fn write_hooks(wt: &Path) -> Result<(), String> {
    let f = wt.join(".claude/settings.local.json");
    let cur = std::fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    std::fs::create_dir_all(wt.join(".claude")).map_err(|e| e.to_string())?;
    std::fs::write(f, serde_json::to_string_pretty(&merge_hooks(cur)).unwrap()).map_err(|e| e.to_string())
}

pub fn exclude_settings(repo: &Path) -> Result<(), String> {
    let f = repo.join(git(repo, &["rev-parse", "--git-common-dir"])?).join("info/exclude"); // join of an absolute path replaces
    let mut cur = std::fs::read_to_string(&f).unwrap_or_default();
    let before = cur.len();
    for pat in [".claude/settings.local.json", ".murim/"] {
        if !cur.lines().any(|l| l.trim() == pat) {
            if !cur.is_empty() && !cur.ends_with('\n') {
                cur.push('\n');
            }
            cur += &format!("{pat}\n");
        }
    }
    if cur.len() == before {
        return Ok(());
    }
    std::fs::create_dir_all(f.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&f, cur).map_err(|e| e.to_string())
}

// ---- PTY ----

/// Prompt goes before the flags: --allowedTools is variadic and would swallow a trailing positional.
fn flags(m: PermissionMode) -> Vec<&'static str> {
    match m {
        // explicit: otherwise Claude Code falls back to the user's own defaultMode (e.g. auto)
        PermissionMode::Normal => vec!["--permission-mode", "default"],
        PermissionMode::Skip => vec!["--dangerously-skip-permissions"],
        PermissionMode::Allowlist => vec![
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Bash(git:*),Bash(gh:*),Bash(pnpm:*),Bash(npm:*),Bash(npx:*),Bash(yarn:*),Bash(cargo:*),Bash(make:*),Bash(node:*),Bash(python3:*),Bash(pytest:*),Bash(go:*),Bash(ls:*),Bash(cat:*),Bash(grep:*),Bash(rg:*),Bash(find:*),Bash(mkdir:*)",
        ],
    }
}

fn spawn(app: &AppHandle, issue_id: &str, head: Vec<String>) -> Result<(), String> {
    let (wt, mode, agent, cmd) = {
        let g = sessions(app).0.lock().unwrap();
        let s = g.map.get(issue_id).ok_or("unknown session")?;
        if g.ptys.contains_key(issue_id) {
            return Err("already running".into());
        }
        (s.worktree.clone(), s.permission_mode, s.agent, s.agent_command.clone())
    };
    let custom = agent == AgentKind::Custom;
    // meditation: artifacts folder for Claude (--add-dir + system prompt) or custom agents (MURIM_ARTIFACTS); also on resume
    let arts = if is_meditation(issue_id) {
        let d = settings::meditations_dir(&settings::load(&settings::dir(app)?));
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        Some(d.to_string_lossy().into_owned())
    } else {
        None
    };
    let mut local_model: Option<String> = None;
    let (vars, claude) = shell_env();
    let hk = app.state::<Hooks>();
    let pair = native_pty_system().openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut c = if custom {
        // user's command line via their login shell; the prompt goes in as "$@" so it is never shell-parsed
        let mut c = CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()));
        c.args(["-lc", &format!("{cmd} \"$@\""), "murim"]);
        c.args(head);
        c
    } else {
        let claude = claude.clone().or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/bin/claude")).filter(|p| Path::new(p).exists())).ok_or("claude not found on PATH (login shell)")?;
        let mut c = CommandBuilder::new(claude);
        c.args(head);
        if let Some(d) = &arts {
            // hooks via --settings: nothing written into repos_root
            let sys = format!("Save documents worth keeping (plans, explanations, notes) as markdown files in {d}, with a descriptive kebab-case filename and a # title.");
            c.args(["--settings", &merge_hooks(json!({})).to_string(), "--add-dir", d, "--append-system-prompt", &sys]);
        } else if is_investigation(issue_id) {
            // read-only like the repo-pick session; also on resume
            c.args(["--settings", &merge_hooks(json!({})).to_string(), "--disallowedTools", "Edit,Write,NotebookEdit"]);
        } else if is_automation(issue_id) {
            c.args(["--settings", &merge_hooks(json!({})).to_string()]); // also on resume
            let m = crate::automations::model_of(app, issue_id);
            if let Some(local) = m.strip_prefix("ollama:") {
                // Ollama speaks the Anthropic Messages API; set after the login-shell env below so it wins
                local_model = Some(settings::load(&settings::dir(app)?).pavilion.ollama_url);
                c.args(["--model", local]);
            } else if !m.is_empty() {
                c.args(["--model", &m]);
            }
        } else if wt.is_empty() {
            // repo-pick session: hooks via --settings (nothing written into repos_root), no file edits
            c.args(["--settings", &merge_hooks(json!({})).to_string(), "--disallowedTools", "Edit,Write,NotebookEdit"]);
        }
        c.args(flags(mode));
        c
    };
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE") || k == "AI_AGENT" {
            c.env_remove(k);
        }
    }
    for (k, v) in vars {
        if k == "PATH" {
            c.env(k, crate::docker::path_with_bin(v)); // docker queue wrappers first
        } else if !(k.starts_with("CLAUDE") || k == "AI_AGENT") {
            c.env(k, v);
        }
    }
    c.cwd(if wt.is_empty() { settings::load(&settings::dir(app)?).repos_root } else { wt });
    term_env(&mut c);
    c.env("MURIM_PORT", hk.port.to_string());
    c.env("MURIM_TOKEN", &hk.token);
    c.env("MURIM_TASK", issue_id);
    if let Some(url) = &local_model {
        c.env("ANTHROPIC_BASE_URL", url.trim_end_matches('/'));
        c.env("ANTHROPIC_AUTH_TOKEN", "ollama");
        c.env("ANTHROPIC_API_KEY", "");
    }
    if let (true, Some(d)) = (custom, &arts) {
        c.env("MURIM_ARTIFACTS", d);
    }
    let child = pair.slave.spawn_command(c).map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let gen = update(app, |g| {
        g.gen += 1;
        g.ptys.insert(issue_id.into(), Pty { gen: g.gen, writer, master: pair.master, child });
        g.bufs.insert(issue_id.into(), Ring::default());
        let s = g.map.get_mut(issue_id).unwrap();
        // no hooks for custom agents: "working" for as long as the process runs
        (s.pty_alive, s.status, s.questions) = (true, if custom { Status::Working } else { Status::Starting }, None);
        g.gen
    });
    let (app, id) = (app.clone(), issue_id.to_string());
    std::thread::spawn(move || {
        let (mut buf, mut tail, mut trusted, mut bypass) = ([0u8; 8192], String::new(), false, false);
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let end = sessions(&app).0.lock().unwrap().bufs.entry(id.clone()).or_default().push(&buf[..n]);
            let _ = app.emit("pty-output", json!({"issueId": id, "data": B64.encode(&buf[..n]), "end": end}));
            // dialog detection over a rolling window so a phrase split across reads still matches
            tail.push_str(&squash(&String::from_utf8_lossy(&buf[..n])));
            if !custom && !trusted && (tail.contains("trustthisfolder") || tail.contains("doyoutrust")) {
                trusted = true;
                let (a, i) = (app.clone(), id.clone());
                std::thread::spawn(move || {
                    // "No, exit" is the default: Down, then Enter selects "Yes". Real-claude smoke test: a Down sent
                    // within ms of the dialog drawing is handled twice (wraps back to "No"), so let the TUI settle first.
                    std::thread::sleep(Duration::from_millis(1000));
                    let _ = write_pty(&a, &i, b"\x1b[B");
                    std::thread::sleep(Duration::from_millis(500));
                    let _ = write_pty(&a, &i, b"\r");
                });
            }
            if !custom && !bypass && tail.contains("yesiaccept") {
                // never auto-accept bypass-permissions: surface it to the user
                bypass = true;
                let note = update(&app, |g| {
                    let s = g.map.get_mut(&id)?;
                    s.status = Status::NeedsInput;
                    Some(title_of(s))
                });
                if let Some(t) = note {
                    notify(&app, t, "Confirm bypass-permissions warning".into());
                }
            }
            if tail.len() > 512 {
                let cut = (tail.len() - 256..).find(|&i| tail.is_char_boundary(i)).unwrap();
                tail = tail[cut..].to_string();
            }
        }
        update(&app, |g| {
            if g.ptys.get(&id).is_some_and(|p| p.gen == gen) {
                g.ptys.remove(&id);
                if let Some(s) = g.map.get_mut(&id) {
                    (s.pty_alive, s.status, s.questions) = (false, Status::Ended, None);
                }
            }
        });
    });
    Ok(())
}

// ---- extra shells: plain login shells in the task's worktree, shown as terminal tabs ----

/// Shell ids are `<issueId>#<n>`; they share the PTY map, ring buffers and pty_* commands with Claude.
fn shell_prefix(issue_id: &str) -> String {
    format!("{issue_id}#")
}

#[tauri::command]
pub fn open_shell(app: AppHandle, issue_id: String) -> Result<String, String> {
    let wt = with_session(&app, &issue_id, |s| s.worktree.clone()).filter(|w| !w.is_empty()).ok_or("Start Claude first: the task has no worktree yet")?;
    let pair = native_pty_system().openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut c = CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()));
    c.arg("-l");
    for (k, v) in &shell_env().0 {
        c.env(k, v);
    }
    c.cwd(&wt);
    term_env(&mut c);
    let child = pair.slave.spawn_command(c).map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let id = {
        let mut g = sessions(&app).0.lock().unwrap();
        g.gen += 1;
        let gen = g.gen;
        let id = format!("{}{gen}", shell_prefix(&issue_id));
        g.ptys.insert(id.clone(), Pty { gen, writer, master: pair.master, child });
        g.bufs.insert(id.clone(), Ring::default());
        id
    };
    let (a, i) = (app.clone(), id.clone());
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let end = sessions(&a).0.lock().unwrap().bufs.entry(i.clone()).or_default().push(&buf[..n]);
            let _ = a.emit("pty-output", json!({"issueId": i, "data": B64.encode(&buf[..n]), "end": end}));
        }
        let mut g = sessions(&a).0.lock().unwrap();
        g.ptys.remove(&i);
        g.bufs.remove(&i);
        drop(g);
        let _ = a.emit("shells-changed", &i);
    });
    Ok(id)
}

#[tauri::command]
pub fn list_shells(state: State<Sessions>, issue_id: String) -> Vec<String> {
    let pre = shell_prefix(&issue_id);
    let mut v: Vec<String> = state.0.lock().unwrap().ptys.keys().filter(|k| k.starts_with(&pre)).cloned().collect();
    v.sort_by_key(|k| k[pre.len()..].parse::<u64>().unwrap_or(0)); // open order
    v
}

#[tauri::command]
pub fn close_shell(app: AppHandle, id: String) {
    // reader thread sees EOF, cleans up and emits shells-changed
    if id.contains('#') {
        hangup(&app, &id);
    }
}

fn write_pty(app: &AppHandle, id: &str, data: &[u8]) -> Result<(), String> {
    let mut g = sessions(app).0.lock().unwrap();
    let p = g.ptys.get_mut(id).ok_or("no running terminal")?;
    p.writer.write_all(data).and_then(|_| p.writer.flush()).map_err(|e| e.to_string())
}

// ---- commands ----

#[tauri::command]
pub fn start_task(app: AppHandle, issue_id: String, permission_mode: PermissionMode) -> Result<(), String> {
    let it = crate::inbox::find(&app, &issue_id).ok_or("unknown issue")?;
    let i = &it.issue;
    let cfg = settings::load(&settings::dir(&app)?);
    let custom = cfg.agent == AgentKind::Custom;
    if custom && cfg.agent_command.trim().is_empty() {
        return Err("Set the agent command in Settings".into());
    }
    let issue = format!("Linear issue {}: {}\n{}\n\n{}", i.identifier, i.title, i.url, i.description);
    let issue = match crate::pavilion::approved_plan(&app, &issue_id) {
        Some(p) => format!("{issue}\n\nImplementation plan (approved by the user):\n{p}"),
        None => issue,
    };
    // semantic/claude matches are guesses: Claude confirms the repo from repos_root first (custom agents have no hooks to report it)
    let guess = it.repo.as_ref().filter(|r| matches!(r.source, Source::Semantic | Source::Claude));
    let (wt, prompt) = match &it.repo {
        Some(r) if custom || guess.is_none() => {
            let branch = if i.branch_name.is_empty() { i.identifier.to_lowercase() } else { i.branch_name.clone() };
            let repo = Path::new(&r.path);
            let wt = ensure_worktree(&PathBuf::from(std::env::var("HOME").map_err(|e| e.to_string())?), repo, &i.identifier, &branch)?;
            write_hooks(&wt)?;
            exclude_settings(repo)?;
            // the MURIM_DONE marker drives Claude's hook-based flow; custom agents just get the issue
            let extra = cfg.default_prompt.trim();
            let extra = if extra.is_empty() { String::new() } else { format!("\n\nInstructions:\n{extra}") };
            (wt.to_string_lossy().into_owned(), format!("Work on {issue}{extra}{}", if custom { "" } else { DONE_SUFFIX }))
        }
        _ if custom => return Err("Pick a repo first".into()),
        _ if cfg.repos_root.is_empty() => return Err("Choose your repos folder in Settings".into()),
        _ => (String::new(), pick_prompt(&issue, &crate::repo_match::list_repos(Path::new(&cfg.repos_root)), guess)),
    };
    update(&app, |g| {
        if g.ptys.contains_key(&issue_id) {
            return Err("already running".to_string());
        }
        let old = g.map.get(&issue_id).cloned();
        g.map.insert(issue_id.clone(), SessionInfo { issue_id: issue_id.clone(), worktree: wt, session_id: None, status: Status::Starting, questions: None, pty_alive: false, permission_mode, tempering: !custom && old.as_ref().is_some_and(|o| o.tempering), tempering_off: old.as_ref().is_some_and(|o| o.tempering_off), tempering_note: String::new(), escalations: vec![], pr_flow: old.as_ref().map(|o| o.pr_flow).unwrap_or_default(), agent: cfg.agent, agent_command: cfg.agent_command.trim().to_string(), name: String::new(), tempering_turn: false, memory: old.map(|o| o.memory).unwrap_or_default() });
        Ok(())
    })?;
    spawn(&app, &issue_id, vec![prompt])
}

#[tauri::command]
pub fn resume_task(app: AppHandle, issue_id: String, prompt: Option<String>) -> Result<(), String> {
    let prompt = prompt.filter(|p| !p.is_empty());
    let (alive, sid, agent) = {
        let g = sessions(&app).0.lock().unwrap();
        let s = g.map.get(&issue_id).ok_or("unknown session")?;
        (g.ptys.contains_key(&issue_id), s.session_id.clone(), s.agent)
    };
    if alive {
        if let Some(p) = prompt {
            write_pty(&app, &issue_id, format!("\x1b[200~{p}\x1b[201~").as_bytes())?; // bracketed paste: newlines must not submit
            std::thread::sleep(Duration::from_millis(500));
            write_pty(&app, &issue_id, b"\r")?;
        }
        return Ok(());
    }
    if agent == AgentKind::Custom {
        // ponytail: no generic resume; rerun the command in the worktree with the follow-up (if any)
        return spawn(&app, &issue_id, prompt.into_iter().collect());
    }
    let mut head = match sid {
        Some(sid) => vec!["--resume".to_string(), sid],
        None if is_adopted(&issue_id) => vec![], // adopted PR: Claude's first run here
        None => return Err("no session id to resume".into()),
    };
    head.extend(prompt);
    spawn(&app, &issue_id, head)
}

#[tauri::command]
pub fn stop_task(app: AppHandle, issue_id: String) {
    // the reader thread sees EOF and marks the session ended
    hangup(&app, &issue_id);
}

/// Review-mode agent in a PR checkout. Same PTY/hook machinery as tasks, keyed `review:<owner>/<repo>/<n>`; no PR flow, no Tempering.
pub fn start_review(app: &AppHandle, id: &str, wt: &str, prompt: String) -> Result<(), String> {
    start_agent(app, id, wt, String::new(), vec![prompt])
}

/// Free-form agent in repos_root keyed `meditation:<hex>`, no initial prompt.
pub fn start_meditation_session(app: &AppHandle, id: &str, name: String, repos_root: &str) -> Result<(), String> {
    start_agent(app, id, repos_root, name, vec![])
}

/// Scheduled/manual run keyed `automation:<auto-id>:<run-id>`; prompt is the first message. `folder` "" = repos_root.
pub fn start_automation_session(app: &AppHandle, id: &str, name: String, folder: &str, prompt: &str) -> Result<(), String> {
    let cfg = settings::load(&settings::dir(app)?);
    let dir = if folder.trim().is_empty() { cfg.repos_root.clone() } else { folder.trim().to_string() };
    if dir.is_empty() {
        return Err("Set your repos folder in Settings".into());
    }
    if !Path::new(&dir).is_dir() {
        return Err(format!("Folder not found: {dir}"));
    }
    let extra = cfg.default_prompt.trim();
    let extra = if extra.is_empty() { String::new() } else { format!("\n\nInstructions:\n{extra}") };
    start_agent(app, id, &dir, name, vec![format!("{prompt}{extra}")])
}

/// Pavilion investigation in repos_root keyed `investigation:<issueId>`; the prompt asks for a plan + MURIM_REPO line.
pub fn start_investigation_session(app: &AppHandle, id: &str, name: String, repos_root: &str, prompt: String) -> Result<(), String> {
    start_agent(app, id, repos_root, name, vec![prompt])
}

pub fn pty_alive(app: &AppHandle, id: &str) -> bool {
    sessions(app).0.lock().unwrap().ptys.contains_key(id)
}

fn start_agent(app: &AppHandle, id: &str, wt: &str, name: String, head: Vec<String>) -> Result<(), String> {
    let cfg = settings::load(&settings::dir(app)?);
    if cfg.agent == AgentKind::Custom && cfg.agent_command.trim().is_empty() {
        return Err("Set the agent command in Settings".into());
    }
    update(app, |g| {
        if g.ptys.contains_key(id) {
            return Err("already running".to_string());
        }
        g.map.insert(id.into(), SessionInfo { issue_id: id.into(), worktree: wt.into(), session_id: None, status: Status::Starting, questions: None, pty_alive: false, permission_mode: cfg.default_permission_mode, tempering: false, tempering_off: false, tempering_note: String::new(), escalations: vec![], memory: Default::default(), pr_flow: PrFlow::None, agent: cfg.agent, agent_command: cfg.agent_command.trim().to_string(), name, tempering_turn: false });
        Ok(())
    })?;
    spawn(app, id, head)
}

/// Idle session for an adopted PR: nothing runs until Tempering has work for it (and an agent slot).
pub fn adopt(app: &AppHandle, id: &str, wt: &str) {
    let mode = settings::dir(app).map(|d| settings::load(&d).default_permission_mode).unwrap_or_default();
    update(app, |g| {
        g.map.entry(id.into()).or_insert_with(|| SessionInfo { issue_id: id.into(), worktree: wt.into(), session_id: None, status: Status::Ended, questions: None, pty_alive: false, permission_mode: mode, tempering: true, tempering_off: false, tempering_note: String::new(), escalations: vec![], memory: Default::default(), pr_flow: PrFlow::Ready, agent: AgentKind::Claude, agent_command: String::new(), name: String::new(), tempering_turn: false });
    });
}

/// Kill a session's PTY and drop it (review dismissed / PR closed).
pub fn forget(app: &AppHandle, id: &str) {
    update(app, |g| {
        if let Some(p) = g.ptys.remove(id) {
            hangup_owned(p);
        }
        g.map.remove(id);
        g.bufs.remove(id);
    });
}

#[tauri::command]
pub fn get_sessions(state: State<Sessions>) -> HashMap<String, SessionInfo> {
    state.0.lock().unwrap().map.clone()
}

#[tauri::command]
pub fn pty_write(app: AppHandle, issue_id: String, data: String) -> Result<(), String> {
    write_pty(&app, &issue_id, data.as_bytes())
}

/// Cmd+V in the terminal (Terminal.tsx): the clipboard as text.
#[tauri::command]
pub fn clipboard_text() -> Result<String, String> {
    let o = std::process::Command::new("pbpaste").output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

#[tauri::command]
pub fn pty_resize(state: State<Sessions>, issue_id: String, cols: u16, rows: u16) -> Result<(), String> {
    let g = state.0.lock().unwrap();
    let p = g.ptys.get(&issue_id).ok_or("no running terminal")?;
    p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_snapshot(state: State<Sessions>, issue_id: String) -> Value {
    let g = state.0.lock().unwrap();
    let Some(r) = g.bufs.get(&issue_id) else { return json!({"data": "", "end": 0}) };
    let (a, b) = r.bytes.as_slices();
    json!({"data": B64.encode([a, b].concat()), "end": r.end})
}

/// Lowercase letters only, ANSI escapes dropped: TUIs move the cursor between words
/// (e.g. `ESC[1C` instead of a space), so match phrases with their spaces removed.
fn squash(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(c) = chars.next() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if c.is_ascii_alphabetic() {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_user_settings_and_is_idempotent() {
        let existing = json!({"permissions": {"allow": ["Bash(ls)"]}, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo mine"}]}]}});
        let once = merge_hooks(existing);
        let twice = merge_hooks(once.clone());
        assert_eq!(once, twice);
        assert_eq!(twice["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(twice["hooks"]["Stop"][0]["hooks"][0]["command"], "echo mine");
        assert_eq!(twice["hooks"]["SessionEnd"].as_array().unwrap().len(), 1);
        assert_eq!(merge_hooks(json!("junk"))["hooks"].as_object().unwrap().len(), 7);
    }

    #[test]
    fn ring_keeps_the_tail_and_counts_everything() {
        let mut r = Ring::default();
        assert_eq!(r.push(b"abc"), 3);
        assert_eq!(r.push(&vec![b'x'; RING]), 3 + RING as u64);
        assert_eq!((r.bytes.len(), r.bytes[0]), (RING, b'x')); // oldest bytes dropped, offset still counts them
    }

    #[test]
    fn shell_env_parse_ignores_noise() {
        let out = "motd\nMURIM_CLAUDE=/x/claude\n__MURIM_ENV__\nPATH=/a:/b\0GITHUB_TOKEN=t=1\0";
        let (vars, claude) = parse_shell_env(out);
        assert_eq!(claude.as_deref(), Some("/x/claude"));
        assert!(vars.contains(&("PATH".into(), "/a:/b".into())));
        assert!(vars.contains(&("GITHUB_TOKEN".into(), "t=1".into())));
        let (vars, _) = parse_shell_env("__MURIM_ENV__\nPATH=/a\0GIT_ASKPASS=/vscode\0VSCODE_GIT_IPC_HANDLE=x\0");
        assert!(!vars.iter().any(|(k, _)| editor_var(k)));
        assert!(vars.contains(&("GIT_CONFIG_VALUE_0".into(), "!gh auth git-credential".into())));
        let (vars, claude) = parse_shell_env("MURIM_CLAUDE=\n"); // no marker → own env
        assert!(claude.is_none() && vars.iter().any(|(k, _)| k == "PATH"));
    }

    #[test]
    fn worktree_and_exclude_on_temp_repo() {
        let root = std::env::temp_dir().join(format!("murim-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("myrepo");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |a: &[&str]| git(&repo, a).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "i"]);
        let wt = ensure_worktree(&root, &repo, "ENG-1", "eng-1-thing").unwrap();
        assert_eq!(wt, root.join(".murim/worktrees/myrepo/ENG-1"));
        assert_eq!(git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(), "eng-1-thing");
        assert_eq!(ensure_worktree(&root, &repo, "ENG-1", "eng-1-thing").unwrap(), wt); // reuse
        std::fs::remove_dir_all(&wt).unwrap();
        g(&["worktree", "prune"]);
        ensure_worktree(&root, &repo, "ENG-1", "eng-1-thing").unwrap(); // existing branch reused
        write_hooks(&wt).unwrap();
        exclude_settings(&repo).unwrap();
        exclude_settings(&repo).unwrap();
        let ex = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(ex.matches(".claude/settings.local.json").count() == 1 && ex.matches(".murim/").count() == 1);
        assert!(git(&wt, &["status", "--porcelain"]).unwrap().is_empty()); // ignored
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompts() {
        assert!(DONE_SUFFIX.ends_with("the line MURIM_DONE."));
        let p = quality_prompt("ENG-1", "origin/main");
        for k in ["/ponytail:ponytail-review", "vs origin/main", "gh pr create --draft", "Do NOT mark it ready", "Fixes ENG-1", "not available", "no AI attribution"] {
            assert!(p.contains(k), "{k}");
        }
        assert!(p.ends_with("line MURIM_PR_OPENED.") && changes_prompt("x").ends_with("line MURIM_PR_OPENED."));
        let repos = [crate::repo_match::Repo { name: "alpha".into(), path: "/r/alpha".into() }, crate::repo_match::Repo { name: "beta".into(), path: "/r/beta".into() }];
        let g = crate::repo_match::RepoMatch { path: "/r/beta".into(), source: Source::Semantic };
        let p = pick_prompt("Linear issue E-1", &repos, Some(&g));
        for k in ["MURIM_REPO: <repo-name>", "Repos: alpha, beta", "Likely repo: beta (guess)", "Do NOT edit", "Linear issue E-1"] {
            assert!(p.contains(k), "{k}");
        }
    }

    #[test]
    fn squash_strips_ansi_and_spacing() {
        assert!(squash("Is this a project you \x1b[1Ctrust\x1b[1Cthis\x1b[22;3Hfolder?").contains("trustthisfolder"));
        assert!(squash("\x1b[32mYes, I accept\x1b[0m").contains("yesiaccept"));
    }
}
