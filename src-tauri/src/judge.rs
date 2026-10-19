//! Pavilion's judgement calls: a small local model (Ollama) answers, Claude checks. Pure prompt/parse/policy + the calls.
use crate::settings::{CheckMode, PavilionSettings};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const LOW: f32 = 0.7;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Q {
    /// clear enough to investigate, or needs the user?
    Ready,
    /// waits on another board issue that Linear doesn't record?
    Blocker,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum By {
    Local,
    Claude,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub answer: String,
    pub reason: String,
    pub confidence: f32,
    pub by: By,
    /// pavilion::fingerprint of the issue this answers; a changed issue is asked again
    pub fingerprint: String,
}

/// Allowed answers. Blocker: the other board identifiers, or "none".
pub fn allowed(q: Q, board: &[String], me: &str) -> Vec<String> {
    match q {
        Q::Ready => vec!["ready".into(), "needsUser".into()],
        Q::Blocker => board.iter().filter(|b| *b != me).cloned().chain(["none".to_string()]).collect(),
    }
}

pub fn prompt(q: Q, issue: &str, board: &[(String, String)], me: &str) -> String {
    let rules = r#"Reply with only a JSON object: {"answer": <one allowed answer>, "reason": <one short sentence>, "confidence": <number from 0 to 1>}."#;
    match q {
        Q::Ready => format!("You triage software issues for a coding agent. Is this issue clear enough for an agent to investigate the code and write an implementation plan, or does it need the user first (missing requirements, a decision only a person can make, or a request to wait)? Allowed answers: \"ready\", \"needsUser\".\n{rules}\n\nIssue:\n{issue}"),
        Q::Blocker => {
            let others: Vec<String> = board.iter().filter(|(id, _)| id != me).map(|(id, t)| format!("{id}: {t}")).collect();
            format!("Does this issue say it must wait until one of the other issues below is finished? Only name an issue if the text says so; otherwise answer \"none\". Allowed answers: the identifier of one issue below, or \"none\".\n{rules}\n\nOther issues:\n{}\n\nIssue:\n{issue}", others.join("\n"))
        }
    }
}

/// The first `{…}` in the reply (models wrap JSON in prose or fences). The answer must be allowed; for Blocker any
/// other id means "none". Confidence is clamped to 0..1.
pub fn parse(raw: &str, allowed: &[String]) -> Option<(String, String, f32)> {
    let (a, b) = (raw.find('{')?, raw.rfind('}')?);
    let v: Value = serde_json::from_str(raw.get(a..=b)?).ok()?;
    let mut ans = v["answer"].as_str()?.trim().to_string();
    if !allowed.contains(&ans) {
        if !allowed.iter().any(|x| x == "none") {
            return None;
        }
        ans = "none".into();
    }
    let conf = v["confidence"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    Some((ans, v["reason"].as_str().unwrap_or_default().trim().to_string(), conf))
}

/// Whether Claude must check a local verdict before Pavilion acts on it.
pub fn needs_check(mode: CheckMode, q: Q, answer: &str, confidence: f32) -> bool {
    match mode {
        CheckMode::Off => false,
        CheckMode::LowConfidence => confidence < LOW,
        CheckMode::Always => !(q == Q::Blocker && answer == "none"), // "none" moves no card
    }
}

fn schema(allowed: &[String]) -> Value {
    json!({"type": "object", "properties": {"answer": {"type": "string", "enum": allowed}, "reason": {"type": "string"}, "confidence": {"type": "number"}}, "required": ["answer", "reason", "confidence"]})
}

/// Ollama /api/chat with structured output; returns the message content.
fn ollama(url: &str, model: &str, prompt: &str, allowed: &[String]) -> Result<String, String> {
    let body = json!({"model": model, "messages": [{"role": "user", "content": prompt}], "stream": false, "format": schema(allowed), "options": {"temperature": 0}});
    let res = match ureq::post(&format!("{}/api/chat", url.trim_end_matches('/'))).timeout(Duration::from_secs(60)).send_json(body) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => return Err(format!("Ollama: {}", r.into_string().unwrap_or_default().trim())), // e.g. model not pulled
        Err(e) => return Err(format!("Ollama not reachable at {url}: {e}")),
    };
    let v: Value = res.into_json().map_err(|e| format!("Ollama: {e}"))?;
    v["message"]["content"].as_str().map(String::from).ok_or_else(|| "Ollama: no message in the reply".into())
}

pub struct Outcome {
    /// None: no usable answer (Claude failed); retry later
    pub verdict: Option<Verdict>,
    /// the local model answered with valid JSON
    pub local_ok: bool,
}

/// Local answer first; Claude checks when the policy says so, or answers alone when the local model failed.
pub fn ask(cfg: &PavilionSettings, q: Q, prompt: &str, allowed: &[String], fp: &str, claude: impl Fn(&str) -> Option<String>) -> Outcome {
    let local = ollama(&cfg.ollama_url, &cfg.model, prompt, allowed).ok().and_then(|r| parse(&r, allowed));
    let local_ok = local.is_some();
    let v = |(answer, reason, confidence): (String, String, f32), by| Verdict { answer, reason, confidence, by, fingerprint: fp.to_string() };
    match local {
        Some(l) if !needs_check(cfg.claude_check, q, &l.0, l.2) => Outcome { verdict: Some(v(l, By::Local)), local_ok },
        l => {
            let note = l.map(|(a, r, c)| format!("\n\nA small local model answered: {}", json!({"answer": a, "reason": r, "confidence": c}))).unwrap_or_default();
            let checked = claude(&format!("{prompt}{note}\n\nGive your own answer in the same JSON format.")).and_then(|r| parse(&r, allowed));
            Outcome { verdict: checked.map(|c| v(c, By::Claude)), local_ok }
        }
    }
}

/// Settings → Test: one tiny structured call.
pub fn test_model(url: &str, model: &str) -> Result<String, String> {
    let ok = vec!["ok".to_string()];
    let r = ollama(url, model, r#"Reply with the JSON {"answer": "ok", "reason": "test", "confidence": 1}."#, &ok)?;
    parse(&r, &ok).map(|_| format!("{model} answered")).ok_or(format!("Unexpected reply from {model}: {r}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CheckMode, PavilionSettings};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allowed_and_prompt_exclude_self() {
        assert_eq!(allowed(Q::Ready, &ids(&["MUR-1"]), "MUR-1"), ids(&["ready", "needsUser"]));
        assert_eq!(allowed(Q::Blocker, &ids(&["MUR-1", "MUR-2"]), "MUR-1"), ids(&["MUR-2", "none"]));
        let p = prompt(Q::Blocker, "MUR-1 t", &[("MUR-1".into(), "self".into()), ("MUR-2".into(), "API migration".into())], "MUR-1");
        assert!(p.contains("MUR-2: API migration") && !p.contains("MUR-1: self"));
    }

    #[test]
    fn parse_is_forgiving() {
        let r = ids(&["ready", "needsUser"]);
        assert_eq!(parse("```json\n{\"answer\": \"ready\", \"reason\": \"clear\", \"confidence\": 0.8}\n```", &r), Some(("ready".into(), "clear".into(), 0.8)));
        assert_eq!(parse("Sure: {\"answer\": \"needsUser\", \"reason\": \"x\", \"confidence\": 7}", &r).unwrap().2, 1.0); // clamped
        assert_eq!(parse("{\"answer\": \"maybe\", \"reason\": \"x\", \"confidence\": 0.5}", &r), None); // not allowed
        assert_eq!(parse("ready", &r), None);
        assert_eq!(parse("} {", &r), None);
        // blocker: an id that isn't an allowed answer (not on the board, or itself) means none
        let b = ids(&["MUR-2", "none"]);
        assert_eq!(parse("{\"answer\": \"MUR-99\", \"reason\": \"x\", \"confidence\": 0.9}", &b).unwrap().0, "none");
    }

    #[test]
    fn check_policy() {
        assert!(needs_check(CheckMode::Always, Q::Ready, "ready", 0.99));
        assert!(needs_check(CheckMode::Always, Q::Blocker, "MUR-2", 0.99));
        assert!(!needs_check(CheckMode::Always, Q::Blocker, "none", 0.1)); // "none" moves nothing
        assert!(needs_check(CheckMode::LowConfidence, Q::Ready, "ready", 0.69));
        assert!(!needs_check(CheckMode::LowConfidence, Q::Ready, "ready", 0.7));
        assert!(!needs_check(CheckMode::Off, Q::Ready, "ready", 0.0));
    }

    fn stub(body: &'static str) -> String {
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", srv.server_addr().to_ip().unwrap().port());
        std::thread::spawn(move || {
            for r in srv.incoming_requests() {
                let _ = r.respond(tiny_http::Response::from_string(body));
            }
        });
        url
    }

    fn cfg(url: &str, check: CheckMode) -> PavilionSettings {
        PavilionSettings { ollama_url: url.into(), claude_check: check, ..Default::default() }
    }

    const READY: &str = r#"{"message": {"content": "{\"answer\": \"ready\", \"reason\": \"clear\", \"confidence\": 0.9}"}}"#;
    const DOWN: &str = "http://127.0.0.1:1";

    #[test]
    fn ask_local_then_claude() {
        let url = stub(READY);
        let a = allowed(Q::Ready, &[], "MUR-1");
        let o = ask(&cfg(&url, CheckMode::Off), Q::Ready, "p", &a, "fp", |_| panic!("Off must not call Claude"));
        assert_eq!((o.verdict.unwrap().by, o.local_ok), (By::Local, true));
        let o = ask(&cfg(&url, CheckMode::LowConfidence), Q::Ready, "p", &a, "fp", |_| panic!("0.9 is confident"));
        assert_eq!(o.verdict.unwrap().by, By::Local);
        let seen = std::cell::RefCell::new(String::new());
        let o = ask(&cfg(&url, CheckMode::Always), Q::Ready, "p", &a, "fp", |p| {
            *seen.borrow_mut() = p.to_string();
            Some(r#"{"answer": "needsUser", "reason": "no spec", "confidence": 0.8}"#.into())
        });
        let v = o.verdict.unwrap();
        assert_eq!((v.answer.as_str(), v.by, v.fingerprint.as_str()), ("needsUser", By::Claude, "fp"));
        assert!(seen.borrow().contains("local model answered"));
        // Ollama down: Claude answers directly (even with Off), and the header learns the local model is offline
        let o = ask(&cfg(DOWN, CheckMode::Off), Q::Ready, "p", &a, "fp", |_| Some(r#"{"answer": "ready", "reason": "ok", "confidence": 1}"#.into()));
        assert_eq!((o.verdict.unwrap().by, o.local_ok), (By::Claude, false));
        // both fail: no verdict, retry later
        assert!(ask(&cfg(DOWN, CheckMode::Always), Q::Ready, "p", &a, "fp", |_| None).verdict.is_none());
        // a local reply that isn't the JSON counts as offline
        let bad = stub(r#"{"message": {"content": "sure! ready"}}"#);
        let o = ask(&cfg(&bad, CheckMode::Off), Q::Ready, "p", &a, "fp", |_| None);
        assert!(!o.local_ok && o.verdict.is_none());
    }

    #[test]
    fn test_model_reports_reachability() {
        assert!(test_model(&stub(r#"{"message": {"content": "{\"answer\": \"ok\", \"reason\": \"t\", \"confidence\": 1}"}}"#), "m").is_ok());
        assert!(test_model(DOWN, "m").unwrap_err().contains("Ollama"));
    }
}
