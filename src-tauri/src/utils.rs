use crate::models::*;
use regex::Regex;
use std::process::Command;
use sysinfo::System;

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
pub fn scan_java_environments() -> Vec<JavaInfo> {
    println!("开始扫描 Java 环境...");
    let paths: Vec<String> = if cfg!(target_os = "windows") {
        match Command::new("where").arg("java").output() {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stdout);
                s.lines().map(|s| s.to_string()).collect()
            }
            Err(_) => vec![]
        }
    } else {
        match Command::new("which").arg("-a").arg("java").output() {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stdout);
                s.lines().map(|s| s.to_string()).collect()
            }
            Err(_) => vec![]
        }
    };

    let mut result = Vec::new();
    for path in paths {
        let output = Command::new(&path).arg("-version").output();
        if let Ok(out) = output {
            let full_text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let display_name = parse_java_display_name(&full_text);
            result.push(JavaInfo { path, version: display_name });
        }
    }
    result
}