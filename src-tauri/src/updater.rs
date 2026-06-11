use crate::models::*;
use chrono::{SecondsFormat, Utc};
use futures::StreamExt;
use reqwest::Client;
use sha1::{Digest, Sha1};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use urlencoding::encode;
use walkdir::WalkDir;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const REMOTE_MANIFEST_URL: &str = "https://gitee.com/hysand/CirCube/raw/main/manifest.json";
const BMCLAPI_BASE_URL: &str = "https://bmclapi2.bangbang93.com";

#[derive(Deserialize)]
struct MinecraftVersionMeta {
    arguments: Option<Value>,
    #[serde(rename = "assetIndex")]
    asset_index: Option<AssetIndexInfo>,
    downloads: Option<VersionDownloads>,
    libraries: Vec<MinecraftLibrary>,
}

#[derive(Deserialize)]
struct VersionDownloads {
    client: Option<DownloadInfo>,
}

#[derive(Deserialize)]
struct AssetIndexInfo {
    id: String,
    sha1: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct MinecraftLibrary {
    name: Option<String>,
    url: Option<String>,
    downloads: Option<LibraryDownloadsInfo>,
    rules: Option<Vec<MinecraftRule>>,
    natives: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct LibraryDownloadsInfo {
    artifact: Option<DownloadInfo>,
    classifiers: Option<HashMap<String, DownloadInfo>>,
}

#[derive(Deserialize, Clone)]
struct DownloadInfo {
    path: Option<String>,
    sha1: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct MinecraftRule {
    action: String,
    os: Option<MinecraftOsRule>,
}

#[derive(Deserialize)]
struct MinecraftOsRule {
    name: Option<String>,
}

#[derive(Deserialize)]
struct AssetIndexJson {
    objects: HashMap<String, AssetObject>,
}

#[derive(Deserialize)]
struct AssetObject {
    hash: String,
}

#[derive(Clone)]
struct DownloadJob {
    urls: Vec<String>,
    dest: PathBuf,
    sha1: Option<String>,
}

struct PreparedAssets {
    index_path: PathBuf,
    tmp_index_path: PathBuf,
    backup_index_path: Option<PathBuf>,
    jobs: Vec<DownloadJob>,
}

#[derive(Clone, Copy)]
enum ModLoaderKind {
    Forge,
    NeoForge,
}

struct ModLoaderInstaller {
    kind: ModLoaderKind,
    version: String,
    artifact_path: String,
    official_base_url: &'static str,
}

fn manifest_info(manifest: &Manifest) -> ManifestInfo {
    ManifestInfo {
        version: manifest.version.clone(),
        manifest_version: manifest.manifest_version.clone(),
    }
}

fn local_manifest_path() -> Result<PathBuf, String> {
    let exe_path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Failed to get parent dir")?
        .to_path_buf();
    Ok(exe_path.join("launcher").join("manifest.json"))
}

#[tauri::command]
pub async fn get_manifest_versions(client: tauri::State<'_, Client>) -> Result<ManifestVersions, String> {
    let local = match std::fs::read_to_string(local_manifest_path()?) {
        Ok(content) => serde_json::from_str::<Manifest>(&content).ok().map(|m| manifest_info(&m)),
        Err(_) => None,
    };

    let remote_manifest: Manifest = client
        .get(REMOTE_MANIFEST_URL)
        .send()
        .await
        .map_err(|e| format!("获取远程 manifest 失败: {}", e))?
        .error_for_status()
        .map_err(|e| format!("远程 manifest 响应异常: {}", e))?
        .json()
        .await
        .map_err(|e| format!("远程 manifest 解析失败: {}", e))?;

    let remote = manifest_info(&remote_manifest);
    let needs_update = local
        .as_ref()
        .map_or(true, |local| local.manifest_version != remote.manifest_version);

    Ok(ManifestVersions {
        local,
        remote,
        needs_update,
    })
}

#[tauri::command]
pub async fn sync_versions(
    client: tauri::State<'_, Client>,
    app_handle: tauri::AppHandle,
    config_state: tauri::State<'_, Mutex<Config>>,
) -> Result<(), String> {
    // 1. 基础路径准备
    let exe_path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Failed to get parent dir")?
        .to_path_buf();

    let base_dir = exe_path.join(".minecraft");
    let launcher_dir = exe_path.join("launcher");
    let local_manifest_path = launcher_dir.join("manifest.json");
    let (java_path, download_base_url) = {
        let config = config_state.lock().unwrap();
        let java_path = if config.java_path.trim().is_empty() {
            "java".to_string()
        } else {
            config.java_path.clone()
        };
        (java_path, config.download_source.base_url().to_string())
    };

    let mut final_version_dir = String::from("UNKNOWN");

    // 2. 第一阶段：尝试从本地清单读取保底版本
    if local_manifest_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&local_manifest_path) {
            if let Ok(local_manifest) = serde_json::from_str::<Manifest>(&content) {
                final_version_dir = local_manifest.version;
                println!(
                    "本地版本: {} ver {}",
                    final_version_dir, local_manifest.manifest_version
                );
            }
        }
    }

    // 3. 第二阶段：网络请求获取远程清单
    let remote_manifest: Manifest = client
        .get(REMOTE_MANIFEST_URL)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?
        .error_for_status()
        .map_err(|e| {
            let _ = VERSION.set(final_version_dir.clone());
            format!("服务器响应异常: {}", e)
        })?
        .json::<Manifest>()
        .await
        .map_err(|e| format!("JSON 解析失败 (结构不匹配或非合法 JSON): {}", e))?;

    final_version_dir = remote_manifest.version.clone();
    let _ = VERSION.set(final_version_dir.clone());

    // 4. 第三阶段：版本对比逻辑
    let mut needs_sync = true;
    if local_manifest_path.exists() {
        if let Ok(content) = fs::read_to_string(&local_manifest_path).await {
            if let Ok(local_manifest) = serde_json::from_str::<Manifest>(&content) {
                if local_manifest.manifest_version == remote_manifest.manifest_version {
                    needs_sync = false;
                }
            }
        }
    }

    if !needs_sync {
        println!(
            "版本已是最新 ({} ver {})，跳过下载。",
            final_version_dir, remote_manifest.manifest_version
        );
        ensure_minecraft_resources(&client, &app_handle, &base_dir, &final_version_dir, &java_path).await?;
        return Ok(());
    }

    let _ = app_handle.emit(
        "download-progress",
        ProgressPayload {
            current: 0,
            total: 0,
            file: "/".to_string(),
        },
    );

    // 5. 第四阶段：构建下载队列
    let mut download_queue = Vec::new();
    for (rel_path, info) in &remote_manifest.files {
        let local_path = base_dir.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));

