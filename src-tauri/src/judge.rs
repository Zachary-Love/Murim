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
    /// PR review feedback: a change for the agent, a call for the user, or nothing to do?
    Feedback,
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
        Q::Feedback => vec!["act".into(), "ask".into(), "skip".into()],
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
        Q::Feedback => format!("You triage code review feedback on a pull request for the coding agent that wrote it. \"act\": it asks for a concrete change the agent can make in the code. \"ask\": it is a question, a product or design decision, or too unclear for the agent, so the person who owns the PR must answer. \"skip\": praise, thanks, a nit explicitly marked optional, or an automated notice with nothing to change. Allowed answers: \"act\", \"ask\", \"skip\".\n{rules}\n\nFeedback:\n{issue}"),
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
fn ollama(url: &str, model: &str, prompt: &str, format: Value) -> Result<String, String> {
    let body = json!({"model": model, "messages": [{"role": "user", "content": prompt}], "stream": false, "format": format, "options": {"temperature": 0}});
    let res = match ureq::post(&format!("{}/api/chat", url.trim_end_matches('/'))).timeout(Duration::from_secs(60)).send_json(body) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => return Err(format!("Ollama: {}", r.into_string().unwrap_or_default().trim())), // e.g. model not pulled
        Err(e) => return Err(format!("Ollama not reachable at {url}: {e}")),
    };
    let v: Value = res.into_json().map_err(|e| format!("Ollama: {e}"))?;
    v["message"]["content"].as_str().map(String::from).ok_or_else(|| "Ollama: no message in the reply".into())
}

/// Who answers first, for the board header and the log.
pub fn model_name(cfg: &PavilionSettings) -> String {
    if cfg.local_model { cfg.model.clone() } else { "Claude Haiku".into() }
}

pub struct Outcome {
    /// None: no usable answer (Claude failed); retry later
    pub verdict: Option<Verdict>,
    /// the local model answered with valid JSON
    pub local_ok: bool,
    /// what each model said, for the orchestrator log
    pub log: Vec<String>,
    /// the local model's (answer, reason), when it gave one: a Claude answer that differs is a lesson
    pub local: Option<(String, String)>,
}

fn said((a, r, c): &(String, String, f32)) -> String {
    format!("{a} ({:.0}% sure) {r}", c * 100.0)
}

/// Local answer first; Claude checks when the policy says so, or answers alone when the local model failed.
pub fn ask(cfg: &PavilionSettings, q: Q, prompt: &str, allowed: &[String], fp: &str, claude: impl Fn(&str) -> Option<String>) -> Outcome {
    // local model off (Settings → Pavilion): Claude answers alone, and the header must not call that "offline"
    let raw = if cfg.local_model { ollama(&cfg.ollama_url, &cfg.model, prompt, schema(allowed)) } else { Err(String::new()) };
    let local = raw.as_ref().ok().and_then(|r| parse(r, allowed));
    let local_ok = local.is_some() || !cfg.local_model;
    let said_local = local.as_ref().map(|l| (l.0.clone(), l.1.clone()));
    let mut log: Vec<String> = match (&raw, &local) {
        _ if !cfg.local_model => vec![],
        (_, Some(l)) => vec![format!("{}: {}", cfg.model, said(l))],
        (Ok(r), None) => vec![format!("{}: unusable reply: {}", cfg.model, r.chars().take(200).collect::<String>())],
        (Err(e), _) => vec![format!("{}: failed: {e}", cfg.model)],
    };
    let v = |(answer, reason, confidence): (String, String, f32), by| Verdict { answer, reason, confidence, by, fingerprint: fp.to_string() };
    match local {
        Some(l) if !needs_check(cfg.claude_check, q, &l.0, l.2) => Outcome { verdict: Some(v(l, By::Local)), local_ok, log, local: said_local },
        l => {
            let note = l.map(|(a, r, c)| format!("\n\nA small local model answered: {}", json!({"answer": a, "reason": r, "confidence": c}))).unwrap_or_default();
            let checked = claude(&format!("{prompt}{note}\n\nGive your own answer in the same JSON format.")).and_then(|r| parse(&r, allowed));
            log.push(checked.as_ref().map_or("claude: no usable answer (failed, timed out or bad JSON)".into(), |c| format!("claude: {}", said(c))));
            Outcome { verdict: checked.map(|c| v(c, By::Claude)), local_ok, log, local: said_local }
        }
    }
}

/// Pavilion's memory (Hermes-style): a small file the model rewrites when it's corrected, hard-capped in characters.
pub const MEMORY_MAX: usize = 2000;

pub fn memory_prompt(current: &str, event: &str, over: Option<usize>) -> String {
    let retry = over.map(|n| format!("\n\nYour last rewrite was {n} characters; it must be at most {MEMORY_MAX}. Merge or drop the least useful lessons.")).unwrap_or_default();
    let current = if current.trim().is_empty() { "(empty)" } else { current.trim() };
    format!("You keep a small memory that helps you triage this user's software issues and code review feedback. Something just happened that may teach you how this user works. Rewrite the memory: keep durable, general lessons (their preferences, recurring patterns, what they consider ready, blocked or needing them), merge duplicates, drop one-off details and anything the event contradicts. If it teaches nothing new, return the memory unchanged. Short markdown bullets, at most {MEMORY_MAX} characters.{retry}\nReply with only a JSON object: {{\"memory\": <the whole new memory>}}.\n\nCurrent memory:\n{current}\n\nWhat happened:\n{event}")
}

