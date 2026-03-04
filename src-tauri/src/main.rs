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
use std::sync::Arc;
use sysinfo::System;
use tauri_plugin_opener::OpenerExt;
use tauri::Emitter;
use reqwest::Client;


const CLIENT_ID: &str = match option_env!("MS_CLIENT_ID") {
    Some(id) => id,
    None => "DEFAULT_ID_OR_ERROR",
};
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
pub struct UserInfo {
    name: String,
    uuid: String,
    access_token: String,
    skin_url: String,
    #[allow(dead_code)]
    auth_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McProfile {
    pub id: String,
    pub name: String,
    pub skins: Vec<Skin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skin {
    pub id: String,
    pub url: String,
    pub variant: String,
}

#[derive(Serialize)]
#[serde(tag = "status", content = "data")]
pub enum AuthResponse {
    #[serde(rename = "success")]
    Success(UserInfo),
    #[serde(rename = "need_selection")]
    NeedSelection {
        profiles: Vec<Profile>,
        access_token: String,
        client_token: String,
    },
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    user_code: String,
    device_code: String,
    verification_uri: String,
    interval: u64,
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
    #[allow(dead_code)]
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


async fn process_user_info(
    client: &reqwest::Client,
    access_token: &serde_json::Value,
    profile_data: &serde_json::Value,
) -> Result<UserInfo, String> {
    let profile_id = profile_data["id"].as_str().ok_or("Invalid profile id")?;
    let profile_name = profile_data["name"].as_str().unwrap_or("Player");

    let texture_url = format!("https://littleskin.cn/api/yggdrasil/sessionserver/session/minecraft/profile/{}", profile_id);
    let texture_res = client.get(&texture_url).send().await
        .map_err(|e| e.to_string())?;

    let texture_data: Value = texture_res.json().await.map_err(|e| e.to_string())?;

    let mut skin_url = String::new();
    if let Some(props) = texture_data["properties"].as_array() {
        if let Some(val_str) = props[0]["value"].as_str() {
            if let Ok(decoded) = STANDARD.decode(val_str) {
                if let Ok(decoded_json) = serde_json::from_slice::<Value>(&decoded) {
                    skin_url = decoded_json["textures"]["SKIN"]["url"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
    }

    Ok(UserInfo {
        name: profile_name.to_string(),
        uuid: profile_id.to_string(),
        access_token: access_token.as_str().unwrap_or("").to_string(),
        skin_url,
        auth_type: "Yggdrasil".into(),
    })
}
// -------------------- Tauri 命令 --------------------

#[tauri::command]
async fn ms_login(app: tauri::AppHandle,  state: tauri::State<'_, Arc<Mutex<AuthState>>>) -> Result<String, String> {
    let client = reqwest::Client::new();

    // 1. 向微软请求设备码
    let params = [
        ("client_id", CLIENT_ID),
        ("scope", "XboxLive.signin offline_access"),
    ];

    let res = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: DeviceCodeResponse = res.json().await.map_err(|e| e.to_string())?;

    // 2. 这里的逻辑：把 user_code 发给前端显示，同时打开验证页面
    let user_code = data.user_code.clone();
    let device_code = data.device_code.clone();
    let interval = data.interval;

    // 自动打开浏览器让用户输入
    app.opener().open_url(&data.verification_uri, None::<String>).map_err(|e| e.to_string())?;

    // 3. 启动后台异步任务进行轮询
    let handle = app.clone();
    let auth_state_arc = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let poll_url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

            let poll_params = [
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", &device_code),
            ];

            let poll_res = client.post(poll_url).form(&poll_params).send().await;

            if let Ok(r) = poll_res {
                let status = r.status();
                let body: serde_json::Value = r.json().await.unwrap_or_default();

                if status.is_success() {
                    // 登录成功！body 里包含 access_token 和 refresh_token
                    let token = body["access_token"].as_str().unwrap_or("");
                    match authenticate_minecraft(token, auth_state_arc.clone()).await {
                                Ok(profile) => {
                                    // 真正成功：所有验证步骤通过，数据已写入 state
                                    handle.emit("ms-login-success", profile).unwrap();
                                }
                                Err(e) => {
                                    // 逻辑失败：例如 Xbox 验证失败或没有购买游戏
                                    handle.emit("ms-login-error", format!("验证失败: {}", e)).unwrap();
                                }
                            }
                    break;
                } else {
                    let error = body["error"].as_str().unwrap_or("");
                    if error == "authorization_pending" {
                        continue; // 用户还没输完，继续等
                    } else {
                        handle.emit("ms-status", format!("错误: {}", error)).unwrap();
                        break;
                    }
                }
            }
        }
    });

    Ok(user_code)
}

async fn authenticate_minecraft(
    ms_access_token: &str,
    state: Arc<Mutex<AuthState>>,
) -> Result<McProfile, String> {
    let client = Client::new();

    // --- XBL ---
    println!("[Auth] Step 1: Requesting XBL Token...");
    let xbl_res = client.post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms_access_token)
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .send().await.map_err(|e| format!("XBL Request Failed: {}", e))?;

    let xbl_data: serde_json::Value = xbl_res.json().await.map_err(|e| format!("XBL JSON Parse Error: {}", e))?;
    let xbl_token = xbl_data["Token"].as_str().ok_or("XBL Token missing in response")?;
    let user_hash = xbl_data["DisplayClaims"]["xui"][0]["uhs"]
        .as_str().ok_or("UHS (User Hash) missing in response")?;
    println!("[Auth] Step 1 Success: Got UHS {}", user_hash);

    // --- XSTS ---
    println!("[Auth] Step 2: Requesting XSTS Token...");
    let xsts_res = client.post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .json(&serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token]
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send().await.map_err(|e| format!("XSTS Request Failed: {}", e))?;

    let xsts_status = xsts_res.status();
    let xsts_data: serde_json::Value = xsts_res.json().await.map_err(|e| format!("XSTS JSON Parse Error: {}", e))?;

    // Xbox 账号可能没有 Minecraft 权限或未成年
    if !xsts_status.is_success() {
        return Err(format!("XSTS Auth Failed with status {}: {}", xsts_status, xsts_data));
    }

    let xsts_token = xsts_data["Token"].as_str().ok_or("XSTS Token missing")?;
    println!("[Auth] Step 2 Success: XSTS Token obtained: {}", xsts_token);

    // --- Minecraft 登录 ---
    println!("[Auth] Step 3: Logging into Minecraft Services...");
    let identity_token = format!("XBL3.0 x={};{}", user_hash, xsts_token);

    let mc_login_res = client.post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({
            "identityToken": identity_token
        }))
        .send().await.map_err(|e| format!("MC Login Request Failed: {}", e))?;

    let mc_status = mc_login_res.status();
    let mc_body_text = mc_login_res.text().await.map_err(|e| e.to_string())?;

    if !mc_status.is_success() {
        println!("[Auth] Step 3 FAILED. Status: {}, Body: {}", mc_status, mc_body_text);
        return Err(format!("Minecraft Auth Server returned {}", mc_status));
    }

    // 只有成功了再解析 JSON
    let mc_data: serde_json::Value = serde_json::from_str(&mc_body_text).map_err(|e| e.to_string())?;
    let mc_access_token = mc_data["access_token"].as_str().ok_or("MC Access Token missing in JSON")?;
    println!("[Auth] Step 3 Success: MC Access Token length: {}", mc_access_token.len());

    // --- 第四步：获取 Profile ---
    println!("[Auth] Step 4: Fetching Minecraft Profile...");
    let profile_res = client.get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(mc_access_token)
        .send().await.map_err(|e| format!("Profile Request Failed: {}", e))?;

    if profile_res.status() == 404 {
        return Err("User does not own Minecraft on this account.".into());
    }

    let profile = profile_res.json::<McProfile>().await.map_err(|e| format!("Profile Parse Error: {}", e))?;
    println!("[Auth] Step 4 Success: Found player {}", profile.name);

    // ====== 写入状态 ======
    println!("[Auth] Final Step: Writing state to memory and disk...");

    // 尽量缩短 Mutex 锁定时间
    {
        let mut s = state.lock().map_err(|_| "Failed to acquire lock: Mutex is poisoned")?;

        // 记录原始数量对比
        let old_count = s.users.len();
        s.users.retain(|u| u.uuid != profile.id);

        s.users.push(UserInfo {
            uuid: profile.id.clone(),
            name: profile.name.clone(),
            access_token: mc_access_token.to_string(),
            auth_type: "Microsoft".into(),
            skin_url: "".into(),
        });

        s.current_user_id = Some(profile.id.clone());

        // 捕获持久化错误
        match s.save() {
            Ok(_) => println!("[Auth] State saved successfully. Users count: {} -> {}", old_count, s.users.len()),
            Err(e) => println!("[Auth] CRITICAL: State memory updated but disk save failed: {}", e),
        }
    }

    Ok(profile)
}

#[tauri::command]
async fn yggdrasil_login(
    payload: AuthPayload,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<AuthResponse, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "agent": { "name": "Minecraft", "version": 1 },
        "username": payload.email,
        "password": payload.password,
        "requestUser": true
    });

