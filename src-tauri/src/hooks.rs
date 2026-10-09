use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
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
        "Stop" => (Status::Idle, None),
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
        assert_eq!(n(Idle, json!({"hook_event_name":"SessionEnd"})).0, Ended);
        assert_eq!(n(Idle, json!({"foo":1})).0, Idle);
    }
}
