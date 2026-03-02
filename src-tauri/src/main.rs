#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::fs;
use std::path::PathBuf;
use dirs_next::data_dir;
use std::process::Command;
use sysinfo::System;

// -------------------- 数据结构 --------------------

#[derive(Deserialize)]
struct AuthPayload {
    email: String,
    password: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Config {
    java_path: String,
    max_memory: u64,
}

impl Config {
    fn file_path() -> PathBuf {
        let mut path = data_dir().unwrap_or_else(|| std::env::current_dir().unwrap());
        path.push("circube-launcher");
        fs::create_dir_all(&path).ok();
        path.push("config.json");
        path
    }

    fn load() -> Self {
        if let Ok(data) = fs::read_to_string(Self::file_path()) {
            if let Ok(cfg) = serde_json::from_str(&data) {
                return cfg;
            }
        }
        Config { java_path: "".into(), max_memory: 4096 }
    }

    fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(Self::file_path(), json)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInfo {
    name: String,
    uuid: String,
    access_token: String,
    skin_url: String,
    auth_type: String,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct AuthState {
    current_user_id: Option<String>,
    users: Vec<UserInfo>,
}

#[derive(Serialize, Debug)]
struct JavaInfo {
    path: String,
    version: String,
}


impl AuthState {
    fn file_path() -> PathBuf {
        let mut path = data_dir().unwrap_or_else(|| std::env::current_dir().unwrap());
        path.push("circube-launcher");
        fs::create_dir_all(&path).ok();
        path.push("auth_state.json");
        path
    }

    fn load() -> Self {
        if let Ok(data) = fs::read_to_string(Self::file_path()) {
            if let Ok(state) = serde_json::from_str(&data) {
                return state;
            }
        }
        AuthState::default()
    }

    fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(Self::file_path(), json)
    }
}

// -------------------- Tauri 命令 --------------------

#[tauri::command]
async fn ms_login_command() -> Result<String, String> {
    println!("调用微软 OAuth (模拟)...");
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    Ok("SUCCESS_MS_AUTH_MOCK".into())
}

use base64::{Engine};
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

#[tauri::command]
async fn yggdrasil_login(
    payload: AuthPayload,
    state: tauri::State<'_, Mutex<AuthState>>
) -> Result<UserInfo, String> {
    let client = reqwest::Client::new();
    let auth_url = "https://littleskin.cn/api/yggdrasil/authserver/authenticate";

    // 更新后的 texture_url
    let texture_url = |uuid: &str| format!("https://littleskin.cn/api/yggdrasil/sessionserver/session/minecraft/profile/{}", uuid);

    // 构建认证请求体
    let body = serde_json::json!( {
        "agent": { "name": "Minecraft", "version": 1 },
        "username": payload.email,
        "password": payload.password,
        "requestUser": true
    });

    // 发送认证请求
    let response = client.post(auth_url).json(&body).send().await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    if response.status().is_success() {
        let auth_data: serde_json::Value = response.json().await
            .map_err(|e| format!("解析失败: {}", e))?;

        // 获取用户角色信息
        let profile = auth_data["availableProfiles"]
            .as_array()
            .and_then(|arr| arr.get(0))
            .ok_or("没有可用角色")?;

        // 获取皮肤数据，构造新的 texture_url
        let profile_id = profile["id"].as_str().unwrap_or_default();
        let texture_url = texture_url(profile_id);

        // 获取皮肤数据
        let texture_response = client
            .get(&texture_url) // 使用 GET 请求来获取皮肤数据
            .send().await
            .map_err(|e| format!("获取皮肤失败: {}", e))?;

        let texture_data: Value = texture_response.json().await
            .map_err(|e| format!("解析皮肤数据失败: {}", e))?;

        // 解码 base64 字符串
        let texture_base64 = texture_data["properties"][0]["value"]
            .as_str()
            .ok_or("没有找到纹理数据")?;

        let decoded = STANDARD.decode(texture_base64)
            .map_err(|e| format!("解码失败: {}", e))?;

        // 解析 JSON 数据
        let decoded_json: Value = serde_json::from_slice(&decoded)
            .map_err(|e| format!("解析纹理 JSON 失败: {}", e))?;

        // 提取皮肤 URL
        let skin_url = decoded_json["textures"]["SKIN"]["url"]
            .as_str()
            .ok_or("没有找到皮肤 URL")?;

        // 构造用户信息
        let user = UserInfo {
            name: profile["name"].as_str().unwrap_or_default().into(),
            uuid: profile_id.into(),
            access_token: auth_data["accessToken"].as_str().unwrap_or_default().into(),
            skin_url: skin_url.into(), // 使用正确的皮肤 URL
            auth_type: "Yggdrasil".into(),
        };

        // 更新认证状态
        let mut auth_state = state.lock().unwrap();
        if let Some(existing) = auth_state.users.iter_mut().find(|u| u.uuid == user.uuid) {
            *existing = user.clone();
        } else {
            auth_state.users.push(user.clone());
        }
        auth_state.current_user_id = Some(user.uuid.clone());
        auth_state.save().map_err(|e| format!("保存失败: {}", e))?;

        println!("用户登录成功: {} ({})", user.name, user.uuid);
        Ok(user)
    } else {
        Err("账号或密码错误".into())
    }
}

#[tauri::command]
fn get_current_user(state: tauri::State<'_, Mutex<AuthState>>) -> Option<UserInfo> {
    let auth_state = state.lock().unwrap();
    if let Some(uuid) = &auth_state.current_user_id {
        auth_state.users.iter().find(|u| &u.uuid == uuid).cloned()
    } else {
        None
    }
}

#[tauri::command]
fn get_total_memory() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024
}

#[tauri::command]
fn get_used_memory() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.used_memory() / 1024 / 1024
}

