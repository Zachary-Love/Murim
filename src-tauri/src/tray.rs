//! macOS menu bar icon: what needs you — drafts to review, Tempering escalations, automation runs waiting, plans to approve.
use crate::hooks::{PrFlow, Status};
use crate::sessions::{self, SessionInfo};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

const ID: &str = "murim";
const PLAIN: &[u8] = include_bytes!("../icons/tray/plain.png");
const DOT: &[u8] = include_bytes!("../icons/tray/dot.png");
const SECTIONS: [&str; 4] = ["Drafts to review", "Tempering needs you", "Automations waiting", "Plans to approve"];

/// (section, menu id, label); menu id is `task|<issueId>`, `run|<sessionId>` or `pavilion|<issueId>`.
type Entry = (usize, String, String);
static LAST: Mutex<Option<Vec<Entry>>> = Mutex::new(None);

/// Pure: who needs the user, sorted by section then label. `task` gives a task's menu label.
pub fn needs(map: &HashMap<String, SessionInfo>, plans: &[(String, String)], task: impl Fn(&SessionInfo) -> String) -> Vec<Entry> {
    let mut v = vec![];
    for (id, s) in map {
        if sessions::is_automation(id) {
            if s.status == Status::NeedsInput {
                v.push((2, format!("run|{id}"), s.name.clone()));
            }
        } else if !sessions::no_pr_flow(id) {
            if matches!(s.pr_flow, PrFlow::AwaitingReview | PrFlow::Opened) {
                v.push((0, format!("task|{id}"), task(s)));
            }
            if !s.escalations.is_empty() {
                v.push((1, format!("task|{id}"), task(s)));
            }
        }
    }
    v.extend(plans.iter().map(|(id, label)| (3, format!("pavilion|{id}"), label.clone())));
    v.sort();
    v
}

fn short(s: &str) -> String {
    if s.chars().count() > 48 { format!("{}…", s.chars().take(47).collect::<String>()) } else { s.to_string() }
}

pub fn start(app: &AppHandle) -> tauri::Result<()> {
    TrayIconBuilder::with_id(ID)
        .icon(Image::from_bytes(PLAIN)?)
        .icon_as_template(true)
        .tooltip("Murim")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, e| click(app, e.id().as_ref()))
        .build(app)?;
    refresh(app, &app.state::<sessions::Sessions>().snapshot());
    Ok(())
}

/// Rebuild the menu when the list changed. Called after every sessions update, outside the sessions lock.
pub fn refresh(app: &AppHandle, map: &HashMap<String, SessionInfo>) {
    let task = |s: &SessionInfo| match crate::inbox::find(app, &s.issue_id) {
        Some(it) => short(&format!("{}  {}", it.issue.identifier, it.issue.title)),
        None => short(&sessions::title_of(s)),
    };
    let plans: Vec<(String, String)> = crate::pavilion::plans_ready(app).into_iter().map(|(id, l)| (id, short(&l))).collect();
    let v = needs(map, &plans, task);
    {
        let mut last = LAST.lock().unwrap();
        if last.as_ref() == Some(&v) {
            return;
        }
        *last = Some(v.clone());
    }
    let Some(tray) = app.tray_by_id(ID) else { return };
    let _ = tray.set_icon(Image::from_bytes(if v.is_empty() { PLAIN } else { DOT }).ok());
    let _ = tray.set_icon_as_template(true);
    if let Ok(m) = menu(app, &v) {
        let _ = tray.set_menu(Some(m));
    }
}

fn menu(app: &AppHandle, v: &[Entry]) -> tauri::Result<Menu<tauri::Wry>> {
    let m = Menu::new(app)?;
    if v.is_empty() {
        m.append(&MenuItem::new(app, "Nothing needs you", false, None::<&str>)?)?;
    }
    for (i, head) in SECTIONS.iter().enumerate() {
        let items: Vec<_> = v.iter().filter(|e| e.0 == i).collect();
        if items.is_empty() {
            continue;
        }
        m.append(&MenuItem::new(app, *head, false, None::<&str>)?)?;
        for (_, id, label) in items {
            m.append(&MenuItem::with_id(app, id.as_str(), format!("   {label}"), true, None::<&str>)?)?;
        }
    }
    m.append(&PredefinedMenuItem::separator(app)?)?;
    m.append(&MenuItem::with_id(app, "open", "Open Murim", true, None::<&str>)?)?;
    m.append(&MenuItem::with_id(app, "quit", "Quit Murim", true, None::<&str>)?)?;
    Ok(m)
}

fn click(app: &AppHandle, id: &str) {
    if id == "quit" {
        return app.exit(0);
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    if let Some(t) = id.strip_prefix("task|") {
        let _ = app.emit("navigate", json!({ "view": "tasks", "id": t }));
    } else if let Some(r) = id.strip_prefix("run|") {
        let _ = app.emit("navigate", json!({ "view": "automations", "id": r }));
    } else if let Some(p) = id.strip_prefix("pavilion|") {
        let _ = app.emit("navigate", json!({ "view": "pavilion", "id": p }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, f: impl FnOnce(&mut SessionInfo)) -> (String, SessionInfo) {
        let mut v: SessionInfo = serde_json::from_value(json!({"issueId": id, "worktree": "", "sessionId": null, "status": "idle", "questions": null, "ptyAlive": false, "permissionMode": "normal"})).unwrap();
        f(&mut v);
        (id.into(), v)
    }

    #[test]
    fn needs_picks_drafts_escalations_and_waiting_runs() {
        let esc: crate::tempering::Escalation = serde_json::from_value(json!({"id": "e", "threadId": "t", "path": "", "comment": "", "reason": "r"})).unwrap();
        let map: HashMap<_, _> = [
            s("a", |x| x.pr_flow = PrFlow::AwaitingReview),
            s("b", |x| x.escalations = vec![esc]),
            s("c", |x| x.status = Status::NeedsInput), // task question: not listed
            s("automation:1:2", |x| (x.status, x.name) = (Status::NeedsInput, "Digest".into())),
            s("automation:1:3", |x| x.name = "Idle run".into()),
            s("meditation:9", |x| x.status = Status::NeedsInput),
            s("review:o/r/1", |x| x.pr_flow = PrFlow::AwaitingReview),
        ].into();
        let v = needs(&map, &[("i9".into(), "MUR-9  Plan".into())], |s| s.issue_id.to_uppercase());
        assert_eq!(v, vec![(0, "task|a".into(), "A".into()), (1, "task|b".into(), "B".into()), (2, "run|automation:1:2".into(), "Digest".into()), (3, "pavilion|i9".into(), "MUR-9  Plan".into())]);
    }
}
