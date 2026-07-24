use crate::models::*;
use futures::{stream, StreamExt};
use regex::Regex;
use std::collections::HashMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::System;
use walkdir::WalkDir;
#[cfg(windows)]
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
};
#[cfg(windows)]
use winreg::RegKey;

const MIN_JAVA_MAJOR: u32 = 21;
const JAVA_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const JAVA_SCAN_CONCURRENCY: usize = 8;

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

struct DetectedJava {
    info: JavaInfo,
    major: u32,
    architecture_bits: Option<u32>,
}

fn java_executable_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn is_java_executable(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if cfg!(windows) {
        name.eq_ignore_ascii_case("java.exe") || name.eq_ignore_ascii_case("javaw.exe")
    } else {
        name == "java"
    }
}

fn clean_canonical_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let display = path.to_string_lossy();
        if let Some(stripped) = display.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

#[cfg(windows)]
fn prefer_console_java(candidate: PathBuf) -> PathBuf {
    if candidate
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("javaw.exe"))
    {
        let console_java = candidate.with_file_name("java.exe");
        if console_java.is_file() {
            return console_java;
        }
    }
    candidate
}

#[cfg(not(windows))]
fn prefer_console_java(candidate: PathBuf) -> PathBuf {
    candidate
}

fn resolve_java_path(input: &Path) -> Result<PathBuf, String> {
    let input = PathBuf::from(input.to_string_lossy().trim().trim_matches('"'));
    let candidates = if input.is_dir() {
        vec![
            input.join("bin").join(java_executable_name()),
            input
                .join("Contents")
                .join("Home")
                .join("bin")
                .join(java_executable_name()),
            input.join(java_executable_name()),
            input.join("jre").join("bin").join(java_executable_name()),
        ]
    } else {
        vec![input.clone()]
    };

    for candidate in candidates.into_iter().map(prefer_console_java) {
        if candidate.is_file() && is_java_executable(&candidate) {
            return candidate
                .canonicalize()
                .map(clean_canonical_path)
                .map_err(|error| {
                    format!("无法规范化 Java 路径 {}: {}", candidate.display(), error)
                });
        }
    }

    Err(format!(
        "未在指定路径中找到 {}: {}",
        java_executable_name(),
        input.display()
    ))
}

fn java_path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

pub fn parse_java_major_from_display(display_name: &str) -> Option<u32> {
    let captures = Regex::new(r"(?i)\b(\d+)(?:\.(\d+))?")
        .ok()?
        .captures(display_name)?;
    let first = captures.get(1)?.as_str().parse::<u32>().ok()?;
    if first == 1 {
        captures.get(2)?.as_str().parse::<u32>().ok()
    } else {
        Some(first)
    }
}

fn parse_java_version_output(full_output: &str) -> Option<(String, u32)> {
    let patterns = [
        r#"(?i)(?:java|openjdk)\s+version\s+"([^"]+)""#,
        r#"(?i)version\s+"?([0-9][0-9A-Za-z._+\-]*)"?"#,
        r#"(?i)build\s+([0-9][0-9A-Za-z._+\-]*)"#,
    ];
    for pattern in patterns {
        if let Some(version) = Regex::new(pattern)
            .ok()?
            .captures(full_output)
            .and_then(|captures| captures.get(1))
        {
            let version = version.as_str().trim().to_string();
            let major = parse_java_major_from_display(&version)?;
            return Some((version, major));
        }
    }
    None
}

fn java_vendor(full_output: &str) -> &'static str {
    let upper = full_output.to_ascii_uppercase();
    if upper.contains("TEMURIN") || upper.contains("ADOPTIUM") {
        "Temurin"
    } else if upper.contains("MICROSOFT") {
        "Microsoft"
    } else if upper.contains("CORRETTO") || upper.contains("AMAZON") {
        "Corretto"
    } else if upper.contains("ZULU") || upper.contains("AZUL") {
        "Zulu"
    } else if upper.contains("LIBERICA") || upper.contains("BELLSOFT") {
        "Liberica"
    } else if upper.contains("GRAALVM") {
        "GraalVM"
    } else if upper.contains("SEMERU") || upper.contains("IBM") {
        "Semeru"
    } else if upper.contains("JETBRAINS") || upper.contains("JBR") {
        "JetBrains"
    } else if upper.contains("ORACLE") || upper.contains("JAVA(TM)") {
        "Oracle"
    } else {
        "OpenJDK"
    }
}

