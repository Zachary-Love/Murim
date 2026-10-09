use crate::hooks::{self, Hooks, Status};
use crate::settings::{self, PermissionMode};
use crate::tempering::{Escalation, Memory, ReplyMode};
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
    #[serde(default)]
    pub reply_mode: ReplyMode,
    #[serde(default)]
    pub escalations: Vec<Escalation>,
    #[serde(default)]
    pub memory: Memory,
}

struct Pty {
    gen: u64,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, SessionInfo>,
    ptys: HashMap<String, Pty>,
    bufs: HashMap<String, VecDeque<u8>>,
    gen: u64,
}

#[derive(Default)]
pub struct Sessions(Mutex<Inner>);

fn sessions(app: &AppHandle) -> &Sessions {
    app.state::<Sessions>().inner()
}

/// Mutate under the lock, then persist + emit the full map.
fn update<R>(app: &AppHandle, f: impl FnOnce(&mut Inner) -> R) -> R {
    let mut g = sessions(app).0.lock().unwrap();
    let r = f(&mut g);
    if let Ok(d) = settings::dir(app) {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("sessions.json"), serde_json::to_string_pretty(&g.map).unwrap_or_default());
    }
    let _ = app.emit("sessions-updated", &g.map);
    r
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

pub fn kill_all(app: &AppHandle) {
    for p in sessions(app).0.lock().unwrap().ptys.values_mut() {
        let _ = p.child.kill();
    }
}

// ---- hooks -> status ----

pub fn title_of(s: &SessionInfo) -> String {
    Path::new(&s.worktree).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| s.issue_id.clone())
}

pub fn notify(app: &AppHandle, title: String, body: String) {
    let _ = app.notification().builder().title(title).body(body).show();
}

