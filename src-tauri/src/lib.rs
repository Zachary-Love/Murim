pub mod repo_match;
pub mod settings;
pub mod linear;
mod caffeine;
mod inbox;
mod github;
mod hooks;
mod sessions;
mod tempering;
mod review;
mod reviews;
mod tray;
mod term_theme;
mod meditations;
mod automations;
mod judge;
mod pavilion;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Launched from an editor's terminal (e.g. `pnpm tauri dev` in VS Code)? Drop its askpass hooks before any thread or child starts.
    for (k, _) in std::env::vars() {
        if sessions::editor_var(&k) {
            std::env::remove_var(k);
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![settings::get_settings, settings::save_settings, settings::set_linear_key, settings::has_linear_key, caffeine::set_caffeine, caffeine::get_caffeine, inbox::get_inbox, inbox::refresh_inbox, inbox::list_repos, inbox::set_issue_repo, linear::get_issue_detail, sessions::start_task, sessions::resume_task, sessions::stop_task, sessions::get_sessions, sessions::pty_write, sessions::pty_resize, sessions::pty_snapshot, sessions::open_shell, sessions::list_shells, sessions::close_shell, github::get_prs, github::refresh_prs, tempering::set_tempering, tempering::resolve_escalation, review::get_review, review::accept_pr, review::request_changes, reviews::get_reviews, reviews::refresh_reviews, settings::set_review_filters, settings::set_review_badge, settings::set_appearance, reviews::start_review_agent, reviews::get_review_draft, reviews::post_review, reviews::dismiss_review, reviews::get_pr_diff, term_theme::terminal_theme, meditations::start_meditation, meditations::rename_meditation, meditations::delete_meditation, meditations::list_artifacts, meditations::read_artifact, meditations::get_meditations_dir, automations::list_automations, automations::save_automation, automations::delete_automation, automations::run_automation_now, automations::delete_run, pavilion::get_pavilion, pavilion::pavilion_approve, pavilion::pavilion_dismiss_blocker, pavilion::pavilion_start_now, pavilion::pavilion_set_paused, pavilion::pavilion_test_model])
        .manage(caffeine::Caffeine::default())
        .manage(inbox::Inbox::default())
        .manage(sessions::Sessions::default())
        .manage(github::Github::default())
        .manage(reviews::Reviews::default())
        .manage(automations::Automations::default())
        .manage(pavilion::Pavilion::default())
        .setup(|app| {
            use tauri::Manager;
            if settings::load(&settings::dir(app.handle())?).caffeine {
                caffeine::set(&app.state::<caffeine::Caffeine>(), true)?;
            }
            sessions::load(app.handle());
            app.manage(hooks::start(app.handle().clone())?);
            inbox::start(app.handle().clone());
            github::start(app.handle().clone());
            reviews::start(app.handle().clone());
            tray::start(app.handle())?;
            automations::start(app.handle().clone());
            pavilion::start(app.handle().clone());
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
