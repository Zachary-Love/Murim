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
    /// How Tempering posts Claude's review-comment replies.
    pub reply_mode: crate::tempering::ReplyMode,
    /// Linear label -> repo path (remembered matches).
    #[serde(default)]
    pub repo_map: std::collections::HashMap<String, String>,
    /// Last-used Reviews filters.
    pub review_filters: ReviewFilters,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct ReviewFilters {
    /// Linear user ids, or "unlinked"
    pub authors: Vec<String>,
    /// Bucket names: needsReview, draft, ciFailing, changesRequested, approved
    pub statuses: Vec<String>,
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
    settings.caffeine = load(&d).caffeine;
    settings.review_filters = load(&d).review_filters; // owned by the Reviews filter bar
    save(&d, &settings)
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
    fn round_trip_and_defaults() {
        let d = std::env::temp_dir().join(format!("murim-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(load(&d), Settings::default()); // missing
        let s = Settings { agent: AgentKind::Custom, agent_command: "codex".into(), repos_root: "/x".into(), default_permission_mode: PermissionMode::Skip, caffeine: true, default_prompt: "be brief".into(), reply_mode: crate::tempering::ReplyMode::Full, repo_map: [("bug".to_string(), "/r".to_string())].into(), review_filters: ReviewFilters { authors: vec!["u1".into(), "unlinked".into()], statuses: vec!["draft".into()] } };
        save(&d, &s).unwrap();
        assert_eq!(load(&d), s);
        std::fs::write(d.join("settings.json"), "{not json").unwrap();
        assert_eq!(load(&d), Settings::default()); // corrupt
        let _ = std::fs::remove_dir_all(&d);
    }
}
