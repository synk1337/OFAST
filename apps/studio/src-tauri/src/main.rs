//! OFAST Studio's executable: starts the app ([`ofast_studio::run`]).

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ofast_studio::run();
}
