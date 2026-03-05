use crate::models::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::sync::Mutex;
use sysinfo::System;
use tauri::{AppHandle, Emitter, State};
use crate::utils::validate_java;

#[derive(Deserialize)]
pub struct VersionConfig {
    pub arguments: Arguments,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    pub libraries: Vec<Library>,
}

#[derive(Deserialize)]
pub struct Library {
    pub downloads: LibraryDownloads,
}

#[derive(Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Artifact,
}

#[derive(Deserialize)]
pub struct Artifact {
    pub path: String,
}

#[derive(Deserialize)]
pub struct Arguments {
    pub game: Vec<Argument>,
    pub jvm: Vec<Argument>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Simple(String),
    Conditional {
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    Single(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
pub struct Rule {
    pub action: String,
    pub os: Option<OSRule>,
}

#[derive(Deserialize)]
pub struct OSRule {
    pub name: Option<String>,
    #[allow(dead_code)]
    pub arch: Option<String>,
}

impl Argument {
    pub fn resolve(&self, env: &HashMap<&str, String>) -> Vec<String> {
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

#[tauri::command]
pub async fn launch_minecraft(
    app: AppHandle,
    auth_state: State<'_, Mutex<AuthState>>,
    config_state: State<'_, Mutex<Config>>,
) -> Result<(), String> {
    // 进度反馈闭包
    let emit_progress = |msg: &str| {
        let _ = app.emit("launch-status", msg);
    };

    emit_progress("正在校验用户身份...");
    let auth = auth_state.lock().unwrap();
    let config = config_state.lock().unwrap();

    let user = auth.current_user_id.as_ref()
        .and_then(|id| auth.users.iter().find(|u| &u.uuid == id))
        .ok_or("User not logged in")?;

    emit_progress("正在准备文件系统...");
    let base_dir = std::env::current_exe().map_err(|e| e.to_string())?.parent().unwrap().to_path_buf();
    let mc_dir = base_dir.join(".minecraft");

    let version_name = "CirCube Zero";
    let version_json_path = mc_dir.join(format!("versions/{0}/{0}.json", version_name));
    let raw_json = fs::read_to_string(&version_json_path).map_err(|_| format!("Missing {}.json", version_name))?;
    let ver_cfg: VersionConfig = serde_json::from_str(&raw_json).map_err(|e| e.to_string())?;

    emit_progress("正在扫描依赖库...");
    let libs_base = mc_dir.join("libraries");
    let cp_sep = if cfg!(windows) { ";" } else { ":" };

    let mut cp_list = Vec::new();
    let mut mp_list = Vec::new();

    for lib in &ver_cfg.libraries {
        let current_os = std::env::consts::OS;
        let path_str = &lib.downloads.artifact.path;
        if current_os == "windows" {
            if path_str.contains("linux") || path_str.contains("macos") { continue; }
        }
        if current_os == "linux" {
            if path_str.contains("windows") || path_str.contains("macos") { continue; }
        }
        if current_os == "macos" {
            if path_str.contains("windows") || path_str.contains("linux") { continue; }
        }

        let lib_path = libs_base.join(path_str);
        if lib_path.exists() {
            let p_str = lib_path.to_string_lossy().to_string();
            cp_list.push(p_str.clone());
            if path_str.contains("bootstraplauncher") ||
               path_str.contains("securejarhandler") ||
               path_str.contains("ow2/asm") ||
               path_str.contains("JarJarFileSystems") {
                mp_list.push(p_str);
            }
        }
    }

    emit_progress("正在构建环境变量...");
    let mut env = HashMap::new();
    let cp_str = cp_list.join(cp_sep);

    env.insert("${auth_player_name}", user.name.clone());
    env.insert("${auth_uuid}", user.uuid.clone());
    env.insert("${auth_access_token}", user.access_token.clone());
    env.insert("${user_type}", "msa".into());
    env.insert("${clientid}", "circube".into());
    env.insert("${auth_xuid}", "0".into());
    env.insert("${version_name}", version_name.into());
    env.insert("${game_directory}", mc_dir.join(format!("versions/{}", version_name)).to_string_lossy().into());
    env.insert("${assets_root}", mc_dir.join("assets").to_string_lossy().into());
    env.insert("${assets_index_name}", "5".into());
    env.insert("${version_type}", "CirCube".into());
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

    emit_progress("正在优化内存配置...");
    let mut final_max_memory = config.max_memory;
    if final_max_memory == 0 {
        let mut sys = System::new_all();
        sys.refresh_memory();
        let total_mb = sys.total_memory() / 1024 / 1024;
        let used_mb = sys.used_memory() / 1024 / 1024;
        let available = total_mb.saturating_sub(used_mb).saturating_sub(512);
        let recommendation = ((available as f64 * 0.75) / 512.0).floor() as u64 * 512;
        final_max_memory = std::cmp::max(2048, recommendation);
    }

    let mut final_args = Vec::new();
    final_args.push(format!("-Xms{}M", final_max_memory));
    final_args.push(format!("-Xmx{}M", final_max_memory));
    final_args.push("-Dfile.encoding=UTF-8".into());

    if user.auth_type == "Yggdrasil" {
        let injector_path = mc_dir.join("authlib-injector.jar");
        if injector_path.exists() {
            final_args.push(format!("-javaagent:{}={}", injector_path.to_string_lossy(), "https://littleskin.cn/api/yggdrasil"));
            final_args.push(format!("-Dauthlibinjector.yggdrasil.prefetched={}", "ewogICAgIm1ldGEiOiB7CiAgICAgICAgInNlcnZlck5hbWUiOiAiTGl0dGxlU2tpbiIsCiAgICAgICAgImltcGxlbWVudGF0aW9uTmFtZSI6ICJZZ2dkcmFzaWwgQ29ubmVjdCIsCiAgICAgICAgImltcGxlbWVudGF0aW9uVmVyc2lvbiI6ICIwLjAuOCIsCiAgICAgICAgImxpbmtzIjogewogICAgICAgICAgICAiYW5ub3VuY2VtZW50IjogImh0dHBzOi8vbGl0dGxlc2tpbi5jbi9hcGkvYW5ub3VuY2VtZW50cyIsCiAgICAgICAgICAgICJob21lcGFnZSI6ICJodHRwczovL2xpdHRsZXNraW4uY24iLAogICAgICAgICAgICAicmVnaXN0ZXIiOiAiaHR0cHM6Ly9saXR0bGVza2luLmNuL2F1dGgvcmVnaXN0ZXIiCiAgICAgICAgfSwKICAgICAgICAiZmVhdHVyZS5ub25fZW1haWxfbG9naW4iOiB0cnVlLAogICAgICAgICJmZWF0dXJlLmVuYWJsZV9wcm9maWxlX2tleSI6IHRydWUsCiAgICAgICAgImZlYXR1cmUub3BlbmlkX2NvbmZpZ3VyYXRpb25fdXJsIjogImh0dHBzOi8vb3Blbi5saXR0bGVza2luLmNuLy53ZWxsLWtub3duL29wZW5pZC1jb25maWd1cmF0aW9uIgogICAgfSwKICAgICJza2luRG9tYWlucyI6IFsKICAgICAgICAibGl0dGxlc2tpbi5jbiIKICAgIF0sCiAgICAic2lnbmF0dXJlUHVibGlja2V5IjogIi0tLS0tQkVHSU4gUFVCTElDIEtFWS0tLS0tXG5NSUlDSWpBTkJna3Foa2lHOXcwQkFRRUZBQU9DQWc4QU1JSUNDZ0tDQWdFQXJHY05PT0ZJcUxKU3FvRTN1MGhqXG50T0VuT2NFVDN3ajlEcnNzMUJFNnNCcWdQbzBiTXVsT1VMaHFqa2MvdUgvd3lvc1luenczeGFhekp0ODdqVEhoXG5KOEJQTXhDZVFNb3lFZFJvUzNKbmoxRzBLZXpqNEEyYjYxUEpKTTFEcHZEQWNxUUJZc3JTZHBCSis1Mk1qb0dTXG52Sm9lUU81WFVsSlZRbTIxL0htSm5xc1BoemNBNkhnWTcxUkhZRTV4bmhwV0ppUHhMS1VQdG10NkNOWVVRUW9TXG5vMnYzNlhXZ01tTEJaaEFiTk9QeFlYKzFpb3hLYW1qaExPMjlVaHd0Z1k5VTZQV0VPNy9TQmZYenlSUFR6aFBWXG4ybkhxN0tKcWQ4SUlybHRzbHY2aS80RkVNODFpdlMvbW0rUE4zaFlsSVlLNno2WW1paTFuclFBcGxzSjY3T0dxXG5ZSHRXS092cGZUek9vbGx1Z3NSaWhrQUc0T0I2aE0wUHI0NWpqQzNUSWM3ZU83a09nSWNHVUdVUUd1dXVnREV6XG5KMU45RkZXbk4vSDZQOXVrRmVnNVNtR0M1K3dtVVBaWkN0TkJMcjhvOHNJNUg3UWhLN05nd0NhR0ZvWXVpQUdMXG5nejNrLzNZd0o0MEJid1FheVEyZ0lxZW56K1hPRklBbGFqdisvbnlmY0R2Wkg5dkdOS1A5bFZjSFhVVDVZUm5TXG5aU0hvNWx3dlZyWVVycUVBYmgvekR6OFFNRXlpdWpXdlVrUGhaczlmaDZmaW1VR3h0bThtRklQQ3RQSlZYamVZXG53RDNMdnQzYUlCMUpIZFVUSlIzZUVjNGVJYVRLTXdNUHlKUnpWbjV6S3NpdGFaejNubi9jT0Evd1pDOW9xeUVVXG5tYzloNlpNUlRSVUVFNFR0YUp5ZzlsTUNBd0VBQVE9PVxuLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tXG4iCn0="));
        }
    }

    final_args.push("-DignoreList=bootstraplauncher,securejarhandler,asm-commons,asm-util,asm-analysis,asm-tree,asm,JarJarFileSystems,client-extra,fmlcore,javafmllanguage,lowcodelanguage,mclanguage,forge-,CirCube.jar".into());
    final_args.push(format!("-DlibraryDirectory={}", libs_base.to_string_lossy()));

    if !mp_list.is_empty() {
        final_args.push("-cp".into());
        final_args.push(mp_list.join(cp_sep));
    }

    for arg in &ver_cfg.arguments.jvm {
        final_args.extend(arg.resolve(&env));
    }
    final_args.push(ver_cfg.main_class.clone());
    for arg in &ver_cfg.arguments.game {
        final_args.extend(arg.resolve(&env));
    }
    final_args.retain(|arg| arg != "--demo");

    emit_progress("正在验证JAVA...");

    match validate_java(config.java_path.clone()) {
       Ok(info) => {
            emit_progress(&format!("Java 校验通过: {} ({})", info.version, info.path));
       }
       Err(err) => {
            emit_progress(&format!("Java 验证失败: {}", err));
            return Err(format!("无法启动游戏: {}", err));
       }
    }
    emit_progress("正在启动 JVM...");

    let mut child = Command::new(config.java_path.clone())
        .args(&final_args)
        .current_dir(&mc_dir)
        .spawn()
        .map_err(|e| format!("Launch failed: {}", e))?;

    // 监控进程退出
    let handle_clone = app.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        let _ = handle_clone.emit("game-exited", ());
    });

    emit_progress("游戏已运行");
    Ok(())
}