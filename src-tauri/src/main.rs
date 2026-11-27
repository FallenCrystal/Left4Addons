#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    l4a_core::filter::init_filter_image_worker();
    l4a_tauri_lib::run();
}
