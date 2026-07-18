#[cfg(feature = "gui")]
mod commands;
#[cfg(feature = "gui")]
pub mod dubbing;
pub mod patcher;

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::OperationState::default())
        .manage(commands::DubbingRecordingState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_capabilities,
            commands::inspect_rom,
            commands::apply_voice_patch,
            commands::reset_voice_patch,
            commands::create_dubbing_project,
            commands::open_dubbing_project,
            commands::update_dubbing_target,
            commands::list_dubbing_input_devices,
            commands::start_dubbing_recording,
            commands::stop_dubbing_recording,
            commands::cancel_dubbing_recording,
            commands::read_dubbing_take,
            commands::select_dubbing_take,
            commands::build_dubbing_test_rom,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Tauri application");
}