pub fn on_hook(app: &AppHandle, task: &str, ev: &Value) {
    let note = update(app, |g| {
        let s = g.map.get_mut(task)?;
        if let Some(id) = ev["session_id"].as_str() {
            s.session_id = Some(id.to_string());
        }
        let (st, q) = hooks::next_status(s.status, ev);
        let was = s.status;
        s.status = st;
        if st != Status::NeedsInput {
            s.questions = None;
        } else if q.is_some() {
            s.questions = q; // permission_prompt after AskUserQuestion keeps the question
        }
        (st == Status::NeedsInput && was != Status::NeedsInput).then(|| {
            let text = s.questions.as_ref().and_then(|q| q[0]["question"].as_str()).or(ev["message"].as_str()).unwrap_or("Needs your input");
            (title_of(s), text.to_string())
        })
    });
    if let Some((t, b)) = note {
        notify(app, t, b);
    }
    if ev["hook_event_name"] == "Stop" {
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

/// Falls back to Murim's own env if the shell produced nothing usable.
fn parse_shell_env(out: &str) -> (Vec<(String, String)>, Option<String>) {
    let claude = out.lines().find_map(|l| l.strip_prefix("MURIM_CLAUDE=")).map(str::trim).filter(|p| p.starts_with('/')).map(String::from);
    let vars: Vec<(String, String)> = out
        .split_once("__MURIM_ENV__\n")
        .map(|(_, env)| env.split('\0').filter_map(|kv| kv.split_once('=')).filter(|(k, _)| !k.is_empty() && !k.contains('\n')).map(|(k, v)| (k.into(), v.into())).collect())
        .unwrap_or_default();
    (if vars.iter().any(|(k, _)| k == "PATH") { vars } else { std::env::vars().collect() }, claude)
}

// ---- worktree + hook settings ----

pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("git").arg("-C").arg(repo).args(args).output().map_err(|e| e.to_string())?;
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
const EVENTS: [&str; 6] = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop", "SessionEnd"];

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

fn write_hooks(wt: &Path) -> Result<(), String> {
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
        PermissionMode::Normal => vec![],
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
    let (wt, mode) = {
        let g = sessions(app).0.lock().unwrap();
        let s = g.map.get(issue_id).ok_or("unknown session")?;
        if g.ptys.contains_key(issue_id) {
            return Err("already running".into());
        }
        (s.worktree.clone(), s.permission_mode)
    };
    let (vars, claude) = shell_env();
    let claude = claude.clone().or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/bin/claude")).filter(|p| Path::new(p).exists())).ok_or("claude not found on PATH (login shell)")?;
    let hk = app.state::<Hooks>();
    let pair = native_pty_system().openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut c = CommandBuilder::new(claude);
    c.args(head);
    c.args(flags(mode));
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE") || k == "AI_AGENT" {
            c.env_remove(k);
        }
    }
    for (k, v) in vars {
        if !(k.starts_with("CLAUDE") || k == "AI_AGENT") {
            c.env(k, v);
        }
    }
    c.cwd(&wt);
    c.env("TERM", "xterm-256color");
    c.env("MURIM_PORT", hk.port.to_string());
    c.env("MURIM_TOKEN", &hk.token);
    c.env("MURIM_TASK", issue_id);
    let child = pair.slave.spawn_command(c).map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let gen = update(app, |g| {
        g.gen += 1;
        g.ptys.insert(issue_id.into(), Pty { gen: g.gen, writer, master: pair.master, child });
        g.bufs.insert(issue_id.into(), VecDeque::new());
        let s = g.map.get_mut(issue_id).unwrap();
        (s.pty_alive, s.status, s.questions) = (true, Status::Starting, None);
        g.gen
    });
    let (app, id) = (app.clone(), issue_id.to_string());
    std::thread::spawn(move || {
        let (mut buf, mut tail, mut trusted, mut bypass) = ([0u8; 8192], String::new(), false, false);
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            {
                let mut g = sessions(&app).0.lock().unwrap();
                let b = g.bufs.entry(id.clone()).or_default();
                b.extend(&buf[..n]);
                let over = b.len().saturating_sub(RING);
                b.drain(..over);
            }
            let _ = app.emit("pty-output", json!({"issueId": id, "data": B64.encode(&buf[..n])}));
            // dialog detection over a rolling window so a phrase split across reads still matches
            tail.push_str(&squash(&String::from_utf8_lossy(&buf[..n])));
            if !trusted && (tail.contains("trustthisfolder") || tail.contains("doyoutrust")) {
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
            if !bypass && tail.contains("yesiaccept") {
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

fn write_pty(app: &AppHandle, id: &str, data: &[u8]) -> Result<(), String> {
    let mut g = sessions(app).0.lock().unwrap();
    let p = g.ptys.get_mut(id).ok_or("no running terminal")?;
    p.writer.write_all(data).and_then(|_| p.writer.flush()).map_err(|e| e.to_string())
}

// ---- commands ----

#[tauri::command]
pub fn start_task(app: AppHandle, issue_id: String, permission_mode: PermissionMode) -> Result<(), String> {
    let it = crate::inbox::find(&app, &issue_id).ok_or("unknown issue")?;
    let repo = it.repo.ok_or("no repo for this issue")?;
    let i = &it.issue;
    let branch = if i.branch_name.is_empty() { i.identifier.to_lowercase() } else { i.branch_name.clone() };
    let repo = Path::new(&repo.path);
    let home = PathBuf::from(std::env::var("HOME").map_err(|e| e.to_string())?);
    let wt = ensure_worktree(&home, repo, &i.identifier, &branch)?;
    write_hooks(&wt)?;
    exclude_settings(repo)?;
    let prompt = format!("Work on Linear issue {}: {}\n{}\n\n{}", i.identifier, i.title, i.url, i.description);
    update(&app, |g| {
        if g.ptys.contains_key(&issue_id) {
            return Err("already running".to_string());
        }
        let old = g.map.get(&issue_id).cloned();
        g.map.insert(issue_id.clone(), SessionInfo { issue_id: issue_id.clone(), worktree: wt.to_string_lossy().into(), session_id: None, status: Status::Starting, questions: None, pty_alive: false, permission_mode, tempering: old.as_ref().is_some_and(|o| o.tempering), reply_mode: old.as_ref().map(|o| o.reply_mode).unwrap_or_default(), escalations: vec![], memory: old.map(|o| o.memory).unwrap_or_default() });
        Ok(())
    })?;
    spawn(&app, &issue_id, vec![prompt])
}

#[tauri::command]
pub fn resume_task(app: AppHandle, issue_id: String, prompt: Option<String>) -> Result<(), String> {
    let prompt = prompt.filter(|p| !p.is_empty());
    let (alive, sid) = {
        let g = sessions(&app).0.lock().unwrap();
        (g.ptys.contains_key(&issue_id), g.map.get(&issue_id).ok_or("unknown session")?.session_id.clone())
    };
    if alive {
        if let Some(p) = prompt {
            write_pty(&app, &issue_id, format!("\x1b[200~{p}\x1b[201~").as_bytes())?; // bracketed paste: newlines must not submit
            std::thread::sleep(Duration::from_millis(500));
            write_pty(&app, &issue_id, b"\r")?;
        }
        return Ok(());
    }
    let mut head = vec!["--resume".to_string(), sid.ok_or("no session id to resume")?];
    head.extend(prompt);
    spawn(&app, &issue_id, head)
}

#[tauri::command]
pub fn stop_task(app: AppHandle, issue_id: String) {
    // the reader thread sees EOF and marks the session ended
    if let Some(p) = sessions(&app).0.lock().unwrap().ptys.get_mut(&issue_id) {
        let _ = p.child.kill();
    }
}

#[tauri::command]
pub fn get_sessions(state: State<Sessions>) -> HashMap<String, SessionInfo> {
    state.0.lock().unwrap().map.clone()
}

#[tauri::command]
pub fn pty_write(app: AppHandle, issue_id: String, data: String) -> Result<(), String> {
    write_pty(&app, &issue_id, data.as_bytes())
}

#[tauri::command]
pub fn pty_resize(state: State<Sessions>, issue_id: String, cols: u16, rows: u16) -> Result<(), String> {
    let g = state.0.lock().unwrap();
    let p = g.ptys.get(&issue_id).ok_or("no running terminal")?;
    p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_snapshot(state: State<Sessions>, issue_id: String) -> String {
    let g = state.0.lock().unwrap();
    let (a, b) = g.bufs.get(&issue_id).map(|b| b.as_slices()).unwrap_or_default();
    B64.encode([a, b].concat())
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
        assert_eq!(merge_hooks(json!("junk"))["hooks"].as_object().unwrap().len(), 6);
    }

    #[test]
    fn shell_env_parse_ignores_noise() {
        let out = "motd\nMURIM_CLAUDE=/x/claude\n__MURIM_ENV__\nPATH=/a:/b\0GITHUB_TOKEN=t=1\0";
        let (vars, claude) = parse_shell_env(out);
        assert_eq!(claude.as_deref(), Some("/x/claude"));
        assert!(vars.contains(&("PATH".into(), "/a:/b".into())));
        assert!(vars.contains(&("GITHUB_TOKEN".into(), "t=1".into())));
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
    fn squash_strips_ansi_and_spacing() {
        assert!(squash("Is this a project you \x1b[1Ctrust\x1b[1Cthis\x1b[22;3Hfolder?").contains("trustthisfolder"));
        assert!(squash("\x1b[32mYes, I accept\x1b[0m").contains("yesiaccept"));
    }
}
