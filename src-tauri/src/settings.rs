use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    #[default]
    Normal,
    Allowlist,
    Skip,
}

/// Which coding agent runs a task. Claude Code gets full automation via its hooks; a custom command
/// (codex, gemini, aider, …) runs in the same terminal without status, questions, quality pass or Tempering.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Custom,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Settings {
    pub agent: AgentKind,
    /// Shell command for a custom agent; the task prompt is appended as its last argument.
    pub agent_command: String,
    /// Folder holding the user's repos; empty until they choose one (no guessed default).
    pub repos_root: String,
    pub default_permission_mode: PermissionMode,
    pub caffeine: bool,
    /// Appended to every task prompt under "Instructions:" when non-empty.
    pub default_prompt: String,
    /// Claude Code through Amazon Bedrock (CLAUDE_CODE_USE_BEDROCK=1); empty region/profile = the AWS defaults
    pub bedrock: bool,
    pub aws_region: String,
    pub aws_profile: String,
    /// Appended to the quality-pass turn that opens the draft PR (proof screenshots, PR template, …).
    pub pr_prompt: String,
    /// Appended to every review agent's prompt under "Instructions:" when non-empty.
    pub review_prompt: String,
    /// How Tempering posts Claude's review-comment replies.
    pub reply_mode: crate::tempering::ReplyMode,
    /// Linear label -> repo path (remembered matches).
    #[serde(default)]
    pub repo_map: std::collections::HashMap<String, String>,
    /// Last-used Reviews filters.
    pub review_filters: ReviewFilters,
    /// Requested-reviews indicator on the rail.
    pub review_badge: ReviewBadge,
    /// Meditation artifacts folder; empty = ~/.murim/meditations.
    pub meditations_dir: String,
    /// Theme, accent, texture and fonts. Schema and defaults live in the frontend (src/appearance.ts); null = defaults.
    pub appearance: serde_json::Value,
    /// Pavilion board: local model, agent slots, auto-investigate, Claude check.
    pub pavilion: PavilionSettings,
}