        let file_needs_update = if !local_path.exists() {
            true
        } else {
            match calculate_sha1(&local_path).await {
                Ok(h) => h != info.hash,
                Err(_) => true,
            }
        };

        if file_needs_update {
            download_queue.push((rel_path.clone(), info.hash.clone()));
        }
    }

    // 6. 执行并发下载任务
    if !download_queue.is_empty() {
        let total = download_queue.len();
        let counter = Arc::new(AtomicUsize::new(0));
        let semaphore = Arc::new(tokio::sync::Semaphore::new(5)); // 限制并发数
        let client_arc = client.inner();

        let fetches =
            futures::stream::iter(download_queue.into_iter().map(|(path, target_hash)| {
                let c = client_arc.clone();
                let h = app_handle.clone();
                let cnt = counter.clone();
                let sem = semaphore.clone();
                let b_dir = base_dir.clone();
                let base_url = download_base_url.clone();

                async move {
                    let _permit = sem.acquire().await.map_err(|e| e.to_string())?;
                    let mut attempts = 0;
                    let max_retries = 3;

                    let encoded_path = path
                        .replace('\\', "/")
                        .split('/')
                        .map(|segment| encode(segment).into_owned())
                        .collect::<Vec<String>>()
                        .join("/");

                    let url = format!(
                        "{}/{}",
                        base_url.trim_end_matches('/'),
                        encoded_path
                    );
                    println!("{}", url);

                    let dest = b_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));

                    loop {
                        match download_file_streamed(&c, &url, &dest, Some(&target_hash)).await {
                            Ok(_) => {
                                let current = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                                let _ = h.emit(
                                    "download-progress",
                                    ProgressPayload {
                                        current,
                                        total,
                                        file: path.clone(),
                                    },
                                );
                                return Ok::<(), String>(());
                            }
                            Err(_e) if attempts < max_retries => {
                                attempts += 1;
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            }
                            Err(e) => return Err(format!("文件 {} 同步失败: {}", path, e)),
                        }
                    }
                }
            }))
            .buffer_unordered(5);

        let results: Vec<_> = fetches.collect().await;
        for res in results {
            res?;
        }
    }

    // 7. 补全 Minecraft 官方资源
    ensure_minecraft_resources(&client, &app_handle, &base_dir, &final_version_dir, &java_path).await?;

    // 8. 清理 mods 目录
    cleanup_unused_mods(&base_dir, &final_version_dir, &remote_manifest).await;

    // 9. 保存新清单
    save_local_manifest(&local_manifest_path, &remote_manifest).await?;
    Ok(())
}

