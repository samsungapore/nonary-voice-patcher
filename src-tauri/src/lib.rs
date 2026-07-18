#[cfg(feature = "gui")]
mod commands;
pub mod patcher;

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::OperationState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_capabilities,
            commands::inspect_rom,
            commands::apply_voice_patch,
            commands::reset_voice_patch,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Tauri application");
}
