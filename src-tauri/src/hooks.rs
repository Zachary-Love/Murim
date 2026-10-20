use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Starting,
    Working,
    NeedsInput,
    Idle,
    Ended,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum PrFlow {
    #[default]
    None,
    Reviewing,
    /// Claude opened (or updated) the draft PR; waiting for the user to accept it.
    AwaitingReview,
    /// legacy (pre-review flow): draft PR opened; treated like AwaitingReview
    Opened,
    Ready,
}

#[derive(PartialEq, Debug)]
pub enum StopOutcome {
    Idle,
    NeedsInput,
    Quality,
    AwaitingReview,
}

/// Deterministic post-turn rule, from the Stop payload's last message.
/// Tempering-invoked turns are exempt (no marker needed). DONE only triggers the quality pass once.
/// Marker counts only as the message's last non-empty line (ignoring markdown `*`/`` ` ``), so prose that
/// merely mentions it ("not done, so no MURIM_DONE yet") doesn't trigger anything.
fn last_line(msg: &str) -> Option<&str> {
    msg.lines().rev().map(|l| l.trim().trim_matches(|c| c == '*' || c == '`').trim()).find(|l| !l.is_empty())
}

fn ends_with_marker(msg: &str, marker: &str) -> bool {
    last_line(msg) == Some(marker)
}

/// Repo-pick turn: last line `MURIM_REPO: <name>` → that repo dir under `root`. Trust boundary (the name comes from
/// an agent that read issue text): a bare directory name only, no paths, and it must be a git repo there.
pub fn repo_marker(msg: &str, root: &Path) -> Option<PathBuf> {
    let name = last_line(msg)?.strip_prefix("MURIM_REPO:")?.trim().trim_matches('`');
    let bare = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
    bare.then(|| root.join(name)).filter(|p| p.join(".git").exists())
}

pub fn on_stop(flow: PrFlow, last_msg: &str, has_pr: bool, tempering_turn: bool) -> StopOutcome {
    if tempering_turn || flow == PrFlow::Ready {
        StopOutcome::Idle
    } else if ends_with_marker(last_msg, "MURIM_PR_OPENED") {
        StopOutcome::AwaitingReview // first open, or updated after requested changes
    } else if matches!(flow, PrFlow::AwaitingReview | PrFlow::Opened) {
        StopOutcome::Idle // follow-up chat during review; the Review tab stays up
    } else if ends_with_marker(last_msg, "MURIM_DONE") {
        if flow == PrFlow::None && !has_pr { StopOutcome::Quality } else { StopOutcome::Idle }
    } else if has_pr {
        StopOutcome::Idle
    } else {
        StopOutcome::NeedsInput
    }
}

pub struct Hooks {
    pub port: u16,
    pub token: String,
}

/// Status derivation. Returns the new status and, for AskUserQuestion, its `questions`.
/// Stop -> idle unconditionally: Stop only fires when the turn ends, which can't happen while a
/// question/permission prompt is pending (the answer's PostToolUse arrives first).
/// idle_prompt while already idle stays idle (turn is over, nothing is being asked).
/// Other notification types (e.g. auth_success) leave the status alone.
pub fn next_status(cur: Status, ev: &Value) -> (Status, Option<Value>) {
    match ev["hook_event_name"].as_str().unwrap_or_default() {
        "UserPromptSubmit" | "PostToolUse" => (Status::Working, None),
        "PreToolUse" if ev["tool_name"] == "AskUserQuestion" => (Status::NeedsInput, Some(ev["tool_input"]["questions"].clone()).filter(|q| !q.is_null())),
        "PreToolUse" => (Status::Working, None),
        "Notification" => match ev["notification_type"].as_str() {
            Some("permission_prompt") => (Status::NeedsInput, None),
            Some("idle_prompt") if cur != Status::Idle => (Status::NeedsInput, None),
            _ => (cur, None),
        },
        "Stop" | "StopFailure" => (Status::Idle, None), // StopFailure: the turn died on an API error
        "SessionEnd" => (Status::Ended, None),
        _ => (cur, None),
    }
}