async fn ensure_minecraft_resources(
    client: &Client,
    app_handle: &tauri::AppHandle,
    mc_dir: &Path,
    version: &str,
    java_path: &str,
) -> Result<(), String> {
    let emit = |file: &str| {
        let _ = app_handle.emit(
            "download-progress",
            ProgressPayload {
                current: 0,
                total: 0,
                file: file.to_string(),
            },
        );
    };

    emit(&format!("正在解析 Minecraft {} 资源", version));

    let version_dir = mc_dir.join("versions").join(version);
    let version_json_path = version_dir.join(format!("{}.json", version));
    if !version_json_path.exists() {
        let url = format!("{}/version/{}/json", BMCLAPI_BASE_URL, encode(version));
        download_file_streamed(client, &url, &version_json_path, None).await?;
    }

    let raw_json = fs::read_to_string(&version_json_path)
        .await
        .map_err(|e| format!("读取版本 JSON 失败: {}", e))?;
    let version_meta: MinecraftVersionMeta =
        serde_json::from_str(&raw_json).map_err(|e| format!("解析版本 JSON 失败: {}", e))?;
    emit(&format!("{} 版本 JSON 已解析", version));

    if asset_index_matches(mc_dir, &version_meta).await? {
        emit("Minecraft assets index 已匹配，跳过资源补全");
        return Ok(());
    }

    let prepared_assets = prepare_assets(client, mc_dir, &version_meta).await?;

    if let Some(client_download) = version_meta.downloads.as_ref().and_then(|d| d.client.as_ref()) {
        emit("正在检查 Minecraft client jar");
        let client_jar_path = version_dir.join(format!("{}.jar", version));
        let mut urls = vec![format!("{}/version/{}/client", BMCLAPI_BASE_URL, encode(version))];
        if let Some(url) = &client_download.url {
            urls.push(url.clone());
        }
        ensure_download_from_urls(
            client,
            &urls,
            &client_jar_path,
            client_download.sha1.as_deref(),
        )
        .await?;
    }

    if let Some(prepared_assets) = prepared_assets {
        run_download_jobs(client, app_handle, prepared_assets.jobs.clone(), "Minecraft assets").await?;
        ensure_libraries(client, app_handle, mc_dir, &version_dir, version, &version_meta).await?;
        emit("正在检查 Forge/NeoForge 安装器");
        ensure_mod_loader_installer_outputs(client, app_handle, mc_dir, version, &version_meta, java_path).await?;
        finish_assets_index(prepared_assets).await?;
    } else {
        ensure_libraries(client, app_handle, mc_dir, &version_dir, version, &version_meta).await?;
        emit("正在检查 Forge/NeoForge 安装器");
        ensure_mod_loader_installer_outputs(client, app_handle, mc_dir, version, &version_meta, java_path).await?;
    }

    Ok(())
}

