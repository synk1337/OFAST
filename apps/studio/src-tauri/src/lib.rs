//! OFAST Studio: the desktop app around `ofast-core` (`docs/user-guide/README.md`).
//!
//! A Tauri 2 window whose web UI (`apps/studio/src`, React) calls the commands below; every file is read through
//! `ofast-core`, as the CLI does. What it does so far: finds CS2, Unreal Engine 5.8 and OFAST's Unreal project
//! ([`commands::environment`]), lists the demos in CS2's replays folder ([`commands::list_demos`]) and sums a demo
//! up — rounds, scores, kill feed, highlights ([`commands::summarize_demo`]).

pub mod commands;
pub mod unreal;

/// Starts the app: the window, its plugins (the native file dialog), its commands and their state; on the way out,
/// stops what a sending left running.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::Summaries::default())
        .manage(commands::Radars::default())
        .manage(unreal::Sending::default())
        .invoke_handler(tauri::generate_handler![
            commands::environment,
            commands::list_demos,
            commands::summarize_demo,
            commands::radar_round,
            commands::radar_image,
            unreal::send_to_unreal,
            unreal::update_cameras,
            unreal::sent_rounds,
            unreal::open_in_unreal,
        ])
        .build(tauri::generate_context!())
        .expect("OFAST Studio failed to start")
        .run(|_, event| {
            // Closing the Studio stops a sending's Unreal, rather than leaving it importing unseen.
            if let tauri::RunEvent::Exit = event {
                unreal::stop_sending();
            }
        });
}