pub fn parse_memory(raw: &str) -> Option<String> {
    let (a, b) = (raw.find('{')?, raw.rfind('}')?);
    let v: Value = serde_json::from_str(raw.get(a..=b)?).ok()?;
    v["memory"].as_str().map(|m| m.trim().to_string())
}

/// The rewritten memory: local model first, Claude when it fails; one retry when over the cap. Err keeps the old one.
pub fn rewrite_memory(cfg: &PavilionSettings, current: &str, event: &str, claude: impl Fn(&str) -> Option<String>) -> Result<String, String> {
    let format = json!({"type": "object", "properties": {"memory": {"type": "string"}}, "required": ["memory"]});
    let mut over = None;
    for _ in 0..2 {
        let p = memory_prompt(current, event, over);
        let local = if cfg.local_model { ollama(&cfg.ollama_url, &cfg.model, &p, format.clone()).ok() } else { None };
        let m = local.and_then(|r| parse_memory(&r)).or_else(|| claude(&p).and_then(|r| parse_memory(&r))).ok_or("neither the local model nor Claude answered")?;
        let n = m.chars().count();
        if n <= MEMORY_MAX {
            return Ok(m);
        }
        over = Some(n);
    }
    Err(format!("the rewrite was still over {MEMORY_MAX} characters"))
}

/// Settings → Test: one tiny structured call.
pub fn test_model(url: &str, model: &str) -> Result<String, String> {
    let ok = vec!["ok".to_string()];
    let r = ollama(url, model, r#"Reply with the JSON {"answer": "ok", "reason": "test", "confidence": 1}."#, schema(&ok))?;
    parse(&r, &ok).map(|_| format!("{model} answered")).ok_or(format!("Unexpected reply from {model}: {r}"))
}

/// Settings model picker: the models installed in Ollama.
pub fn models(url: &str) -> Result<Vec<String>, String> {
    let v: Value = ureq::get(&format!("{}/api/tags", url.trim_end_matches('/')))
        .timeout(Duration::from_secs(5))
        .call()
        .map_err(|e| format!("Ollama not reachable at {url}: {e}"))?
        .into_json()
        .map_err(|e| format!("Ollama: {e}"))?;
    let mut m: Vec<String> = v["models"].as_array().into_iter().flatten().filter_map(|x| x["name"].as_str().map(String::from)).collect();
    m.sort();
    Ok(m)
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
        assert_eq!(allowed(Q::Feedback, &[], ""), ids(&["act", "ask", "skip"]));
        assert!(prompt(Q::Feedback, "a.rs:3\nbob: rename x", &[], "").contains("bob: rename x"));
        assert_eq!(parse(r#"{"answer": "maybe"}"#, &allowed(Q::Feedback, &[], "")), None); // no "none" fallback: unanswered → the agent gets it
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
        // local model off: Claude only, never "offline", Ollama never called (DOWN would fail)
        let off = PavilionSettings { local_model: false, ..cfg(DOWN, CheckMode::Off) };
        let o = ask(&off, Q::Ready, "p", &a, "fp", |_| Some(r#"{"answer": "ready", "reason": "ok", "confidence": 1}"#.into()));
        assert_eq!((o.verdict.unwrap().by, o.local_ok, o.log.len()), (By::Claude, true, 1));
        assert_eq!(model_name(&off), "Claude Haiku");
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
    fn memory_rewrite_is_capped() {
        assert!(memory_prompt("", "moved MUR-1", None).contains("Current memory:\n(empty)"));
        assert!(memory_prompt("- a", "e", Some(2400)).contains("was 2400 characters"));
        assert_eq!(parse_memory("ok {\"memory\": \" - likes small PRs \"}").as_deref(), Some("- likes small PRs"));
        let ok = stub(r#"{"message": {"content": "{\"memory\": \"- bugs with repro steps are ready\"}"}}"#);
        assert_eq!(rewrite_memory(&cfg(&ok, CheckMode::Off), "", "e", |_| panic!("local answered")).unwrap(), "- bugs with repro steps are ready");
        // local model down: Claude rewrites; too long twice: keep the old memory
        assert_eq!(rewrite_memory(&cfg(DOWN, CheckMode::Off), "", "e", |_| Some(r#"{"memory": "- x"}"#.into())).unwrap(), "- x");
        let long = format!(r#"{{"memory": "{}"}}"#, "x".repeat(MEMORY_MAX + 1));
        let calls = std::cell::Cell::new(0);
        assert!(rewrite_memory(&cfg(DOWN, CheckMode::Off), "", "e", |p| { calls.set(calls.get() + 1); assert_eq!(calls.get() == 2, p.contains("characters; it must be")); Some(long.clone()) }).is_err());
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_model_reports_reachability() {
        assert!(test_model(&stub(r#"{"message": {"content": "{\"answer\": \"ok\", \"reason\": \"t\", \"confidence\": 1}"}}"#), "m").is_ok());
        assert!(test_model(DOWN, "m").unwrap_err().contains("Ollama"));
        assert_eq!(models(&stub(r#"{"models": [{"name": "qwen3:4b"}, {"name": "gemma3:1b"}]}"#)).unwrap(), ["gemma3:1b", "qwen3:4b"]);
        assert!(models(DOWN).is_err());
    }
}
