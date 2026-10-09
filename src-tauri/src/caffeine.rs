use std::process::{Child, Command};
use std::sync::Mutex;

#[derive(Default)]
pub struct Caffeine(Mutex<Option<Child>>);

pub fn set(state: &Caffeine, enabled: bool) -> Result<(), String> {
    let mut child = state.0.lock().unwrap();
    if enabled && child.is_none() {
        // ponytail: macOS-only; the `keepawake` crate is the cross-platform upgrade path.
        if !cfg!(target_os = "macos") {
            return Err("caffeine mode is macOS-only for now".into());
        }
        // -w makes caffeinate exit when Murim dies, so crashes don't leak it.
        *child = Some(
            Command::new("caffeinate")
                .args(["-i", "-w", &std::process::id().to_string()])
                .spawn()
                .map_err(|e| e.to_string())?,
        );
    } else if !enabled {
        if let Some(mut c) = child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    Ok(())
}

#[tauri::command]
pub fn set_caffeine(app: tauri::AppHandle, state: tauri::State<Caffeine>, enabled: bool) -> Result<(), String> {
    set(&state, enabled)?;
    let d = crate::settings::dir(&app)?;
    let mut s = crate::settings::load(&d);
    s.caffeine = enabled;
    crate::settings::save(&d, &s)
}

#[tauri::command]
pub fn get_caffeine(state: tauri::State<Caffeine>) -> bool {
    state.0.lock().unwrap().is_some()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn alive(pid: u32) -> bool {
        Command::new("kill").args(["-0", &pid.to_string()]).status().unwrap().success()
    }

    #[test]
    fn toggle_spawns_and_kills() {
        let c = Caffeine::default();
        set(&c, true).unwrap();
        let pid = c.0.lock().unwrap().as_ref().unwrap().id();
        set(&c, true).unwrap(); // idempotent
        assert_eq!(c.0.lock().unwrap().as_ref().unwrap().id(), pid);
        assert!(alive(pid));
        set(&c, false).unwrap();
        assert!(c.0.lock().unwrap().is_none());
        assert!(!alive(pid));
    }
}
