// Prevents extra console window on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nimbusbill_desktop_lib::run();
}
