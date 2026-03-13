use crate::models::*;
use regex::Regex;
use std::process::Command;
use sysinfo::System;
use std::path::PathBuf;
use std::collections::HashSet;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[tauri::command]
pub fn get_total_memory() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024
}

#[tauri::command]
pub fn get_used_memory() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.used_memory() / 1024 / 1024
}

pub fn parse_java_display_name(full_output: &str) -> String {
    let version_regex = Regex::new(r#"(?i)version\s+"?([\d\._]+)"?"#).unwrap();
    let fallback_regex = Regex::new(r#"(?i)build\s+"?([\d\._]+)"?"#).unwrap();

    let mut version_num = if let Some(cap) = version_regex.captures(full_output) {
        cap.get(1).map_or("??".to_string(), |m| m.as_str().to_string())
    } else if let Some(cap) = fallback_regex.captures(full_output) {
        cap.get(1).map_or("??".to_string(), |m| m.as_str().to_string())
    } else {
        Regex::new(r#"(\d+\.\d+[\d\._]*)"#).unwrap()
            .captures(full_output)
            .and_then(|cap| cap.get(1))
            .map_or("??".to_string(), |m| m.as_str().to_string())
    };

    // 针对 Minecraft 的版本归一化逻辑：1.8 -> 8, 17.0.x -> 17
    if version_num.starts_with("1.8") {
        version_num = "8".to_string();
    } else {
        version_num = version_num.split('.').next().unwrap_or(&version_num).to_string();
    }

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
pub async fn scan_java_environments() -> Vec<JavaInfo> {
    let mut found_paths: HashSet<PathBuf> = HashSet::new();

    // 1. 基于 PATH 环境变量查询
    let (search_cmd, target_bin) = if cfg!(windows) {
        ("where", "javaw.exe")
    } else {
        ("which", "java")
    };

    // 执行系统搜索指令
    let mut cmd = Command::new(search_cmd);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }
    if !cfg!(windows) { cmd.arg("-a"); }
    cmd.arg(target_bin);

    if let Ok(output) = cmd.output() {
        let s = String::from_utf8_lossy(&output.stdout);
        for line in s.lines() {
            let p = PathBuf::from(line.trim());
            if p.exists() { found_paths.insert(p); }
        }
    }

    // 2. 通过系统特定环境变量探测 (JAVA_HOME)
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let bin_name = if cfg!(windows) { "java.exe" } else { "java" };
        let p = PathBuf::from(java_home).join("bin").join(bin_name);
        if p.exists() { found_paths.insert(p); }
    }

    // 3. macOS 专属：使用系统专有工具 java_home 查询
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("/usr/libexec/java_home").arg("-V").output() {
            let s = String::from_utf8_lossy(&output.stderr);
            // 匹配 java_home 输出中的绝对路径
            let path_re = Regex::new(r#"\s+.*?\s+(/.*)"#).unwrap();
            for line in s.lines() {
                if let Some(cap) = path_re.captures(line) {
                    if let Some(path_str) = cap.get(1) {
                        let p = PathBuf::from(path_str.as_str().trim()).join("bin/java");
                        if p.exists() { found_paths.insert(p); }
                    }
                }
            }
        }
    }

    // 4. 统一元数据解析
    let mut result = Vec::new();
    for path in found_paths {
        let mut cmd = Command::new(&path);
        cmd.arg("-version");

        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }

        if let Ok(out) = cmd.output() {
            let full_text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );

            if !full_text.trim().is_empty() {
                let display_name = parse_java_display_name(&full_text);
                result.push(JavaInfo {
                    path: path.to_string_lossy().to_string(),
                    version: display_name
                });
            }
        }
    }

    // 排序：按版本号降序
    result.sort_by(|a, b| b.version.cmp(&a.version));
    result
}

#[tauri::command]
pub fn validate_java(path: String) -> Result<JavaInfo, String> {
    let path_buf = PathBuf::from(&path);

    if !path_buf.exists() || !path_buf.is_file() {
        return Err("Java 路径不存在或不是文件".into());
    }

    let mut cmd = Command::new(&path_buf);
    cmd.arg("-version");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    match cmd.output() {
        Ok(output) => {
            let full_text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );

            if full_text.trim().is_empty() {
                return Err("Java 执行失败或输出为空".into());
            }

            let display_name = parse_java_display_name(&full_text);

            Ok(JavaInfo {
                path,
                version: display_name,
            })
        }
        Err(e) => Err(format!("无法执行 Java: {}", e)),
    }
}