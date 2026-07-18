// GUI releases suppress the extra console because it can outlive the patcher
// window and make a normal exit look like a hung process to Windows users.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nonary_voice_patcher_lib::run()
}