async fn ensure_download_from_urls(
    client: &Client,
    urls: &[String],
    dest: &PathBuf,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    if dest.exists() {
        if let Some(expected_hash) = expected_hash {
            if calculate_sha1(dest).await.ok().as_deref() == Some(expected_hash) {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }

    let mut last_error = None;
    for url in urls {
        match download_file_streamed(client, url, dest, expected_hash).await {
            Ok(_) => return Ok(()),
            Err(e) => last_error = Some(e),
        }
    }

    Err(last_error.unwrap_or_else(|| format!("下载失败: {}", dest.display())))
}

async fn run_download_jobs(
    client: &Client,
    app_handle: &tauri::AppHandle,
    jobs: Vec<DownloadJob>,
    label: &str,
) -> Result<(), String> {
    let _ = app_handle.emit(
        "download-progress",
        ProgressPayload {
            current: 0,
            total: 0,
            file: format!("正在检查{}", label),
        },
    );

    let mut pending_jobs = Vec::new();
    for job in jobs {
        if download_job_needed(&job).await {
            pending_jobs.push(job);
        }
    }

    let jobs = pending_jobs;
    if jobs.is_empty() {
        let _ = app_handle.emit(
            "download-progress",
            ProgressPayload {
                current: 0,
                total: 0,
                file: format!("{}已完整", label),
            },
        );
        return Ok(());
    }

    let total = jobs.len();
    let counter = Arc::new(AtomicUsize::new(0));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(8));
    let client = client.clone();

    let fetches = futures::stream::iter(jobs.into_iter().map(|job| {
        let client = client.clone();
        let app_handle = app_handle.clone();
        let counter = counter.clone();
        let semaphore = semaphore.clone();
        let label = label.to_string();

        async move {
            let _permit = semaphore.acquire().await.map_err(|e| e.to_string())?;
            ensure_download_from_urls(&client, &job.urls, &job.dest, job.sha1.as_deref()).await?;
            let current = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = app_handle.emit(
                "download-progress",
                ProgressPayload {
                    current,
                    total,
                    file: label,
                },
            );
            Ok::<(), String>(())
        }
    }))
    .buffer_unordered(8);

    let results: Vec<_> = fetches.collect().await;
    for result in results {
        result?;
    }

    Ok(())
}

async fn download_job_needed(job: &DownloadJob) -> bool {
    if !job.dest.exists() {
        return true;
    }

    match job.sha1.as_deref() {
        Some(expected_hash) => calculate_sha1(&job.dest)
            .await
            .map_or(true, |actual_hash| actual_hash != expected_hash),
        None => false,
    }
}

async fn ensure_mod_loader_installer_outputs(
    client: &Client,
    app_handle: &tauri::AppHandle,
    mc_dir: &Path,
    current_version: &str,
    version_meta: &MinecraftVersionMeta,
    java_path: &str,
) -> Result<(), String> {
    let Some(installer) = detect_mod_loader_installer(version_meta) else {
        return Ok(());
    };

    let loader_name = match installer.kind {
        ModLoaderKind::Forge => "Forge",
        ModLoaderKind::NeoForge => "NeoForge",
    };
    let launcher_dir = mc_dir
        .parent()
        .ok_or("无法获取 launcher 目录")?
        .join("launcher")
        .join("installers");

    let _ = app_handle.emit(
        "download-progress",
        ProgressPayload {
            current: 0,
            total: 0,
            file: format!("正在安装 {} {}", loader_name, installer.version),
        },
    );

    let installer_path = launcher_dir.join(
        Path::new(&installer.artifact_path)
            .file_name()
            .ok_or("安装器文件名无效")?,
    );
    let urls = vec![
        format!("{}/maven/{}", BMCLAPI_BASE_URL, installer.artifact_path),
        format!(
            "{}/{}",
            installer.official_base_url.trim_end_matches('/'),
            installer.artifact_path
        ),
    ];
    ensure_download_from_urls(client, &urls, &installer_path, None).await?;
    run_mod_loader_installer(java_path, &installer_path, mc_dir, current_version, &installer)?;
    cleanup_installer_versions(mc_dir, current_version, &installer)?;

    Ok(())
}

fn detect_mod_loader_installer(version_meta: &MinecraftVersionMeta) -> Option<ModLoaderInstaller> {
    for library in &version_meta.libraries {
        let Some(name) = library.name.as_deref() else {
            continue;
        };

        if let Some(version) = name.strip_prefix("net.neoforged:neoforge:") {
            let version = version
                .split('@')
                .next()
                .unwrap_or(version)
                .split(':')
                .next()
                .unwrap_or(version)
                .to_string();
            return Some(ModLoaderInstaller {
                kind: ModLoaderKind::NeoForge,
                artifact_path: format!(
                    "net/neoforged/neoforge/{0}/neoforge-{0}-installer.jar",
                    version
                ),
                official_base_url: "https://maven.neoforged.net/releases",
                version,
            });
        }

        if let Some(version) = name.strip_prefix("net.minecraftforge:forge:") {
            let version = version
                .split('@')
                .next()
                .unwrap_or(version)
                .split(':')
                .next()
                .unwrap_or(version)
                .to_string();
            return Some(ModLoaderInstaller {
                kind: ModLoaderKind::Forge,
                artifact_path: format!(
                    "net/minecraftforge/forge/{0}/forge-{0}-installer.jar",
                    version
                ),
                official_base_url: "https://maven.minecraftforge.net",
                version,
            });
        }
    }

    detect_mod_loader_from_fml_args(version_meta)
}

fn detect_mod_loader_from_fml_args(version_meta: &MinecraftVersionMeta) -> Option<ModLoaderInstaller> {
    let mut args = Vec::new();
    collect_json_strings(version_meta.arguments.as_ref()?, &mut args);

    let get_arg_value = |key: &str| {
        args.windows(2)
            .find_map(|pair| (pair[0] == key).then(|| pair[1].clone()))
    };

    let mc_version = get_arg_value("--fml.mcVersion");
    let forge_version = get_arg_value("--fml.forgeVersion");
    let forge_group = get_arg_value("--fml.forgeGroup");

    if forge_group.as_deref() == Some("net.minecraftforge") {
        if let (Some(mc_version), Some(forge_version)) = (mc_version, forge_version) {
            let version = format!("{}-{}", mc_version, forge_version);
            return Some(ModLoaderInstaller {
                kind: ModLoaderKind::Forge,
                artifact_path: format!(
                    "net/minecraftforge/forge/{0}/forge-{0}-installer.jar",
                    version
                ),
                official_base_url: "https://maven.minecraftforge.net",
                version,
            });
        }
    }

    None
}

fn collect_json_strings(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(value) => output.push(value.clone()),
        Value::Array(values) => {
            for value in values {
                collect_json_strings(value, output);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_json_strings(value, output);
            }
        }
        _ => {}
    }
}

