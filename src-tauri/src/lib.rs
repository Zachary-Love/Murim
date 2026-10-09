pub mod repo_match;
pub mod settings;
pub mod linear;
mod caffeine;
mod inbox;
mod hooks;
mod sessions;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![settings::get_settings, settings::save_settings, settings::set_linear_key, settings::has_linear_key, caffeine::set_caffeine, caffeine::get_caffeine, inbox::get_inbox, inbox::refresh_inbox, inbox::list_repos, inbox::set_issue_repo, sessions::start_task, sessions::resume_task, sessions::stop_task, sessions::get_sessions, sessions::pty_write, sessions::pty_resize, sessions::pty_snapshot])
        .manage(caffeine::Caffeine::default())
        .manage(inbox::Inbox::default())
        .manage(sessions::Sessions::default())
        .setup(|app| {
            use tauri::Manager;
            if settings::load(&settings::dir(app.handle())?).caffeine {
                caffeine::set(&app.state::<caffeine::Caffeine>(), true)?;
            }
            sessions::load(app.handle());
            app.manage(hooks::start(app.handle().clone())?);
            inbox::start(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                sessions::kill_all(app);
                let _ = caffeine::set(&app.state::<caffeine::Caffeine>(), false);
            }
        });
}