pub fn meditations_dir(s: &Settings) -> PathBuf {
    if s.meditations_dir.trim().is_empty() {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".murim/meditations")
    } else {
        PathBuf::from(s.meditations_dir.trim())
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReviewBadge {
    #[default]
    Count,
    Dot,
    Off,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct ReviewFilters {
    /// Linear user ids, or "unlinked"
    pub authors: Vec<String>,
    /// Bucket names: needsReview, draft, ciFailing, changesRequested, approved
    pub statuses: Vec<String>,
    /// "owner/repo"
    pub repos: Vec<String>,
}

/// When Claude checks the local model's verdict.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum CheckMode {
    #[default]
    Always,
    LowConfidence,
    Off,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct PavilionSettings {
    /// false: judgement calls skip Ollama and go straight to Claude Haiku
    pub local_model: bool,
    pub ollama_url: String,
    pub model: String,
    /// Live agent sessions Pavilion fills up to when starting investigations.
    pub max_agents: u32,
    /// Docker builds/runs agents may have going at once (docker.rs); 0 = no queue.
    pub docker_slots: u32,
    /// Tempering on for every Claude session with an open PR (the local model triages the feedback first).
    pub auto_temper: bool,
    pub auto_investigate: bool,
    /// start a review agent on others' PRs once CI passes and no person has reviewed
    pub auto_review: bool,
    /// investigate + plan before coding; off (default): ready issues go straight to the coding agent
    pub plan_first: bool,
    pub claude_check: CheckMode,
    /// the orchestrator approves an investigation's plan itself (and coding starts) when it's at least this sure; 0 = always ask
    pub auto_approve_at: f32,
}

impl Default for PavilionSettings {
    fn default() -> Self {
        Self { local_model: true, ollama_url: "http://localhost:11434".into(), model: "qwen3:4b".into(), max_agents: 10, docker_slots: 2, auto_temper: true, auto_investigate: true, auto_review: true, plan_first: false, claude_check: CheckMode::Always, auto_approve_at: 0.75 }
    }
}

/// Missing or corrupt file yields defaults.
pub fn load(app_config_dir: &Path) -> Settings {
    std::fs::read_to_string(app_config_dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app_config_dir: &Path, s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(app_config_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(app_config_dir.join("settings.json"), json).map_err(|e| e.to_string())
}

/// Env every Claude Code process gets (agents and `claude -p`): the Bedrock switch. Set after the login-shell env,
/// which has its CLAUDE* vars stripped.
pub fn claude_env(s: &Settings) -> Vec<(String, String)> {
    if !s.bedrock {
        return vec![];
    }
    let mut v = vec![("CLAUDE_CODE_USE_BEDROCK".to_string(), "1".to_string())];
    for (k, x) in [("AWS_REGION", &s.aws_region), ("AWS_PROFILE", &s.aws_profile)] {
        if !x.trim().is_empty() {
            v.push((k.into(), x.trim().into()));
        }
    }
    v
}

/// claude_env of the saved settings, for callers without an AppHandle (claude_print); set at launch and on save.
pub static CLAUDE_ENV: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

pub fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

pub fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("dev.murim.app", "linear").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    Ok(load(&dir(&app)?))
}

#[tauri::command]
pub fn save_settings(app: AppHandle, mut settings: Settings) -> Result<(), String> {
    let d = dir(&app)?;
    // caffeine is owned by the sidebar toggle; don't let a stale form overwrite it.
    let cur = load(&d);
    settings.caffeine = cur.caffeine;
    settings.review_filters = cur.review_filters; // owned by the Reviews filter bar
    settings.appearance = cur.appearance; // owned by set_appearance (saves on change)
    crate::docker::set_slots(settings.pavilion.docker_slots);
    crate::pavilion::poke(&app); // a new model or URL: the offline probe picks it up now
    *CLAUDE_ENV.lock().unwrap() = claude_env(&settings);
    save(&d, &settings)
}

/// Applies immediately (no Save click), like the filters.
/// Applies immediately (no Save click).
#[tauri::command]
pub fn set_appearance(app: AppHandle, appearance: serde_json::Value) -> Result<(), String> {
    let d = dir(&app)?;
    let mut s = load(&d);
    s.appearance = appearance;
    save(&d, &s)
}

#[tauri::command]
pub fn set_review_badge(app: AppHandle, badge: ReviewBadge) -> Result<(), String> {
    let d = dir(&app)?;
    let mut s = load(&d);
    s.review_badge = badge;
    save(&d, &s)
}

#[tauri::command]
pub fn set_review_filters(app: AppHandle, filters: ReviewFilters) -> Result<(), String> {
    let d = dir(&app)?;
    let mut s = load(&d);
    s.review_filters = filters;
    save(&d, &s)
}

#[tauri::command]
pub fn set_linear_key(key: String) -> Result<(), String> {
    entry()?.set_password(&key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn has_linear_key() -> bool {
    entry().and_then(|e| e.get_password().map_err(|e| e.to_string())).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_env_only_when_on() {
        let mut s = Settings::default();
        assert!(claude_env(&s).is_empty());
        (s.bedrock, s.aws_region, s.aws_profile) = (true, " us-west-2 ".into(), String::new());
        assert_eq!(claude_env(&s), vec![("CLAUDE_CODE_USE_BEDROCK".to_string(), "1".to_string()), ("AWS_REGION".to_string(), "us-west-2".to_string())]);
    }

    #[test]
    fn round_trip_and_defaults() {
        let d = std::env::temp_dir().join(format!("murim-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(load(&d), Settings::default()); // missing
        let s = Settings { agent: AgentKind::Custom, agent_command: "codex".into(), repos_root: "/x".into(), default_permission_mode: PermissionMode::Skip, caffeine: true, default_prompt: "be brief".into(), review_prompt: "check tests".into(), pr_prompt: "add proof".into(), bedrock: true, aws_region: "us-east-1".into(), aws_profile: String::new(), reply_mode: crate::tempering::ReplyMode::Full, repo_map: [("bug".to_string(), "/r".to_string())].into(), review_filters: ReviewFilters { authors: vec!["u1".into(), "unlinked".into()], statuses: vec!["draft".into()], repos: vec!["o/api".into()] }, review_badge: ReviewBadge::Dot, meditations_dir: "/m".into(), appearance: serde_json::json!({"mode": "custom", "base": "#efe6d8"}), pavilion: PavilionSettings { max_agents: 5, ..Default::default() } };
        save(&d, &s).unwrap();
        assert_eq!(load(&d), s);
        std::fs::write(d.join("settings.json"), "{not json").unwrap();
        assert_eq!(load(&d), Settings::default()); // corrupt
        assert_eq!(meditations_dir(&s), PathBuf::from("/m"));
        assert!(meditations_dir(&Settings::default()).ends_with(".murim/meditations"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pavilion_defaults_and_partial() {
        let p = PavilionSettings::default();
        assert_eq!((p.ollama_url.as_str(), p.model.as_str(), p.max_agents, p.docker_slots, p.auto_investigate, p.claude_check), ("http://localhost:11434", "qwen3:4b", 10, 2, true, CheckMode::Always));
        assert!(p.auto_temper && p.local_model && p.auto_approve_at == 0.75);
        let s: Settings = serde_json::from_str(r#"{"pavilion": {"max_agents": 2, "claude_check": "lowConfidence"}}"#).unwrap();
        assert_eq!((s.pavilion.max_agents, s.pavilion.claude_check, s.pavilion.model.as_str()), (2, CheckMode::LowConfidence, "qwen3:4b"));
    }
}