fn java_architecture_bits(full_output: &str) -> Option<u32> {
    let lower = full_output.to_ascii_lowercase();
    if lower.contains("sun.arch.data.model = 64")
        || lower.contains("os.arch = amd64")
        || lower.contains("os.arch = x86_64")
        || lower.contains("os.arch = aarch64")
    {
        Some(64)
    } else if lower.contains("sun.arch.data.model = 32")
        || lower.contains("os.arch = x86")
        || lower.contains("os.arch = i386")
    {
        Some(32)
    } else {
        None
    }
}

pub fn parse_java_display_name(full_output: &str) -> Option<(String, u32)> {
    let (version, major) = parse_java_version_output(full_output)?;
    let architecture = java_architecture_bits(full_output)
        .map(|bits| format!(" ({}-bit)", bits))
        .unwrap_or_default();
    Some((
        format!("{} {}{}", java_vendor(full_output), version, architecture),
        major,
    ))
}

async fn inspect_java(path: PathBuf) -> Result<DetectedJava, String> {
    let path = resolve_java_path(&path)?;
    let mut command = tokio::process::Command::new(&path);
    command.args(["-XshowSettings:properties", "-version"]);
    command.kill_on_drop(true);
    #[cfg(windows)]
    command.as_std_mut().creation_flags(0x08000000);

    let output = tokio::time::timeout(JAVA_CHECK_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("Java 检测超时: {}", path.display()))?
        .map_err(|error| format!("无法执行 Java {}: {}", path.display(), error))?;
    let full_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let (version, major) = parse_java_display_name(&full_output)
        .ok_or_else(|| format!("无法解析 Java 版本: {}", path.display()))?;
    let architecture_bits = java_architecture_bits(&full_output);

    Ok(DetectedJava {
        info: JavaInfo {
            path: path.to_string_lossy().to_string(),
            version,
        },
        major,
        architecture_bits,
    })
}

fn add_java_candidate(candidates: &mut HashMap<String, PathBuf>, path: PathBuf) {
    if let Ok(path) = resolve_java_path(&path) {
        candidates.entry(java_path_key(&path)).or_insert(path);
    }
}

fn scan_java_root(candidates: &mut HashMap<String, PathBuf>, root: PathBuf) {
    if !root.is_dir() {
        return;
    }
    for entry in WalkDir::new(root)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && is_java_executable(entry.path()))
    {
        add_java_candidate(candidates, entry.into_path());
    }
}

#[cfg(windows)]
fn collect_registry_java_candidates(candidates: &mut HashMap<String, PathBuf>) {
    const JAVA_KEYS: &[&str] = &[
        r"SOFTWARE\JavaSoft\JDK",
        r"SOFTWARE\JavaSoft\Java Development Kit",
        r"SOFTWARE\JavaSoft\Java Runtime Environment",
        r"SOFTWARE\Eclipse Adoptium\JDK",
        r"SOFTWARE\AdoptOpenJDK\JDK",
    ];
    let hives = [
        RegKey::predef(HKEY_LOCAL_MACHINE),
        RegKey::predef(HKEY_CURRENT_USER),
    ];
    let views = [KEY_READ | KEY_WOW64_64KEY, KEY_READ | KEY_WOW64_32KEY];

    for hive in &hives {
        for key_path in JAVA_KEYS {
            for flags in views {
                let Ok(key) = hive.open_subkey_with_flags(key_path, flags) else {
                    continue;
                };
                for value_name in ["JavaHome", "Path"] {
                    if let Ok(home) = key.get_value::<String, _>(value_name) {
                        add_java_candidate(candidates, PathBuf::from(home));
                    }
                }
                for version_key in key.enum_keys().filter_map(Result::ok) {
                    if let Ok(version) = key.open_subkey_with_flags(version_key, flags) {
                        for value_name in ["JavaHome", "Path"] {
                            if let Ok(home) = version.get_value::<String, _>(value_name) {
                                add_java_candidate(candidates, PathBuf::from(home));
                            }
                        }
                    }
                }
            }
        }
    }

    const APP_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\java.exe";
    for hive in &hives {
        for flags in views {
            if let Ok(key) = hive.open_subkey_with_flags(APP_PATH, flags) {
                if let Ok(path) = key.get_value::<String, _>("") {
                    add_java_candidate(candidates, PathBuf::from(path));
                }
            }
        }
    }
}

