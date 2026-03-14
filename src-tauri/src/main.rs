#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod launcher;
mod models;
mod updater;
mod utils;

use models::{AuthState, Config, UserInfo};
use std::fs;
use std::sync::{Arc, Mutex};

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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(AuthState::load()))
        .manage(Mutex::new(Config::load()))
        .manage(Arc::new(Mutex::new(AuthState::load())))
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