#[tauri::command]
fn scan_java_environments() -> Vec<JavaInfo> {
    println!("开始扫描 Java 环境...");

    // 获取系统中 java 路径
    let paths: Vec<String> = if cfg!(target_os = "windows") {
        match Command::new("where").arg("java").output() {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stdout);
                println!("where java 输出:\n{}", s);
                s.lines().map(|s| s.to_string()).collect()
            }
            Err(e) => {
                println!("执行 where java 出错: {:?}", e);
                vec![]
            }
        }
    } else {
        match Command::new("which").arg("-a").arg("java").output() {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stdout);
                println!("which -a java 输出:\n{}", s);
                s.lines().map(|s| s.to_string()).collect()
            }
            Err(e) => {
                println!("执行 which -a java 出错: {:?}", e);
                vec![]
            }
        }
    };

    println!("找到 Java 路径: {:?}", paths);

    let mut result = Vec::new();

    for path in paths {
        println!("检测路径: {}", path);
        let version_output = Command::new(&path).arg("-version").output();
        let version_str = match version_output {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stderr);
                let first_line = s.lines().next().unwrap_or("Unknown").to_string();
                println!("{} 版本信息: {}", path, first_line);
                first_line
            }
            Err(e) => {
                println!("{} 获取版本失败: {:?}", path, e);
                "Unknown".to_string()
            }
        };
        result.push(JavaInfo { path, version: version_str });
    }

    println!("扫描完成，结果: {:?}", result);
    result
}

#[tauri::command]
fn get_config() -> Config {
    Config::load()
}

#[tauri::command]
fn save_config(config: Config) -> bool {
    config.save().is_ok()
}

#[tauri::command]
fn logout_current_user() -> bool {
    fs::remove_file(AuthState::file_path()).ok();
    true
}

// -------------------- 启动 --------------------

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(AuthState::load()))
        .invoke_handler(tauri::generate_handler![
            ms_login_command,
            yggdrasil_login,
            get_total_memory,
            get_used_memory,
            scan_java_environments,
            get_current_user,
            get_config,
            save_config,
            logout_current_user
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}