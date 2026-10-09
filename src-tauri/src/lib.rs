pub mod settings;
mod caffeine;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![settings::get_settings, settings::save_settings, settings::set_linear_key, settings::has_linear_key, caffeine::set_caffeine, caffeine::get_caffeine])
        .manage(caffeine::Caffeine::default())
        .setup(|app| {
            use tauri::Manager;
            if settings::load(&settings::dir(app.handle())?).caffeine {
                caffeine::set(&app.state::<caffeine::Caffeine>(), true)?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let _ = caffeine::set(&app.state::<caffeine::Caffeine>(), false);
            }
        });
}
