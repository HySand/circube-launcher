#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{PathBuf};
use std::process::Command;
use std::sync::Mutex;
use sysinfo::System;

// -------------------- 数据结构 --------------------

#[derive(Deserialize)]
struct AuthPayload {
    email: String,
    password: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub java_path: String,
    pub max_memory: u64,
}

impl Config {
    fn file_path() -> PathBuf {
        let mut path = dirs_next::data_dir().unwrap_or_else(|| std::env::current_dir().unwrap());
        path.push("circube-launcher");
        let _ = fs::create_dir_all(&path);
        path.push("config.json");
        path
    }

    fn load() -> Self {
        fs::read_to_string(Self::file_path())
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or(Config {
                java_path: "".into(),
                max_memory: 4096,
            })
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
    #[allow(dead_code)]
    auth_type: String,
}

#[derive(Serialize)]
struct JavaInfo {
    path: String,
    version: String,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct AuthState {
    current_user_id: Option<String>,
    users: Vec<UserInfo>,
}

impl AuthState {
    fn file_path() -> PathBuf {
        let mut path = dirs_next::data_dir().unwrap_or_else(|| std::env::current_dir().unwrap());
        path.push("circube-launcher");
        let _ = fs::create_dir_all(&path);
        path.push("auth_state.json");
        path
    }