fn collect_java_candidates() -> Vec<PathBuf> {
    let mut candidates = HashMap::new();
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            add_java_candidate(&mut candidates, directory.join(java_executable_name()));
        }
    }
    for variable in ["JAVA_HOME", "JDK_HOME", "JRE_HOME"] {
        if let Some(home) = std::env::var_os(variable) {
            add_java_candidate(&mut candidates, PathBuf::from(home));
        }
    }

    let mut roots = Vec::new();
    if let Some(home) = dirs_next::home_dir() {
        roots.extend([
            home.join(".jdks"),
            home.join(".gradle").join("jdks"),
            home.join(".minecraft").join("runtime"),
        ]);
        #[cfg(not(windows))]
        roots.push(home.join(".sdkman").join("candidates").join("java"));
        #[cfg(target_os = "macos")]
        roots.push(home.join("Library/Java/JavaVirtualMachines"));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.push(parent.join(".minecraft").join("runtime"));
            roots.push(parent.join("runtime"));
        }
    }
    if let Ok(current_dir) = std::env::current_dir() {
        roots.extend([
            current_dir.join("java"),
            current_dir.join("runtime"),
            current_dir.join(".minecraft").join("runtime"),
        ]);
    }

    #[cfg(windows)]
    {
        collect_registry_java_candidates(&mut candidates);
        const VENDOR_DIRS: &[&str] = &[
            "Java",
            "Eclipse Adoptium",
            "Microsoft",
            "Amazon Corretto",
            "Zulu",
            "BellSoft",
            "Semeru",
            "JetBrains",
        ];
        for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(variable).map(PathBuf::from) {
                for vendor in VENDOR_DIRS {
                    roots.push(base.join(vendor));
                    if variable == "LOCALAPPDATA" {
                        roots.push(base.join("Programs").join(vendor));
                    }
                }
            }
        }
        if let Some(app_data) = std::env::var_os("APPDATA").map(PathBuf::from) {
            roots.extend([
                app_data.join(".minecraft").join("runtime"),
                app_data.join("PrismLauncher").join("java"),
                app_data
                    .join("com.modrinth.theseus")
                    .join("meta")
                    .join("java_versions"),
                app_data.join("HMCL"),
            ]);
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            roots.extend([
                local_app_data
                    .join("Packages")
                    .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
                    .join("LocalCache")
                    .join("Local")
                    .join("runtime"),
                local_app_data
                    .join("Programs")
                    .join("PrismLauncher")
                    .join("java"),
                local_app_data
                    .join("CurseForge")
                    .join("Minecraft")
                    .join("Install"),
            ]);
        }
    }
    #[cfg(target_os = "linux")]
    roots.extend([PathBuf::from("/usr/lib/jvm"), PathBuf::from("/usr/java")]);
    #[cfg(target_os = "macos")]
    roots.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));

    let mut unique_roots = HashMap::new();
    for root in roots {
        unique_roots.entry(java_path_key(&root)).or_insert(root);
    }
    for root in unique_roots.into_values() {
        scan_java_root(&mut candidates, root);
    }
    candidates.into_values().collect()
}

