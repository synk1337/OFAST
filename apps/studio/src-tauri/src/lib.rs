//! OFAST Studio: the desktop app around `ofast-core` (`docs/user-guide/README.md`).
//!
//! A Tauri 2 window whose web UI (`apps/studio/src`, React) calls the commands below; every file is read through
//! `ofast-core`, as the CLI does. What it does so far: finds CS2, Unreal Engine 5.8 and OFAST's Unreal project
//! ([`commands::environment`]) and lists the demos in CS2's replays folder ([`commands::list_demos`]).

pub mod commands;

/// Starts the app: the window, its plugins (the native file dialog) and its commands.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::environment,
            commands::list_demos,
        ])
        .run(tauri::generate_context!())
        .expect("OFAST Studio failed to start");
}
