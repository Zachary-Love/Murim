use crate::{linear::Issue, repo_match::{self, Repo, RepoMatch, Source}, settings};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub issue: Issue,
    pub repo: Option<RepoMatch>,
    pub resolving: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct Payload {
    pub items: Vec<InboxItem>,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    payload: Payload,
    /// issue id -> resolved match (None = tried, no match). Lives for the app run.
    cache: HashMap<String, Option<RepoMatch>>,
    inflight: HashSet<String>,
}

#[derive(Default)]
pub struct Inbox {
    inner: Mutex<Inner>,
    wake: (Mutex<bool>, Condvar),
}

/// Where a manual choice is remembered: the label if it's the only one (it must be the repo label),
/// else the issue itself — with several labels we can't tell which one names the repo.
pub fn remember_key(i: &Issue) -> String {
    match i.labels.as_slice() {
        [only] => only.clone(),
        _ => format!("issue:{}", i.id),
    }
}

// ponytail: one resolver at a time so a cold start doesn't fork a `claude` per unmatched issue.
static RESOLVE_LOCK: Mutex<()> = Mutex::new(());

fn emit(app: &AppHandle, inner: &Inner) {
    let _ = app.emit("inbox-updated", &inner.payload);
}

fn state(app: &AppHandle) -> State<'_, Inbox> {
    app.state::<Inbox>()
}

fn cheap(i: &Issue, repos: &[Repo], map: &HashMap<String, String>) -> Option<RepoMatch> {
    repo_match::by_remembered(&[format!("issue:{}", i.id)], map).or_else(|| repo_match::resolve_with(&i.labels, &i.title, repos, map, false, false))
}

fn poll(app: &AppHandle) {
    let inbox = state(app);
    let set_err = |e: &str, clear: bool| {
        let mut g = inbox.inner.lock().unwrap();
        if clear {
            g.payload.items.clear();
        }
        g.payload.error = Some(e.to_string());
        emit(app, &g);
    };
    let Some(key) = settings::entry().ok().and_then(|e| e.get_password().ok()) else {
        return set_err("Add your Linear API key in Settings", true);
    };
    if settings::dir(app).map(|d| settings::load(&d).repos_root.is_empty()).unwrap_or(true) {
        return set_err("Choose your repos folder in Settings", true);
    }
    let issues = match crate::linear::fetch_cycle_issues(&key) {
        Ok(v) => v,
        Err(e) => return set_err(&e, false),
    };
    let Ok(dir) = settings::dir(app) else { return };
    let s = settings::load(&dir);
    let repos = repo_match::list_repos(std::path::Path::new(&s.repos_root));
    let mut g = inbox.inner.lock().unwrap();
    let mut items = vec![];
    for issue in issues {
        let (repo, resolving) = match g.cache.get(&issue.id) {
            Some(m) => (m.clone(), false),
            None => match cheap(&issue, &repos, &s.repo_map) {
                Some(m) => {
                    g.cache.insert(issue.id.clone(), Some(m.clone()));
                    (Some(m), false)
                }
                None => {
                    if g.inflight.insert(issue.id.clone()) {
                        spawn_resolver(app.clone(), issue.clone(), repos.clone(), s.repo_map.clone());
                    }
                    (None, true)
                }
            },
        };
        items.push(InboxItem { issue, repo, resolving });
    }
    g.payload = Payload { items, error: None };
    emit(app, &g);
}

fn spawn_resolver(app: AppHandle, issue: Issue, repos: Vec<Repo>, map: HashMap<String, String>) {
    std::thread::spawn(move || {
        let m = {
            let _one = RESOLVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            repo_match::resolve(&issue.labels, &issue.title, &repos, &map)
        };
        let inbox = state(&app);
        let mut g = inbox.inner.lock().unwrap();
        g.inflight.remove(&issue.id);
        if matches!(g.cache.get(&issue.id), Some(Some(c)) if c.source == Source::Manual) {
            return; // user picked while we were thinking
        }
        g.cache.insert(issue.id.clone(), m.clone());
        if let Some(it) = g.payload.items.iter_mut().find(|it| it.issue.id == issue.id) {
            it.repo = m;
            it.resolving = false;
        }
        emit(&app, &g);
    });
}

/// Current inbox item for an issue id (used by sessions to find issue + repo).
pub fn find(app: &AppHandle, issue_id: &str) -> Option<InboxItem> {
    state(app).inner.lock().unwrap().payload.items.iter().find(|it| it.issue.id == issue_id).cloned()
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        poll(&app);
        let (m, cv) = &state(&app).wake;
        let g = cv.wait_timeout_while(m.lock().unwrap(), Duration::from_secs(60), |woken| !*woken).unwrap().0;
        drop(g);
        *m.lock().unwrap() = false;
    });
}

#[tauri::command]
pub fn get_inbox(state: State<Inbox>) -> Payload {
    state.inner.lock().unwrap().payload.clone()
}

#[tauri::command]
pub fn refresh_inbox(state: State<Inbox>) {
    *state.wake.0.lock().unwrap() = true;
    state.wake.1.notify_one();
}

#[tauri::command]
pub fn list_repos(app: AppHandle) -> Result<Vec<Repo>, String> {
    Ok(repo_match::list_repos(std::path::Path::new(&settings::load(&settings::dir(&app)?).repos_root)))
}

#[tauri::command]
pub fn set_issue_repo(app: AppHandle, state: State<Inbox>, issue_id: String, path: String) -> Result<(), String> {
    let mut g = state.inner.lock().unwrap();
    let it = g.payload.items.iter().find(|it| it.issue.id == issue_id).ok_or("unknown issue")?;
    let key = remember_key(&it.issue);
    let d = settings::dir(&app)?;
    let mut s = settings::load(&d);
    s.repo_map.insert(key, path.clone());
    settings::save(&d, &s)?;
    let m = Some(RepoMatch { path, source: Source::Manual });
    g.cache.insert(issue_id.clone(), m.clone());
    if let Some(it) = g.payload.items.iter_mut().find(|it| it.issue.id == issue_id) {
        it.repo = m;
        it.resolving = false;
    }
    emit(&app, &g);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_key_and_issue_key_lookup() {
        let mut i = Issue { id: "abc".into(), identifier: "E-1".into(), title: "t".into(), description: "".into(), url: "".into(), branch_name: "".into(), state_name: "".into(), state_type: "".into(), priority: 0, labels: vec![], team_key: "".into() };
        assert_eq!(remember_key(&i), "issue:abc");
        let dir = std::env::temp_dir();
        let map: HashMap<_, _> = [(remember_key(&i), dir.to_string_lossy().to_string())].into();
        let m = cheap(&i, &[], &map).unwrap();
        assert_eq!((m.source, m.path), (Source::Remembered, dir.to_string_lossy().to_string()));
        i.labels = vec!["murim".into()];
        assert_eq!(remember_key(&i), "murim");
        i.labels = vec!["bug".into(), "murim".into()];
        assert_eq!(remember_key(&i), "issue:abc");
    }
}