fn sort_detected_java(detected: &mut [DetectedJava]) {
    let architecture_rank = |java: &DetectedJava| match java.architecture_bits {
        Some(64) => 0,
        None => 1,
        Some(_) => 2,
    };
    detected.sort_by(|a, b| {
        architecture_rank(a)
            .cmp(&architecture_rank(b))
            .then_with(|| a.major.cmp(&b.major))
            .then_with(|| a.info.path.cmp(&b.info.path))
    });
}

#[tauri::command]
pub async fn scan_java_environments() -> Vec<JavaInfo> {
    let candidates = tokio::task::spawn_blocking(collect_java_candidates)
        .await
        .unwrap_or_default();
    let inspections = stream::iter(
        candidates
            .into_iter()
            .map(|path| async move { inspect_java(path).await.ok() }),
    )
    .buffer_unordered(JAVA_SCAN_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut detected = inspections
        .into_iter()
        .flatten()
        .filter(|java| java.major >= MIN_JAVA_MAJOR)
        .collect::<Vec<_>>();
    sort_detected_java(&mut detected);
    detected.into_iter().map(|java| java.info).collect()
}

#[tauri::command]
pub async fn validate_java(path: String) -> Result<JavaInfo, String> {
    let detected = inspect_java(PathBuf::from(path)).await?;
    if detected.major < MIN_JAVA_MAJOR {
        return Err(format!(
            "Java 版本不符合要求：检测到 Java {}，需要 Java {} 或更高版本",
            detected.major, MIN_JAVA_MAJOR
        ));
    }
    Ok(detected.info)
}

#[tauri::command]
pub async fn check_mc_directory() -> Result<bool, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("无法获取程序路径: {}", error))?;
    let current_dir = executable.parent().ok_or("无法获取程序所在目录")?;
    let mc_path = current_dir.join(".minecraft");
    Ok(mc_path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_java_vendor_version_and_architecture() {
        let output = r#"
openjdk version "21.0.6" 2025-01-21 LTS
OpenJDK Runtime Environment Microsoft-11933203 (build 21.0.6+7-LTS)
    os.arch = amd64
"#;
        assert_eq!(
            parse_java_display_name(output),
            Some(("Microsoft 21.0.6 (64-bit)".to_string(), 21))
        );
    }

    #[test]
    fn parses_legacy_java_major_version() {
        let output = r#"java version "1.8.0_401"
Java(TM) SE Runtime Environment (build 1.8.0_401-b10)"#;
        assert_eq!(
            parse_java_display_name(output),
            Some(("Oracle 1.8.0_401".to_string(), 8))
        );
    }

    #[test]
    fn resolves_java_home_directory_to_bin_executable() {
        let root = std::env::temp_dir().join(format!(
            "circube-java-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let executable = bin.join(java_executable_name());
        std::fs::write(&executable, b"").unwrap();

        let resolved = resolve_java_path(&root).unwrap();
        assert_eq!(
            resolved,
            clean_canonical_path(executable.canonicalize().unwrap())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn automatic_selection_prefers_64_bit_java_21() {
        let mut detected = vec![
            DetectedJava {
                info: JavaInfo {
                    path: "java-25".to_string(),
                    version: "OpenJDK 25".to_string(),
                },
                major: 25,
                architecture_bits: Some(64),
            },
            DetectedJava {
                info: JavaInfo {
                    path: "java-21-unknown".to_string(),
                    version: "OpenJDK 21".to_string(),
                },
                major: 21,
                architecture_bits: None,
            },
            DetectedJava {
                info: JavaInfo {
                    path: "java-21-x64".to_string(),
                    version: "Temurin 21".to_string(),
                },
                major: 21,
                architecture_bits: Some(64),
            },
        ];

        sort_detected_java(&mut detected);
        assert_eq!(detected[0].info.path, "java-21-x64");
        assert_eq!(detected[1].info.path, "java-25");
        assert_eq!(detected[2].info.path, "java-21-unknown");
    }
}