fn run_mod_loader_installer(
    java_path: &str,
    installer_path: &Path,
    mc_dir: &Path,
    current_version: &str,
    installer: &ModLoaderInstaller,
) -> Result<(), String> {
    ensure_launcher_profiles(mc_dir, current_version, installer)?;

    let attempts: Vec<(&str, bool)> = match installer.kind {
        ModLoaderKind::Forge => vec![
            ("--installClient", true),
            ("--installClient", false),
        ],
        ModLoaderKind::NeoForge => vec![
            ("--installClient", true),
            ("--installClient", false),
            ("--install-client", true),
            ("--install-client", false),
        ],
    };
    let mut outputs = Vec::new();

    for (install_arg, include_dir) in attempts {
        let mut cmd = Command::new(java_path);
        cmd.arg("-jar")
            .arg(installer_path)
            .arg(install_arg);
        if include_dir {
            cmd.arg(mc_dir);
        }
        cmd.current_dir(mc_dir);

        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }

        match cmd.output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                outputs.push(format!(
                    "参数: {}{} | stdout: {} | stderr: {}",
                    install_arg,
                    if include_dir { " <mc_dir>" } else { "" },
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ));
                continue;
            }
            Err(e) => return Err(format!("无法执行安装器: {}", e)),
        }
    }

    Err(format!("安装器执行失败: {} ({})", installer_path.display(), outputs.join(" || ")))
}

fn cleanup_installer_versions(
    mc_dir: &Path,
    current_version: &str,
    installer: &ModLoaderInstaller,
) -> Result<(), String> {
    let mut generated_versions = Vec::new();

    match installer.kind {
        ModLoaderKind::Forge => {
            if let Some((mc_version, forge_version)) = installer.version.split_once('-') {
                generated_versions.push(mc_version.to_string());
                generated_versions.push(format!("{}-forge-{}", mc_version, forge_version));
            }
        }
        ModLoaderKind::NeoForge => {
            generated_versions.push(format!("neoforge-{}", installer.version));
        }
    }

    let versions_dir = mc_dir.join("versions");
    for generated_version in generated_versions {
        if generated_version == current_version {
            continue;
        }

        let path = versions_dir.join(&generated_version);
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .map_err(|e| format!("清理 installer 生成版本失败 {}: {}", path.display(), e))?;
        }
    }

    Ok(())
}

