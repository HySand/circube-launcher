use crate::models::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::sync::Mutex;
use sysinfo::System;
use tauri::{AppHandle, Emitter, State};
use crate::utils::validate_java;
use crate::auth::ensure_authenticated;
use tauri::Manager;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Debug, Clone)]
pub struct VersionConfig {
    pub arguments: Arguments,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    pub libraries: Vec<Library>,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndex>,
    #[serde(rename = "assets")]
    pub assets: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssetIndex {
    pub id: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Library {
    pub downloads: LibraryDownloads,
    pub rules: Option<Vec<Rule>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LibraryDownloads {
    pub artifact: Artifact,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Artifact {
    pub path: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Arguments {
    pub game: Vec<Argument>,
    pub jvm: Vec<Argument>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Argument {
    Simple(String),
    Conditional {
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum ArgumentValue {
    Single(String),
    Many(Vec<String>),
}

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    pub action: String,
    pub os: Option<OSRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OSRule {
    pub name: Option<String>,
}

impl Argument {
    pub fn resolve(&self, env: &HashMap<String, String>, launcher_features: &HashMap<String, bool>) -> Vec<String> {
        match self {
            Argument::Simple(s) => vec![Self::replace_vars(s, env)],
            Argument::Conditional { rules, value } => {
                if Self::evaluate_rules(rules, launcher_features) {
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

    fn evaluate_rules(rules: &[Rule], launcher_features: &HashMap<String, bool>) -> bool {
        if rules.is_empty() {
            return true;
        }

        let mut allowed = true;
        for rule in rules {
            let mut matched = true;

            if let Some(ref os_rule) = rule.os {
                let current_os = if cfg!(target_os = "windows") { "windows" }
                                 else if cfg!(target_os = "macos") { "osx" }
                                 else { "linux" };

                if let Some(ref name) = os_rule.name {
                    if name != current_os {
                        matched = false;
                    }
                }
            }

            if let Some(ref features) = rule.features {
                for (key, val) in features {
                    let launcher_val = launcher_features.get(key).copied().unwrap_or(false);
                    if launcher_val != *val {
                        matched = false;
                        break;
                    }
                }
            }

            if rule.action == "allow" {
                if !matched { allowed = false; }
            } else if rule.action == "disallow" {
                if matched { allowed = false; }
            }
        }
        allowed
    }

    fn replace_vars(template: &str, env: &HashMap<String, String>) -> String {
        let mut result = template.to_string();
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len()));

        for key in keys {
            if let Some(val) = env.get(key) {
                result = result.replace(key, val);
            }
        }
        result
    }
}

fn get_launcher_features(env: &HashMap<String, String>) -> HashMap<String, bool> {
    let mut features = HashMap::new();
    features.insert("is_demo_user".to_string(), false);
    features.insert("has_custom_resolution".to_string(),
        env.get("${resolution_width}").map_or(false, |w| w != "854") ||
        env.get("${resolution_height}").map_or(false, |h| h != "480")
    );
    features.insert("has_quick_plays_support".to_string(), true);
    features.insert("is_quick_play_singleplayer".to_string(), false);
    features.insert("is_quick_play_multiplayer".to_string(), false);
    features.insert("is_quick_play_realms".to_string(), false);
    features
}

#[tauri::command]
pub async fn launch_minecraft(
    app: AppHandle,
    auth_state: State<'_, Mutex<AuthState>>,
    config_state: State<'_, Mutex<Config>>,
) -> Result<(), String> {
    let emit_progress = |msg: &str| {
        let _ = app.emit("launch-status", msg);
    };

    emit_progress("正在校验用户身份...");

    let (java_path, max_memory_config) = {
        let config = config_state.lock().unwrap();
        (
            config.java_path.clone(),
            config.max_memory,
        )
    };

    let (user_uuid, auth_type, initial_token) = {
        let auth = auth_state.lock().unwrap();
        let user = auth.current_user_id.as_ref()
            .and_then(|id| auth.users.iter().find(|u| &u.uuid == id))
            .ok_or_else(|| "User not logged in".to_string())?;
        (user.uuid.clone(), user.auth_type.clone(), user.access_token.clone())
    };

    ensure_authenticated(&user_uuid, &auth_type, &initial_token, &auth_state, app.clone()).await?;

    let (access_token, user_name) = {
        let auth = auth_state.lock().unwrap();
        let user = auth.users.iter().find(|u| u.uuid == user_uuid)
            .ok_or("User data lost after auth")?;
        (user.access_token.clone(), user.name.clone())
    };

    emit_progress("正在准备文件系统...");
    let base_dir = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Failed to get parent dir")?
        .to_path_buf();
    let mc_dir = base_dir.join(".minecraft");

    let version_name = VERSION.get().map(|s| s.as_str()).unwrap_or("UNKNOWN");

    let version_json_path = mc_dir.join(format!("versions/{0}/{0}.json", version_name));
    let raw_json = fs::read_to_string(&version_json_path)
        .map_err(|_| format!("Missing {}.json", version_name))?;
    let ver_cfg: VersionConfig = serde_json::from_str(&raw_json).map_err(|e| e.to_string())?;

    let assets_index_name = ver_cfg.asset_index
        .as_ref()
        .map(|a| a.id.as_str())
        .or(ver_cfg.assets.as_deref())
        .unwrap_or("1.8");

    emit_progress("正在扫描依赖库...");
    let libs_base = mc_dir.join("libraries");
    let cp_sep = if cfg!(windows) { ";" } else { ":" };

    let mut cp_list: Vec<String> = Vec::new();
    let current_os_name = if cfg!(target_os = "windows") { "windows" }
                          else if cfg!(target_os = "macos") { "osx" }
                          else { "linux" };

    for lib in &ver_cfg.libraries {
        if let Some(rules) = &lib.rules {
            let mut allowed = true;
            for rule in rules {
                let mut matched = true;
                if let Some(ref os_rule) = rule.os {
                    if let Some(ref name) = os_rule.name {
                        if name != current_os_name { matched = false; }
                    }
                }
                if rule.action == "allow" && !matched { allowed = false; }
                if rule.action == "disallow" && matched { allowed = false; }
            }
            if !allowed { continue; }
        }

        let path_str = &lib.downloads.artifact.path;
        let lib_path = libs_base.join(path_str);

        if lib_path.exists() {
            cp_list.push(lib_path.to_string_lossy().to_string());
        }
    }

    let version_jar_path = mc_dir.join(format!("versions/{0}/{0}.jar", version_name));
    if version_jar_path.exists() {
        cp_list.push(version_jar_path.to_string_lossy().to_string());
    } else {
        emit_progress(&format!("警告: 未找到核心文件 {:?}", version_jar_path));
    }

    emit_progress("正在构建环境变量...");
    let mut env: HashMap<String, String> = HashMap::new();

    let cp_str = cp_list.join(cp_sep);

    let assets_dir = mc_dir.join("assets");
    let version_dir = mc_dir.join(format!("versions/{}", version_name));
    let natives_dir = version_dir.join(format!("{}-natives", version_name));

    env.insert("${auth_player_name}".to_string(), user_name.clone());
    env.insert("${auth_uuid}".to_string(), user_uuid.clone());
    env.insert("${auth_access_token}".to_string(), access_token.clone());
    env.insert("${user_type}".to_string(), "msa".to_string());
    env.insert("${clientid}".to_string(), "circube".to_string());
    env.insert("${versionType}".to_string(), "CirCube Launcher".to_string());
    env.insert("${auth_xuid}".to_string(), "0".to_string());
    env.insert("${version_name}".to_string(), version_name.to_string());
    env.insert("${game_directory}".to_string(), version_dir.to_string_lossy().to_string());
    env.insert("${assets_root}".to_string(), assets_dir.to_string_lossy().to_string());
    env.insert("${assets_index_name}".to_string(), assets_index_name.to_string());
    env.insert("${version_type}".to_string(), "CirCube Launcher".to_string());
    env.insert("${natives_directory}".to_string(), natives_dir.to_string_lossy().to_string());
    env.insert("${library_directory}".to_string(), libs_base.to_string_lossy().to_string());
    env.insert("${classpath_separator}".to_string(), cp_sep.to_string());
    env.insert("${classpath}".to_string(), cp_str);
    env.insert("${resolution_width}".to_string(), "1600".to_string());
    env.insert("${resolution_height}".to_string(), "900".to_string());
    env.insert("${quickPlayPath}".to_string(), "".to_string());
    env.insert("${quickPlaySingleplayer}".to_string(), "".to_string());
    env.insert("${quickPlayMultiplayer}".to_string(), "".to_string());
    env.insert("${quickPlayRealms}".to_string(), "".to_string());
    env.insert("${launcher_name}".to_string(), "CirCube Launcher".to_string());
    env.insert("${launcher_version}".to_string(), LAUNCHER_VERSION.to_string());

    let launcher_features = get_launcher_features(&env);

    emit_progress("正在优化内存配置...");
    let mut final_max_memory = max_memory_config;
    if final_max_memory == 0 {
        let mut sys = System::new_all();
        sys.refresh_memory();
        let total_mb = sys.total_memory() / 1024 / 1024;
        let used_mb = sys.used_memory() / 1024 / 1024;
        let available = total_mb.saturating_sub(used_mb).saturating_sub(512);
        let recommendation = ((available as f64 * 0.75) / 512.0).floor() as u64 * 512;
        final_max_memory = std::cmp::max(2048, recommendation);
    }

    let mut final_args: Vec<String> = Vec::new();
    final_args.push(format!("-Xms{}M", final_max_memory));
    final_args.push(format!("-Xmx{}M", final_max_memory));
    final_args.push("-Dfile.encoding=UTF-8".to_string());

    if auth_type == "Yggdrasil" {
        let injector_dir = base_dir.join("launcher");
        let injector_path = injector_dir.join("authlib-injector.jar");

        if !injector_path.exists() {
            emit_progress("正在获取认证插件最新版本信息...");

            let download_client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| e.to_string())?;

            let latest_json_url = "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/artifact/latest.json";
            let res = download_client.get(latest_json_url)
                .send().await.map_err(|e| format!("获取元数据失败：{}", e))?;

            #[derive(serde::Deserialize)]
            struct InjectorMeta {
                download_url: String,
                version: String,
            }

            let meta: InjectorMeta = res.json().await.map_err(|e| format!("解析元数据失败：{}", e))?;

            emit_progress(&format!("正在下载 authlib-injector v{}...", meta.version));

            let response = download_client.get(&meta.download_url)
                .send().await.map_err(|e| format!("下载请求失败：{}", e))?;

            if response.status().is_success() {
                std::fs::create_dir_all(&injector_dir).map_err(|e| e.to_string())?;
                let bytes = response.bytes().await.map_err(|e| e.to_string())?;
                std::fs::write(&injector_path, bytes).map_err(|e| e.to_string())?;
            } else {
                return Err(format!("下载服务器响应异常：{}", response.status()));
            }
        }

        if injector_path.exists() {
            final_args.push(format!("-javaagent:{}={}", injector_path.to_string_lossy(), "https://littleskin.cn/api/yggdrasil"));
            final_args.push(format!("-Dauthlibinjector.yggdrasil.prefetched={}", "eyJtZXRhIjp7InNlcnZlck5hbWUiOiJMaXR0bGVTa2luIiwiaW1wbGVtZW50YXRpb25OYW1lIjoiWWdnZHJhc2lsIENvbm5lY3QiLCJpbXBsZW1lbnRhdGlvblZlcnNpb24iOiIwLjAuOCIsImxpbmtzIjp7ImFubm91bmNlbWVudCI6Imh0dHBzOi8vbGl0dGxlc2tpbi5jbi9hcGkvYW5ub3VuY2VtZW50cyIsImhvbWVwYWdlIjoiaHR0cHM6Ly9saXR0bGVza2luLmNuIiwicmVnaXN0ZXIiOiJodHRwczovL2xpdHRsZXNraW4uY24vYXV0aC9yZWdpc3RlciJ9LCJmZWF0dXJlLm5vbl9lbWFpbF9sb2dpbiI6dHJ1ZSwiZmVhdHVyZS5lbmFibGVfcHJvZmlsZV9rZXkiOnRydWUsImZlYXR1cmUub3BlbmlkX2NvbmZpZ3VyYXRpb25fdXJsIjoiaHR0cHM6Ly9vcGVuLmxpdHRsZXNraW4uY24vLndlbGwta25vd24vb3BlbmlkLWNvbmZpZ3VyYXRpb24ifSwic2tpbkRvbWFpbnMiOlsibGl0dGxlc2tpbi5jbiJdLCJzaWduYXR1cmVQdWJsaWNrZXkiOiItLS0tLUJFR0lOIFBVQkxJQyBLRVktLS0tLVxuTUlJQ0lqQU5CZ2txaGtpRzl3MEJBUUVGQUFPQ0FnOEFNSUlDQ2dLQ0FnRUFyR2NOT09GSXFMSlNxb0UzdTBoalxudE9Fbk9jRVQzd2o5RHJzczFCRTZzQnFnUG8wYk11bE9VTGhxamtjL3VIL3d5b3NZbnp3M3hhYXpKdDg3alRIaFxuSjhCUE14Q2VRTW95RWRSb1MzSm5qMUcwS2V6ajRBMmI2MVBKSk0xRHB2REFjcVFCWXNyU2RwQkorNTJNam9HU1xudkpvZVFPNVhVbEpWUW0yMS9IbUpucXNQaHpjQTZIZ1k3MVJIWUU1eG5ocFdKaVB4TEtVUHRtdDZDTllVUVFvU1xubzJ2MzZYV2dNbUxCWmhBYk5PUHhZWCsxaW94S2FtamhMTzI5VWh3dGdZOVU2UFdFTzcvU0JmWHp5UlBUemhQVlxuMm5IcTdLSnFkOElJcmx0c2x2NmkvNEZFTTgxaXZTL21tK1BOM2hZbElZSzZ6NlltaWkxbnJRQXBsc0o2N09HcVxuWUh0V0tPdnBmVHpPb2xsdWdzUmloa0FHNE9CNmhNMFByNDVqakMzVEljN2VPN2tPZ0ljR1VHVVFHdXV1Z0RFelxuSjFOOUZGV25OL0g2UDl1a0ZlZzVTbUdDNSt3bVVQWlpDdE5CTHI4bzhzSTVIN1FoSzdOZ3dDYUdGb1l1aUFHTFxuZ3ozay8zWXdKNDBCYndRYXlRMmdJcWVueitYT0ZJQWxhanYrL255ZmNEdlpIOXZHTktQOWxWY0hYVVQ1WVJuU1xuWlNIbzVsd3ZWcllVcnFFQWJoL3pEejhRTUV5aXVqV3ZVa1BoWnM5Zmg2ZmltVUd4dG04bUZJUEN0UEpWWGplWVxud0QzTHZ0M2FJQjFKSGRVVEpSM2VFYzRlSWFUS013TVB5SlJ6Vm41ektzaXRhWnozbm4vY09BL3daQzlvcXlFVVxubWM5aDZaTVJUUlVFRTRUdGFKeWc5bE1DQXdFQUFRPT1cbi0tLS0tRU5EIFBVQkxJQyBLRVktLS0tLVxuIn0="));
        }
    }

    // 1. 解析 JVM 参数
    for arg in &ver_cfg.arguments.jvm {
        final_args.extend(arg.resolve(&env, &launcher_features));
    }

    // 2. 添加主类
    final_args.push(ver_cfg.main_class.clone());

    // 3. 解析游戏参数
    for arg in &ver_cfg.arguments.game {
        final_args.extend(arg.resolve(&env, &launcher_features));
    }

    // 4. 清理 demo 标记
    final_args.retain(|arg| arg != "--demo");

    emit_progress("正在验证 JAVA...");
    let java_path_str: String = java_path.clone();
    match validate_java(java_path_str) {
       Ok(info) => {
            emit_progress(&format!("Java 校验通过：{} ({})", info.version, info.path));
       }
       Err(err) => {
            emit_progress(&format!("Java 验证失败：{}", err));
            return Err(format!("无法启动游戏：{}", err));
       }
    }

    emit_progress("正在启动 JVM...");

    let mut cmd = Command::new(java_path);
    cmd.args(&final_args)
       .current_dir(&version_dir);

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Launch failed: {}", e))?;

    let handle_clone = app.clone();

    if let Some(main_window) = app.get_webview_window("main") {
        let _ = main_window.minimize();
    }

    std::thread::spawn(move || {
        let _ = child.wait();
        if let Some(main_window) = handle_clone.get_webview_window("main") {
            let _ = main_window.unminimize();
            let _ = main_window.set_focus();
        }
        let _ = handle_clone.emit("game-exited", ());
    });

    emit_progress("游戏已运行");
    Ok(())
}