    let res = client.post("https://littleskin.cn/api/yggdrasil/authserver/authenticate")
        .json(&body).send().await.map_err(|e| e.to_string())?;

    let data: Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(msg) = data.get("errorMessage").and_then(|v| v.as_str()) {
        return Err(msg.to_string());
    }

    if let Some(selected) = data.get("selectedProfile").filter(|v| !v.is_null()) {
        // 情况 1: 已有角色，直接进入
        let user = process_user_info(&client, &data["accessToken"], selected).await?;

        let mut s = state.lock().unwrap();
        s.users.retain(|u| u.uuid != user.uuid);
        s.users.push(user.clone());
        s.current_user_id = Some(user.uuid.clone());
        let _ = s.save();

        Ok(AuthResponse::Success(user))
    } else {
        // 情况 2: 需要选角
        Ok(AuthResponse::NeedSelection {
            profiles: serde_json::from_value(data["availableProfiles"].clone()).unwrap_or_default(),
            access_token: data["accessToken"].as_str().unwrap_or("").to_string(),
            client_token: data["clientToken"].as_str().unwrap_or("").to_string(),
        })
    }
}

#[tauri::command]
async fn yggdrasil_select(
    access_token: String,
    client_token: String,
    profile: Profile,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<UserInfo, String> {
    let client = reqwest::Client::new();
    let res = client.post("https://littleskin.cn/api/yggdrasil/authserver/refresh")
        .json(&serde_json::json!({
            "accessToken": access_token,
            "clientToken": client_token,
            "selectedProfile": profile
        })).send().await.map_err(|e| e.to_string())?;

    let data: Value = res.json().await.map_err(|e| e.to_string())?;

    // 刷新后通常返回 selectedProfile，使用它构建最终 UserInfo
    let user = process_user_info(&client, &data["accessToken"], &data["selectedProfile"]).await?;

    let mut s = state.lock().unwrap();
    s.users.retain(|u| u.uuid != user.uuid);
    s.users.push(user.clone());
    s.current_user_id = Some(user.uuid.clone());
    let _ = s.save();

    Ok(user)
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

    let base_dir = std::env::current_exe().map_err(|e| e.to_string())?.parent().unwrap().to_path_buf();
    let mc_dir = base_dir.join(".minecraft");
    let version_json_path = mc_dir.join("versions/CirCube/CirCube.json");

    let raw_json = fs::read_to_string(&version_json_path).map_err(|_| "Missing CirCube.json")?;
    let ver_cfg: VersionConfig = serde_json::from_str(&raw_json).map_err(|e| e.to_string())?;

    let libs_base = mc_dir.join("libraries");
    let cp_sep = if cfg!(windows) { ";" } else { ":" };

    // --- 1. 构建 Classpath 和 Module Path ---
    let mut cp_list = Vec::new();
    let mut mp_list = Vec::new();

    for lib in &ver_cfg.libraries {
        let current_os = std::env::consts::OS;
        let path_str = &lib.downloads.artifact.path;
if current_os == "windows" {
    if path_str.contains("linux") || path_str.contains("macos") {
        continue;
    }
}

if current_os == "linux" {
    if path_str.contains("windows") || path_str.contains("macos") {
        continue;
    }
}

if current_os == "macos" {
    if path_str.contains("windows") || path_str.contains("linux") {
        continue;
    }
}

        let lib_path = libs_base.join(path_str);
        if lib_path.exists() {
            let p_str = lib_path.to_string_lossy().to_string();
            cp_list.push(p_str.clone());

            // Forge 1.20.1 关键：特定库必须加入 Module Path (-p)
            if path_str.contains("bootstraplauncher") ||
               path_str.contains("securejarhandler") ||
               path_str.contains("ow2/asm") ||
               path_str.contains("JarJarFileSystems") {
                mp_list.push(p_str);
            }
        }
    }


    // --- 2. 准备环境变量 ---
    let mut env = HashMap::new();
    let cp_str = cp_list.join(cp_sep);

    env.insert("${auth_player_name}", user.name.clone());
    env.insert("${auth_uuid}", user.uuid.clone());
    env.insert("${auth_access_token}", user.access_token.clone());
    env.insert("${user_type}", "msa".into());
    env.insert("${clientid}", "circube".into());
    env.insert("${auth_xuid}", "0".into());
    env.insert("${game_directory}", mc_dir.join("versions/CirCube").to_string_lossy().into());
    env.insert("${assets_root}", mc_dir.join("assets").to_string_lossy().into());
    env.insert("${assets_index_name}", "5".into());
    env.insert("${version_name}", "CirCube".into());
    env.insert("${version_type}", "CirCube Launcher".into());
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

    // --- 3. 组装最终参数 ---
    let mut final_args = Vec::new();

    // A. 基础 JVM 参数
    let mut final_max_memory = config.max_memory;

        if final_max_memory == 0 {
            // 执行自动调优逻辑 (同前端 performAutoTune)
            let mut sys = System::new_all();
            sys.refresh_memory();

            let total_mb = sys.total_memory() / 1024 / 1024;
            let used_mb = sys.used_memory() / 1024 / 1024;

            // 逻辑：(总内存 - 已用内存 - 512MB 预留) * 0.75，并向下对齐到 512MB 的倍数
            let available = total_mb.saturating_sub(used_mb).saturating_sub(512);
            let recommendation = ((available as f64 * 0.75) / 512.0).floor() as u64 * 512;

            // 最小保底 2048MB，防止系统过载时算出一个极小值
            final_max_memory = std::cmp::max(2048, recommendation);

            println!("[Launch] Auto-tuning memory: {}MB", final_max_memory);
        }
    final_args.push(format!("-Xms{}M", final_max_memory));
    final_args.push(format!("-Xmx{}M", final_max_memory));
    final_args.push("-Dfile.encoding=UTF-8".into());

    // B. Authlib Injector
    if user.auth_type.clone() == "Yggdrasil" {
        let injector_path = mc_dir.join("authlib-injector.jar");
            if injector_path.exists() {
                final_args.push(format!("-javaagent:{}={}", injector_path.to_string_lossy(), "https://littleskin.cn/api/yggdrasil"));
                final_args.push(format!("-Dauthlibinjector.yggdrasil.prefetched={}", "ewogICAgIm1ldGEiOiB7CiAgICAgICAgInNlcnZlck5hbWUiOiAiTGl0dGxlU2tpbiIsCiAgICAgICAgImltcGxlbWVudGF0aW9uTmFtZSI6ICJZZ2dkcmFzaWwgQ29ubmVjdCIsCiAgICAgICAgImltcGxlbWVudGF0aW9uVmVyc2lvbiI6ICIwLjAuOCIsCiAgICAgICAgImxpbmtzIjogewogICAgICAgICAgICAiYW5ub3VuY2VtZW50IjogImh0dHBzOi8vbGl0dGxlc2tpbi5jbi9hcGkvYW5ub3VuY2VtZW50cyIsCiAgICAgICAgICAgICJob21lcGFnZSI6ICJodHRwczovL2xpdHRsZXNraW4uY24iLAogICAgICAgICAgICAicmVnaXN0ZXIiOiAiaHR0cHM6Ly9saXR0bGVza2luLmNuL2F1dGgvcmVnaXN0ZXIiCiAgICAgICAgfSwKICAgICAgICAiZmVhdHVyZS5ub25fZW1haWxfbG9naW4iOiB0cnVlLAogICAgICAgICJmZWF0dXJlLmVuYWJsZV9wcm9maWxlX2tleSI6IHRydWUsCiAgICAgICAgImZlYXR1cmUub3BlbmlkX2NvbmZpZ3VyYXRpb25fdXJsIjogImh0dHBzOi8vb3Blbi5saXR0bGVza2luLmNuLy53ZWxsLWtub3duL29wZW5pZC1jb25maWd1cmF0aW9uIgogICAgfSwKICAgICJza2luRG9tYWlucyI6IFsKICAgICAgICAibGl0dGxlc2tpbi5jbiIKICAgIF0sCiAgICAic2lnbmF0dXJlUHVibGlja2V5IjogIi0tLS0tQkVHSU4gUFVCTElDIEtFWS0tLS0tXG5NSUlDSWpBTkJna3Foa2lHOXcwQkFRRUZBQU9DQWc4QU1JSUNDZ0tDQWdFQXJHY05PT0ZJcUxKU3FvRTN1MGhqXG50T0VuT2NFVDN3ajlEcnNzMUJFNnNCcWdQbzBiTXVsT1VMaHFqa2MvdUgvd3lvc1luenczeGFhekp0ODdqVEhoXG5KOEJQTXhDZVFNb3lFZFJvUzNKbmoxRzBLZXpqNEEyYjYxUEpKTTFEcHZEQWNxUUJZc3JTZHBCSis1Mk1qb0dTXG52Sm9lUU81WFVsSlZRbTIxL0htSm5xc1BoemNBNkhnWTcxUkhZRTV4bmhwV0ppUHhMS1VQdG10NkNOWVVRUW9TXG5vMnYzNlhXZ01tTEJaaEFiTk9QeFlYKzFpb3hLYW1qaExPMjlVaHd0Z1k5VTZQV0VPNy9TQmZYenlSUFR6aFBWXG4ybkhxN0tKcWQ4SUlybHRzbHY2aS80RkVNODFpdlMvbW0rUE4zaFlsSVlLNno2WW1paTFuclFBcGxzSjY3T0dxXG5ZSHRXS092cGZUek9vbGx1Z3NSaWhrQUc0T0I2aE0wUHI0NWpqQzNUSWM3ZU83a09nSWNHVUdVUUd1dXVnREV6XG5KMU45RkZXbk4vSDZQOXVrRmVnNVNtR0M1K3dtVVBaWkN0TkJMcjhvOHNJNUg3UWhLN05nd0NhR0ZvWXVpQUdMXG5nejNrLzNZd0o0MEJid1FheVEyZ0lxZW56K1hPRklBbGFqdisvbnlmY0R2Wkg5dkdOS1A5bFZjSFhVVDVZUm5TXG5aU0hvNWx3dlZyWVVycUVBYmgvekR6OFFNRXlpdWpXdlVrUGhaczlmaDZmaW1VR3h0bThtRklQQ3RQSlZYamVZXG53RDNMdnQzYUlCMUpIZFVUSlIzZUVjNGVJYVRLTXdNUHlKUnpWbjV6S3NpdGFaejNubi9jT0Evd1pDOW9xeUVVXG5tYzloNlpNUlRSVUVFNFR0YUp5ZzlsTUNBd0VBQVE9PVxuLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tXG4iCn0="));
            }
    }


    // C. Forge 专属系统变量
    final_args.push("-DignoreList=bootstraplauncher,securejarhandler,asm-commons,asm-util,asm-analysis,asm-tree,asm,JarJarFileSystems,client-extra,fmlcore,javafmllanguage,lowcodelanguage,mclanguage,forge-,CirCube.jar".into());
    final_args.push(format!("-DlibraryDirectory={}", libs_base.to_string_lossy()));

    // D. Module Path
    if !mp_list.is_empty() {
        final_args.push("-cp".into());
        final_args.push(mp_list.join(cp_sep));
    }

    // E. 注入 Version JSON 中的 JVM 参数 (含 add-opens 等)
    for arg in &ver_cfg.arguments.jvm {
        final_args.extend(arg.resolve(&env));
    }

    // F. Main Class
    final_args.push(ver_cfg.main_class.clone());

    for arg in &ver_cfg.arguments.game {
        final_args.extend(arg.resolve(&env));
    }
    final_args.retain(|arg| arg != "--demo");

    // 5. 执行启动
    let java_exec = if config.java_path.is_empty() { "java" } else { &config.java_path };

    // 调试输出：检查是否还存在 ${...}
    // println!("Args: {:?}", final_args);

    Command::new(java_exec)
        .args(&final_args)
        .current_dir(&mc_dir)
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
fn logout_current_user(state: tauri::State<'_, Mutex<AuthState>>) -> bool {
    let mut auth = state.lock().unwrap();
    auth.users.clear();
    auth.current_user_id = None;

    let path = AuthState::file_path();
    if path.exists() {
        if let Err(e) = fs::remove_file(&path) {
            println!("Failed to remove auth_state.json: {:?}", e);
            return false;
        }
    }
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
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(AuthState::load()))
        .manage(Mutex::new(Config::load()))
        .manage(Arc::new(Mutex::new(AuthState::load())))
        .invoke_handler(tauri::generate_handler![
            ms_login,
            yggdrasil_login,
            yggdrasil_select,
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