fn ensure_launcher_profiles(
    mc_dir: &Path,
    current_version: &str,
    installer: &ModLoaderInstaller,
) -> Result<(), String> {
    let profile_path = mc_dir.join("launcher_profiles.json");
    let mut profile = if profile_path.exists() {
        std::fs::read_to_string(&profile_path)
            .ok()
            .and_then(|content| serde_json::from_str::<Value>(&content).ok())
            .filter(|value| value.is_object())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let profile_id = launcher_profile_id(current_version);
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

    let root = profile
        .as_object_mut()
        .ok_or("launcher_profiles.json 根节点不是对象")?;
    root.insert("version".to_string(), serde_json::json!(3));
    root.insert("selectedProfile".to_string(), serde_json::json!(profile_id.clone()));
    root.entry("clientToken".to_string())
        .or_insert_with(|| serde_json::json!("circube-launcher"));
    root.entry("authenticationDatabase".to_string())
        .or_insert_with(|| serde_json::json!({}));
    root.entry("userProperties".to_string())
        .or_insert_with(|| serde_json::json!([]));
    root.insert(
        "launcherVersion".to_string(),
        serde_json::json!({
            "name": "CirCube Launcher",
            "format": 21,
            "profilesFormat": 2
        }),
    );

    if !root.get("settings").is_some_and(Value::is_object) {
        root.insert("settings".to_string(), serde_json::json!({}));
    }
    if let Some(settings) = root.get_mut("settings").and_then(Value::as_object_mut) {
        settings.entry("enableAdvanced".to_string()).or_insert(serde_json::json!(true));
        settings.entry("enableAnalytics".to_string()).or_insert(serde_json::json!(false));
        settings.entry("enableHistorical".to_string()).or_insert(serde_json::json!(false));
        settings.entry("enableReleases".to_string()).or_insert(serde_json::json!(true));
        settings.entry("enableSnapshots".to_string()).or_insert(serde_json::json!(false));
        settings.entry("keepLauncherOpen".to_string()).or_insert(serde_json::json!(false));
        settings.entry("profileSorting".to_string()).or_insert(serde_json::json!("ByLastPlayed"));
        settings.entry("showGameLog".to_string()).or_insert(serde_json::json!(false));
        settings.entry("showMenu".to_string()).or_insert(serde_json::json!(false));
    }

    if !root.get("profiles").is_some_and(Value::is_object) {
        root.insert("profiles".to_string(), serde_json::json!({}));
    }
    let profiles = root
        .get_mut("profiles")
        .and_then(Value::as_object_mut)
        .ok_or("launcher_profiles.json profiles 节点不是对象")?;
    profiles.insert(
        profile_id.clone(),
        serde_json::json!({
            "name": format!("CirCube Installer {}", current_version),
            "type": "custom",
            "created": timestamp,
            "lastUsed": timestamp,
            "lastVersionId": current_version,
            "gameDir": mc_dir.to_string_lossy(),
            "icon": match installer.kind {
                ModLoaderKind::Forge => "Furnace",
                ModLoaderKind::NeoForge => "Crafting_Table",
            },
            "javaArgs": "-Xmx2G",
            "logConfig": null
        }),
    );

    std::fs::write(
        profile_path,
        serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn launcher_profile_id(version: &str) -> String {
    let safe_version: String = version
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("circube-installer-{}", safe_version)
}

async fn ensure_libraries(
    client: &Client,
    app_handle: &tauri::AppHandle,
    mc_dir: &Path,
    version_dir: &Path,
    version: &str,
    version_meta: &MinecraftVersionMeta,
) -> Result<(), String> {
    let libs_dir = mc_dir.join("libraries");
    let natives_dir = version_dir.join(format!("{}-natives", version));
    let mut jobs = Vec::new();
    let mut native_extracts = Vec::new();

    for library in &version_meta.libraries {
        if !is_library_allowed(library.rules.as_deref()) {
            continue;
        }

        let artifact = library
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.artifact.as_ref());

        if let Some(path) = artifact
            .and_then(|artifact| artifact.path.clone())
            .or_else(|| library.name.as_deref().and_then(library_path_from_name))
        {
            let mut urls = vec![format!("{}/maven/{}", BMCLAPI_BASE_URL, path.replace('\\', "/"))];
            if let Some(url) = artifact.and_then(|artifact| artifact.url.as_ref()) {
                urls.push(url.clone());
            }
            if let Some(base_url) = &library.url {
                append_base_url(&mut urls, base_url, &path);
            }
            let dest = libs_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            jobs.push(DownloadJob {
                urls,
                dest,
                sha1: artifact.and_then(|artifact| artifact.sha1.clone()),
            });
        }

        let Some(native_key) = native_classifier_key(library.natives.as_ref()) else {
            continue;
        };

        let Some(native_artifact) = library
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.classifiers.as_ref())
            .and_then(|classifiers| classifiers.get(&native_key))
        else {
            continue;
        };

        if let Some(path) = &native_artifact.path {
            let mut urls = vec![format!("{}/maven/{}", BMCLAPI_BASE_URL, path.replace('\\', "/"))];
            if let Some(url) = &native_artifact.url {
                urls.push(url.clone());
            }
            if let Some(base_url) = &library.url {
                append_base_url(&mut urls, base_url, path);
            }
            let dest = libs_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            jobs.push(DownloadJob {
                urls,
                dest: dest.clone(),
                sha1: native_artifact.sha1.clone(),
            });
            native_extracts.push(dest);
        }
    }

    run_download_jobs(client, app_handle, jobs, "Minecraft libraries").await?;

    for jar_path in native_extracts {
        extract_native_jar(&jar_path, &natives_dir)?;
    }

    Ok(())
}

async fn asset_index_matches(mc_dir: &Path, version_meta: &MinecraftVersionMeta) -> Result<bool, String> {
    let Some(asset_index) = &version_meta.asset_index else {
        return Ok(true);
    };

    let index_path = mc_dir
        .join("assets")
        .join("indexes")
        .join(format!("{}.json", asset_index.id));
    if !index_path.exists() {
        return Ok(false);
    }

    match asset_index.sha1.as_deref() {
        Some(expected_hash) => calculate_sha1(&index_path)
            .await
            .map(|actual_hash| actual_hash == expected_hash)
            .map_err(|e| e.to_string()),
        None => Ok(true),
    }
}

async fn prepare_assets(
    client: &Client,
    mc_dir: &Path,
    version_meta: &MinecraftVersionMeta,
) -> Result<Option<PreparedAssets>, String> {
    let Some(asset_index) = &version_meta.asset_index else {
        return Ok(None);
    };

    let indexes_dir = mc_dir.join("assets").join("indexes");
    let objects_dir = mc_dir.join("assets").join("objects");
    let index_path = indexes_dir.join(format!("{}.json", asset_index.id));
    let tmp_index_path = index_path.with_extension("json.download");
    let backup_index_path = if index_path.exists() {
        let backup_index_path = index_path.with_extension("json.bak");
        let _ = fs::remove_file(&backup_index_path).await;
        fs::rename(&index_path, &backup_index_path)
            .await
            .map_err(|e| format!("重命名旧 assets index 失败: {}", e))?;
        Some(backup_index_path)
    } else {
        None
    };

    let index_url = asset_index
        .url
        .as_ref()
        .ok_or_else(|| format!("版本 JSON 缺少 assetIndex.url: {}", asset_index.id))?;
    ensure_download_from_urls(
        client,
        &[index_url.clone()],
        &tmp_index_path,
        asset_index.sha1.as_deref(),
    )
    .await?;

    let raw_index = fs::read_to_string(&tmp_index_path)
        .await
        .map_err(|e| format!("读取 assets index 失败: {}", e))?;
    let asset_index_json: AssetIndexJson =
        serde_json::from_str(&raw_index).map_err(|e| format!("解析 assets index 失败: {}", e))?;

    let mut jobs = Vec::new();
    for (name, object) in asset_index_json.objects {
        let prefix = object
            .hash
            .get(0..2)
            .ok_or_else(|| format!("资源 {} 的 hash 无效", name))?;
        let rel_path = format!("{}/{}", prefix, object.hash);
        let urls = vec![
            format!("{}/assets/{}", BMCLAPI_BASE_URL, rel_path),
            format!("https://resources.download.minecraft.net/{}", rel_path),
        ];
        let dest = objects_dir.join(prefix).join(&object.hash);
        jobs.push(DownloadJob {
            urls,
            dest,
            sha1: Some(object.hash),
        });
    }

    Ok(Some(PreparedAssets {
        index_path,
        tmp_index_path,
        backup_index_path,
        jobs,
    }))
}

async fn finish_assets_index(prepared_assets: PreparedAssets) -> Result<(), String> {
    if prepared_assets.index_path.exists() {
        let _ = fs::remove_file(&prepared_assets.index_path).await;
    }
    fs::rename(&prepared_assets.tmp_index_path, &prepared_assets.index_path)
        .await
        .map_err(|e| format!("保存 assets index 失败: {}", e))?;
    if let Some(backup_index_path) = prepared_assets.backup_index_path {
        let _ = fs::remove_file(backup_index_path).await;
    }

    Ok(())
}

fn is_library_allowed(rules: Option<&[MinecraftRule]>) -> bool {
    let Some(rules) = rules else {
        return true;
    };

    let current_os = current_minecraft_os();
    let mut allowed = true;
    for rule in rules {
        let matched = rule
            .os
            .as_ref()
            .and_then(|os| os.name.as_deref())
            .map_or(true, |name| name == current_os);

        if rule.action == "allow" && !matched {
            allowed = false;
        }
        if rule.action == "disallow" && matched {
            allowed = false;
        }
    }
    allowed
}

fn native_classifier_key(natives: Option<&HashMap<String, String>>) -> Option<String> {
    let natives = natives?;
    let key = natives.get(current_minecraft_os())?;
    let arch = if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    };
    Some(key.replace("${arch}", arch))
}