/// Bind 127.0.0.1:<random port>, serve hooks on a thread. Requests without the token get 401.
pub fn start(app: AppHandle) -> Result<Hooks, String> {
    let srv = tiny_http::Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = srv.server_addr().to_ip().ok_or("no port")?.port();
    let token = format!("{:032x}", rand::random::<u128>());
    let tok = token.clone();
    std::thread::spawn(move || {
        for mut r in srv.incoming_requests() {
            let hv = |n: &'static str| r.headers().iter().find(|h| h.field.equiv(n)).map(|h| h.value.to_string()).unwrap_or_default();
            if hv("X-Murim-Token") != tok {
                let _ = r.respond(tiny_http::Response::empty(401));
                continue;
            }
            let task = hv("X-Murim-Task");
            let mut body = String::new();
            let _ = r.as_reader().take(4 << 20).read_to_string(&mut body);
            if let Ok(ev) = serde_json::from_str::<Value>(&body) {
                crate::sessions::on_hook(&app, &task, &ev);
            }
            let _ = r.respond(tiny_http::Response::empty(200));
        }
    });
    Ok(Hooks { port, token })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use Status::*;

    #[test]
    fn stop_rules() {
        use PrFlow as F;
        use StopOutcome as O;
        assert_eq!(on_stop(F::None, "x\nMURIM_DONE", false, false), O::Quality);
        assert_eq!(on_stop(F::None, "x\nMURIM_DONE", true, false), O::Idle); // PR already exists
        assert_eq!(on_stop(F::Reviewing, "MURIM_DONE", false, false), O::Idle); // no re-prompt
        assert_eq!(on_stop(F::Reviewing, "x\nMURIM_PR_OPENED", true, false), O::AwaitingReview);
        assert_eq!(on_stop(F::AwaitingReview, "x\nMURIM_PR_OPENED", true, false), O::AwaitingReview); // after requested changes
        assert_eq!(on_stop(F::AwaitingReview, "Which name do you prefer?", true, false), O::Idle);
        assert_eq!(on_stop(F::Ready, "MURIM_PR_OPENED", true, false), O::Idle); // accepted already
        assert_eq!(on_stop(F::None, "Which API?", false, false), O::NeedsInput);
        assert_eq!(on_stop(F::None, "anything", false, true), O::Idle); // tempering turn
    }

    #[test]
    fn transitions() {
        let n = |cur, v| next_status(cur, &v);
        assert_eq!(n(Starting, json!({"hook_event_name":"UserPromptSubmit"})).0, Working);
        assert_eq!(n(Idle, json!({"hook_event_name":"PreToolUse","tool_name":"Bash"})).0, Working);
        assert_eq!(n(NeedsInput, json!({"hook_event_name":"PostToolUse","tool_name":"AskUserQuestion"})), (Working, None));
        let q = json!([{"question":"Which?","options":[]}]);
        assert_eq!(n(Working, json!({"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":q}})), (NeedsInput, Some(q)));
        assert_eq!(n(Working, json!({"hook_event_name":"Notification","notification_type":"permission_prompt"})), (NeedsInput, None));
        assert_eq!(n(Working, json!({"hook_event_name":"Notification","notification_type":"idle_prompt"})).0, NeedsInput);
        assert_eq!(n(Idle, json!({"hook_event_name":"Notification","notification_type":"idle_prompt"})).0, Idle);
        assert_eq!(n(Working, json!({"hook_event_name":"Notification","notification_type":"auth_success"})).0, Working);
        assert_eq!(n(Working, json!({"hook_event_name":"Stop"})).0, Idle);
        assert_eq!(n(Working, json!({"hook_event_name":"StopFailure","error":"rate_limit"})).0, Idle);
        assert_eq!(n(Idle, json!({"hook_event_name":"SessionEnd"})).0, Ended);
        assert_eq!(n(Idle, json!({"foo":1})).0, Idle);
    }

    #[test]
    fn marker_must_be_last_line() {
        assert!(ends_with_marker("All done.\n\n**MURIM_DONE**\n", "MURIM_DONE"));
        assert!(!ends_with_marker("Not finished, so no MURIM_DONE yet. Which API should I use?", "MURIM_DONE"));
        assert_eq!(on_stop(PrFlow::None, "I'll print MURIM_DONE when finished. Question?", false, false), StopOutcome::NeedsInput);
    }

    #[test]
    fn repo_marker_validates_name() {
        let root = std::env::temp_dir().join(format!("murim-pick-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/.git")).unwrap();
        std::fs::create_dir_all(root.join("x/.git")).unwrap();
        assert_eq!(repo_marker("It's the app.\n**MURIM_REPO: app**", &root), Some(root.join("app")));
        assert_eq!(repo_marker("MURIM_REPO: `app`", &root), Some(root.join("app")));
        for bad in ["MURIM_REPO: ../x", "MURIM_REPO: a/b", "MURIM_REPO: nope", "MURIM_REPO: ..", "MURIM_REPO:", "MURIM_REPO: app\nWhich one?", &format!("MURIM_REPO: {}", root.join("app").display())] {
            assert_eq!(repo_marker(bad, &root), None, "{bad}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