    fn load() -> Self {
        fs::read_to_string(Self::file_path())
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(Self::file_path(), json)
    }
}

// --- Minecraft Version JSON Parser ---

#[derive(Deserialize)]
struct VersionConfig {
    arguments: Arguments,
    #[serde(rename = "mainClass")]
    main_class: String,
    libraries: Vec<Library>,
}

#[derive(Deserialize)]
struct Library {
    downloads: LibraryDownloads,
}

#[derive(Deserialize)]
struct LibraryDownloads {
    artifact: Artifact,
}

#[derive(Deserialize)]
struct Artifact {
    path: String,
}

#[derive(Deserialize)]
struct Arguments {
    game: Vec<Argument>,
    jvm: Vec<Argument>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Argument {
    Simple(String),
    Conditional {
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ArgumentValue {
    Single(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Rule {
    action: String,
    os: Option<OSRule>,
}

#[derive(Deserialize)]
struct OSRule {
    name: Option<String>,
    #[allow(dead_code)] // 暂不处理架构差异
    arch: Option<String>,
}

// -------------------- 解析引擎逻辑 --------------------

impl Argument {
    fn resolve(&self, env: &HashMap<&str, String>) -> Vec<String> {
        match self {
            Argument::Simple(s) => vec![Self::replace_vars(s, env)],
            Argument::Conditional { rules, value } => {
                if Self::evaluate_rules(rules) {
                    match value {
                        ArgumentValue::Single(s) => vec![Self::replace_vars(s, env)],
                        ArgumentValue::Many(v) => v.iter().map(|s| Self::replace_vars(s, env)).collect(),
                    }
                } else {
                    vec![]
                }
            }
        }
    }

    fn evaluate_rules(rules: &[Rule]) -> bool {
        for rule in rules {
            let mut matched = true;
            if let Some(ref os_rule) = rule.os {
                let current_os = if cfg!(target_os = "windows") { "windows" }
                                 else if cfg!(target_os = "macos") { "osx" }
                                 else { "linux" };
                if let Some(ref name) = os_rule.name {
                    if name != current_os { matched = false; }
                }
            }
            if (rule.action == "allow" && !matched) || (rule.action == "disallow" && matched) {
                return false;
            }
        }
        true
    }

    fn replace_vars(template: &str, env: &HashMap<&str, String>) -> String {
        let mut result = template.to_string();
        for (key, val) in env {
            result = result.replace(key, val);
        }
        result
    }
}

// -------------------- Tauri 命令 --------------------

#[tauri::command]
async fn yggdrasil_login(
    payload: AuthPayload,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<UserInfo, String> {
    let client = reqwest::Client::new();
    let auth_url = "https://littleskin.cn/api/yggdrasil/authserver/authenticate";

    let body = serde_json::json!({
        "agent": { "name": "Minecraft", "version": 1 },
        "username": payload.email,
        "password": payload.password,
        "requestUser": true
    });

    let response = client.post(auth_url).json(&body).send().await
        .map_err(|e| format!("Network error: {}", e))?;

    if response.status().is_success() {
        let auth_data: Value = response.json().await.map_err(|e| e.to_string())?;
        let profile = auth_data["availableProfiles"][0].clone();
        let profile_id = profile["id"].as_str().ok_or("Invalid profile")?;

        let texture_url = format!("https://littleskin.cn/api/yggdrasil/sessionserver/session/minecraft/profile/{}", profile_id);
        let texture_data: Value = client.get(&texture_url).send().await
            .map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;

        let texture_base64 = texture_data["properties"][0]["value"].as_str().unwrap_or("");
        let decoded = STANDARD.decode(texture_base64).unwrap_or_default();
        let decoded_json: Value = serde_json::from_slice(&decoded).unwrap_or(serde_json::json!({}));
        let skin_url = decoded_json["textures"]["SKIN"]["url"].as_str().unwrap_or("").to_string();

        let user = UserInfo {
            name: profile["name"].as_str().unwrap_or("Player").into(),
            uuid: profile_id.into(),
            access_token: auth_data["accessToken"].as_str().unwrap_or("").into(),
            skin_url,
            auth_type: "Yggdrasil".into(),
        };

        let mut auth_state = state.lock().unwrap();
        auth_state.users.retain(|u| u.uuid != user.uuid);
        auth_state.users.push(user.clone());
        auth_state.current_user_id = Some(user.uuid.clone());
        let _ = auth_state.save();

        Ok(user)
    } else {
        Err("Authentication failed".into())
    }
}

#[tauri::command]
fn save_config(config: Config, state: tauri::State<'_, Mutex<Config>>) -> Result<(), String> {
    let mut current_config = state.lock().unwrap();
    *current_config = config;
    current_config.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn launch_minecraft(
    auth_state: tauri::State<'_, Mutex<AuthState>>,
    config_state: tauri::State<'_, Mutex<Config>>,
) -> Result<(), String> {
    let auth = auth_state.lock().unwrap();
    let config = config_state.lock().unwrap();

    let user = auth.current_user_id.as_ref()
        .and_then(|id| auth.users.iter().find(|u| &u.uuid == id))
        .ok_or("User not logged in")?;

    // 1. 路径初始化
    let base_dir = std::env::current_exe().map_err(|e| e.to_string())?.parent().unwrap().to_path_buf();
    let mc_dir = base_dir.join(".minecraft");
    let version_json_path = mc_dir.join("versions/CirCube/CirCube.json");

    let raw_json = fs::read_to_string(&version_json_path).map_err(|_| "Missing CirCube.json")?;
    let ver_cfg: VersionConfig = serde_json::from_str(&raw_json).map_err(|e| e.to_string())?;

    // 2. 精确构建 Classpath
    // 不再使用递归扫描，而是解析 JSON 里的 libraries 数组
    let mut cp_list = Vec::new();
    let libs_base = mc_dir.join("libraries");

    for lib in ver_cfg.libraries {
        let lib_path = libs_base.join(&lib.downloads.artifact.path);
        if lib_path.exists() {
            cp_list.push(lib_path);
        } else {
            // 如果你发现启动不了，通常是库没下载全
            println!("Warning: Library missing at {:?}", lib_path);
        }
    }

    // 加入版本核心 JAR
    cp_list.push(mc_dir.join("versions/CirCube/CirCube.jar"));

    let cp_sep = if cfg!(windows) { ";" } else { ":" };
    let cp_str = cp_list.iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(cp_sep);

    // 3. 环境变量插值
    let mut env = HashMap::new();
    env.insert("${auth_player_name}", user.name.clone());
    env.insert("${auth_uuid}", user.uuid.clone());
    env.insert("${auth_access_token}", user.access_token.clone());
    env.insert("${game_directory}", mc_dir.to_string_lossy().into());
    env.insert("${assets_root}", mc_dir.join("assets").to_string_lossy().into());
    env.insert("${assets_index_name}", "5".into());
    env.insert("${version_name}", "CirCube".into());
    env.insert("${version_type}", "CirCube Launcher".into());
    env.insert("${user_type}", "msa".into());
    env.insert("${natives_directory}", mc_dir.join("natives").to_string_lossy().into());
    env.insert("${library_directory}", libs_base.to_string_lossy().into());
    env.insert("${classpath_separator}", cp_sep.into());
    env.insert("${classpath}", cp_str);

    env.insert("${resolution_width}", "854".into());
    env.insert("${resolution_height}", "480".into());
    env.insert("${quickPlayPath}", "".into());
    env.insert("${quickPlaySingleplayer}", "".into());
    env.insert("${quickPlayMultiplayer}", "".into());
    env.insert("${quickPlayRealms}", "".into());

    // 4. 参数装配
    let mut final_args = Vec::new();

    // 注入内存
    final_args.push(format!("-Xmx{}M", config.max_memory));

    // JVM 参数 (处理了模块化路径、add-opens 等)
    for arg in &ver_cfg.arguments.jvm {
        final_args.extend(arg.resolve(&env));
    }

    // 主类
    final_args.push(ver_cfg.main_class.clone());

    // 游戏参数
    for arg in &ver_cfg.arguments.game {
        final_args.extend(arg.resolve(&env));
    }

    // 5. 执行启动
    let java_exec = if config.java_path.is_empty() { "java" } else { &config.java_path };

    // 打印调试信息（可选）
    // println!("Executing: {} {:?}", java_exec, final_args);

    Command::new(java_exec)
        .args(&final_args)
        .current_dir(&mc_dir) // 重要：设置工作目录为 .minecraft
        .spawn()
        .map_err(|e| format!("Launch failed: {}", e))?;

    Ok(())
}

#[tauri::command]
fn get_total_memory() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024
}

#[tauri::command]
fn get_config(s: tauri::State<'_, Mutex<Config>>) -> Config { s.lock().unwrap().clone() }

#[tauri::command]
fn get_current_user(s: tauri::State<'_, Mutex<AuthState>>) -> Option<UserInfo> {
    let auth = s.lock().unwrap();
    auth.current_user_id.as_ref().and_then(|id| auth.users.iter().find(|u| &u.uuid == id).cloned())
}

#[tauri::command]
fn logout_current_user() -> bool {
    fs::remove_file(AuthState::file_path()).ok();
    true
}

#[tauri::command]
fn get_used_memory() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.used_memory() / 1024 / 1024
}

fn parse_java_display_name(full_output: &str) -> String {
    // 1. 定义更宽泛的正则，匹配 version 后跟引号或数字
    let version_regex = Regex::new(r#"(?i)version\s+"?([\d\._]+)"?"#).unwrap();
    let fallback_regex = Regex::new(r#"(?i)build\s+"?([\d\._]+)"?"#).unwrap(); // 某些 JDK 只报 build

    // 2. 在全文中搜索，而不仅仅是第一行
    let mut version_num = if let Some(cap) = version_regex.captures(full_output) {
        cap.get(1).map_or("??".to_string(), |m| m.as_str().to_string())
    } else if let Some(cap) = fallback_regex.captures(full_output) {
        cap.get(1).map_or("??".to_string(), |m| m.as_str().to_string())
    } else {
        // 最后的手段：找第一个看起来像版本号的数字序列
        Regex::new(r#"(\d+\.\d+[\d\._]*)"#).unwrap()
            .captures(full_output)
            .and_then(|cap| cap.get(1))
            .map_or("??".to_string(), |m| m.as_str().to_string())
    };

    // 3. 逻辑处理：1.8 -> 8, 21.0.6 -> 21
    if version_num.starts_with("1.8") {
        version_num = "8".to_string();
    } else {
        version_num = version_num.split('.').next().unwrap_or(&version_num).to_string();
    }

    // 4. 提取厂商 (全文扫描)
    let content_upper = full_output.to_uppercase();
    let vendor = if content_upper.contains("ZULU") { "Zulu" }
        else if content_upper.contains("GRAALVM") { "GraalVM" }
        else if content_upper.contains("MICROSOFT") { "Microsoft" }
        else if content_upper.contains("CORRETTO") { "Corretto" }
        else if content_upper.contains("TEMURIN") || content_upper.contains("ADOPTIUM") { "Temurin" }
        else if content_upper.contains("ORACLE") { "Oracle" }
        else { "OpenJDK" };

    format!("{} {}", vendor, version_num)
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
            let output = Command::new(&path).arg("-version").output();
            if let Ok(out) = output {
                let full_text = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );

                let display_name = parse_java_display_name(&full_text);

                result.push(JavaInfo {
                    path,
                    version: display_name,
                });
            }
        }
        result
}

// -------------------- Main --------------------

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(AuthState::load()))
        .manage(Mutex::new(Config::load()))
        .invoke_handler(tauri::generate_handler![
            yggdrasil_login,
            save_config,
            get_config,
            get_total_memory,
            get_used_memory,
            get_current_user,
            launch_minecraft,
            logout_current_user,
             scan_java_environments
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}