fn current_minecraft_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

fn library_path_from_name(name: &str) -> Option<String> {
    let (coords, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<&str> = coords.split(':').collect();
    if parts.len() < 3 {
        return None;
    }

    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).copied();
    let file_name = match classifier {
        Some(classifier) => format!("{}-{}-{}.{}", artifact, version, classifier, ext),
        None => format!("{}-{}.{}", artifact, version, ext),
    };

    Some(format!("{}/{}/{}/{}", group, artifact, version, file_name))
}

fn append_base_url(urls: &mut Vec<String>, base_url: &str, path: &str) {
    urls.push(format!("{}/{}", base_url.trim_end_matches('/'), path.replace('\\', "/")));
}

fn extract_native_jar(jar_path: &Path, natives_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(natives_dir).map_err(|e| e.to_string())?;

    #[cfg(windows)]
    {
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "Expand-Archive",
            "-LiteralPath",
        ])
        .arg(jar_path)
        .arg("-DestinationPath")
        .arg(natives_dir)
        .arg("-Force")
        .creation_flags(0x08000000);

        let status = cmd.status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("解压 native jar 失败: {}", jar_path.display()));
        }
    }

    #[cfg(not(windows))]
    {
        let status = Command::new("jar")
            .arg("xf")
            .arg(jar_path)
            .current_dir(natives_dir)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("解压 native jar 失败: {}", jar_path.display()));
        }
    }

    let _ = std::fs::remove_dir_all(natives_dir.join("META-INF"));
    Ok(())
}

