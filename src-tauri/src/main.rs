#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod launcher;
mod models;
mod updater;
mod utils;

use models::{AuthState, Config, UserInfo};
use reqwest::Client;
use std::fs;
use std::sync::Mutex;
use std::time::Duration;

#[tauri::command]
fn save_config(config: Config, state: tauri::State<'_, Mutex<Config>>) -> Result<(), String> {
    let mut current_config = state.lock().unwrap();
    *current_config = config;
    current_config.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_config(s: tauri::State<'_, Mutex<Config>>) -> Config {
    s.lock().unwrap().clone()
}

#[tauri::command]
fn get_current_user(s: tauri::State<'_, Mutex<AuthState>>) -> Option<UserInfo> {
    let auth = s.lock().unwrap();
    auth.current_user_id
        .as_ref()
        .and_then(|id| auth.users.iter().find(|u| &u.uuid == id).cloned())
}

#[tauri::command]
fn logout_current_user(state: tauri::State<'_, Mutex<AuthState>>) -> bool {
    let mut auth = state.lock().unwrap();
    auth.users.clear();
    auth.current_user_id = None;

    let path = AuthState::file_path();
    if path.exists() {
        if let Err(_) = fs::remove_file(&path) {
            return false;
        }
    }
    true
}

fn main() {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
        .user_agent("CirCubeLauncher/2.0 (Windows NT 10.0; Win64; x64) reqwest/0.12")
        .build()
        .expect("Failed to create reqwest::Client");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(client)
        .manage(Mutex::new(AuthState::load()))
        .manage(Mutex::new(Config::load()))
        .invoke_handler(tauri::generate_handler![
            auth::ms_login,
            auth::yggdrasil_login,
            auth::yggdrasil_select,
            save_config,
            get_config,
            utils::get_total_memory,
            utils::get_used_memory,
            get_current_user,
            launcher::launch_minecraft,
            logout_current_user,
            utils::scan_java_environments,
            utils::validate_java,
            utils::check_mc_directory,
            updater::sync_versions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