async fn download_file_streamed(
    client: &Client,
    url: &str,
    dest: &PathBuf,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }

    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{} -> HTTP {}", url, resp.status()));
    }

    let tmp_path = dest.with_extension("tmp");
    let mut file = File::create(&tmp_path).await.map_err(|e| e.to_string())?;
    let mut hasher = Sha1::new();
    let mut stream = resp.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|e| e.to_string())?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }

    file.flush().await.map_err(|e| e.to_string())?;

    let actual_hash = hex::encode(hasher.finalize());
    if expected_hash.is_some_and(|hash| actual_hash != hash) {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(format!("{} -> Hash 校验失败，文件可能损坏", url));
    }

    fs::rename(tmp_path, dest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn calculate_sha1(path: &Path) -> tokio::io::Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn cleanup_unused_mods(base_dir: &Path, version: &str, remote: &Manifest) {
    let remote_mods_set: std::collections::HashSet<String> = remote
        .files
        .keys()
        .filter(|k| k.contains("/mods/"))
        .map(|k| k.replace('\\', "/"))
        .collect();

    let mods_dir = base_dir.join("versions").join(version).join("mods");
    if !mods_dir.exists() {
        return;
    }

    let entries: Vec<_> = WalkDir::new(&mods_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .collect();
    for entry in entries {
        if entry.file_type().is_file() {
            if let Ok(rel_path) = entry.path().strip_prefix(base_dir) {
                let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");
                if !remote_mods_set.contains(&rel_path_str) {
                    let _ = fs::remove_file(entry.path()).await;
                }
            }
        }
    }
}

async fn save_local_manifest(path: &PathBuf, manifest: &Manifest) -> Result<(), String> {
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.ok();
    }
    fs::write(path, json).await.map_err(|e| e.to_string())?;
    Ok(())
}
