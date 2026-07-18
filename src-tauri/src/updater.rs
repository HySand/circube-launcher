use crate::download_engine::{self, DownloadJob};
use crate::download_sources::{asset_index_url, asset_object_url, library_urls, version_client_url};
use crate::models::*;
use chrono::{SecondsFormat, Utc};
use flate2::read::DeflateDecoder;
use futures::{FutureExt, StreamExt};
use reqwest::header::{CACHE_CONTROL, PRAGMA};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use urlencoding::encode;
use walkdir::WalkDir;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const REMOTE_MANIFEST_URL: &str = "https://gitee.com/hysand/CirCube/raw/main/manifest.json";
const PACK_FILE_CONCURRENCY: usize = 24;
const DOWNLOAD_REQUEST_CONCURRENCY: usize = 64;
const PACK_LOW_SPEED_WINDOW_SECS: u64 = 10;
const PACK_LOW_SPEED_THRESHOLD_BYTES: u64 = 500_000;
const DOWNLOAD_SOURCE_SWITCHED_MESSAGE: &str = "下载源已切换，正在重试";

static PACK_SOURCE_GENERATION: AtomicUsize = AtomicUsize::new(0);
static PACK_SOURCE_IS_BITIFUL: AtomicBool = AtomicBool::new(false);
static PACK_SOURCE_SWITCH_NOTIFY: tokio::sync::Notify = tokio::sync::Notify::const_new();
static DOWNLOAD_REQUEST_SEMAPHORE: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();

fn download_request_semaphore() -> Arc<tokio::sync::Semaphore> {
    DOWNLOAD_REQUEST_SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(DOWNLOAD_REQUEST_CONCURRENCY)))
        .clone()
}

#[derive(Deserialize)]
struct MinecraftVersionMeta {
    arguments: Option<Value>,
    #[serde(rename = "assetIndex")]
    asset_index: Option<AssetIndexInfo>,
    downloads: Option<VersionDownloads>,
    libraries: Vec<MinecraftLibrary>,
}

#[derive(Deserialize)]
struct InstallerLibraries {
    #[serde(default)]
    libraries: Vec<MinecraftLibrary>,
}

#[derive(Deserialize)]
struct InstallerProfile {
    #[serde(default)]
    data: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    processors: Vec<InstallerProcessor>,
}

#[derive(Deserialize)]
struct InstallerProcessor {
    jar: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    classpath: Vec<String>,
    sides: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct VersionDownloads {
    client: Option<DownloadInfo>,
}

#[derive(Deserialize)]
struct AssetIndexInfo {
    id: String,
    sha1: Option<String>,
    size: Option<u64>,
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
    size: Option<u64>,
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
    #[serde(default)]
    size: u64,
}

struct PreparedAssets {
    index_update: Option<AssetIndexUpdate>,
    jobs: Vec<DownloadJob>,
}

struct AssetIndexUpdate {
    index_path: PathBuf,
    tmp_index_path: PathBuf,
    backup_index_path: Option<PathBuf>,
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
}

fn manifest_info(manifest: &Manifest) -> ManifestInfo {
    ManifestInfo {
        version: manifest.version.clone(),
        manifest_version: manifest.manifest_version.clone(),
    }
}

fn remote_manifest_url(force_refresh: bool) -> String {
    if force_refresh {
        format!("{}?t={}", REMOTE_MANIFEST_URL, Utc::now().timestamp_millis())
    } else {
        REMOTE_MANIFEST_URL.to_string()
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

fn manifest_file_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    normalized
        .strip_prefix(".minecraft/")
        .or_else(|| normalized.strip_prefix("minecraft/"))
        .unwrap_or(&normalized)
        .to_string()
}

fn is_user_managed_pack_path(path: &str) -> bool {
    let normalized = manifest_file_path(path);
    let mut components = normalized.split('/');
    let first = components.next().unwrap_or_default();

    first.eq_ignore_ascii_case("config")
        || (first.eq_ignore_ascii_case("options.txt") && components.next().is_none())
}

fn is_assets_index_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.starts_with("assets/indexes/") && normalized.ends_with(".json")
}

fn emit_progress(app_handle: &tauri::AppHandle, file: impl Into<String>) {
    let _ = app_handle.emit(
        "download-progress",
        ProgressPayload {
            current: 0,
            total: 0,
            file: file.into(),
        },
    );
}

fn pack_download_url(config_state: &Mutex<Config>, path: &str) -> String {
    let base_url = {
        let config = config_state.lock().unwrap();
        config.download_source.base_url().to_string()
    };
    let encoded_path = path
        .replace('\\', "/")
        .split('/')
        .map(|segment| encode(segment).into_owned())
        .collect::<Vec<String>>()
        .join("/");
    format!("{}/{}", base_url.trim_end_matches('/'), encoded_path)
}

fn is_pack_source_switch_error(error: &str) -> bool {
    error.contains(DOWNLOAD_SOURCE_SWITCHED_MESSAGE)
}

async fn monitor_download_speed(
    app_handle: tauri::AppHandle,
    downloaded_bytes: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    allow_source_switch_prompt: bool,
) {
    let started_at = Instant::now();
    let mut samples = VecDeque::from([(started_at, 0usize)]);
    let mut last_sample_at = started_at;
    let mut last_sample_bytes = 0usize;
    let mut low_speed_latched = false;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.tick().await;

    loop {
        interval.tick().await;

        let now = Instant::now();
        let total_bytes = downloaded_bytes.load(Ordering::SeqCst);
        let current_bytes_per_sec = {
            let elapsed = now.duration_since(last_sample_at).as_secs_f64();
            let bytes = total_bytes.saturating_sub(last_sample_bytes);
            last_sample_at = now;
            last_sample_bytes = total_bytes;
            if elapsed > 0.0 {
                (bytes as f64 / elapsed) as u64
            } else {
                0
            }
        };
        samples.push_back((now, total_bytes));

        let cutoff = now
            .checked_sub(Duration::from_secs(PACK_LOW_SPEED_WINDOW_SECS))
            .unwrap_or(started_at);
        while samples.front().is_some_and(|(at, _)| *at < cutoff) {
            samples.pop_front();
        }

        let observed_window = now
            .duration_since(started_at)
            .min(Duration::from_secs(PACK_LOW_SPEED_WINDOW_SECS));
        if !observed_window.is_zero() {
            let oldest_bytes = samples
                .front()
                .map(|(_, bytes)| *bytes)
                .unwrap_or(total_bytes);
            let bytes_in_window = total_bytes.saturating_sub(oldest_bytes);
            let average_bytes_per_sec =
                (bytes_in_window as f64 / observed_window.as_secs_f64()) as u64;
            if allow_source_switch_prompt
                && now.duration_since(started_at)
                >= Duration::from_secs(PACK_LOW_SPEED_WINDOW_SECS)
                && average_bytes_per_sec < PACK_LOW_SPEED_THRESHOLD_BYTES
            {
                low_speed_latched = true;
            }
            let source = if PACK_SOURCE_IS_BITIFUL.load(Ordering::SeqCst) {
                DownloadSource::Bitiful
            } else {
                DownloadSource::R2
            };

            let _ = app_handle.emit(
                "download-speed",
                DownloadSpeedPayload {
                    average_bytes_per_sec,
                    current_bytes_per_sec,
                    low_speed: low_speed_latched,
                    source,
                },
            );
        }

        if stop.load(Ordering::SeqCst) {
            break;
        }
    }
}

#[tauri::command]
pub async fn get_manifest_versions(
    client: tauri::State<'_, Client>,
    force_refresh: Option<bool>,
) -> Result<ManifestVersions, String> {
    let local = match std::fs::read_to_string(local_manifest_path()?) {
        Ok(content) => serde_json::from_str::<Manifest>(&content)
            .ok()
            .map(|m| manifest_info(&m)),
        Err(_) => None,
    };

    let remote_manifest: Manifest = client
        .get(remote_manifest_url(force_refresh.unwrap_or(false)))
        .header(CACHE_CONTROL, "no-cache")
        .header(PRAGMA, "no-cache")
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
        .map_or(true, |local| local.version != remote.version);

    Ok(ManifestVersions {
        local,
        remote,
        needs_update,
    })
}

#[tauri::command]
pub fn switch_to_china_cdn(
    config_state: tauri::State<'_, Mutex<Config>>,
) -> Result<Config, String> {
    let mut config = config_state.lock().unwrap();
    config.download_source = DownloadSource::Bitiful;
    config.save().map_err(|e| e.to_string())?;
    PACK_SOURCE_IS_BITIFUL.store(true, Ordering::SeqCst);
    PACK_SOURCE_GENERATION.fetch_add(1, Ordering::SeqCst);
    PACK_SOURCE_SWITCH_NOTIFY.notify_waiters();
    Ok(config.clone())
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
    let java_path = {
        let config = config_state.lock().unwrap();
        PACK_SOURCE_IS_BITIFUL.store(
            config.download_source == DownloadSource::Bitiful,
            Ordering::SeqCst,
        );
        if config.java_path.trim().is_empty() {
            "java".to_string()
        } else {
            config.java_path.clone()
        }
    };

    let mut minecraft_version_dir = String::from("UNKNOWN");

    // 2. 第一阶段：尝试从本地清单读取保底版本
    if local_manifest_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&local_manifest_path) {
            if let Ok(local_manifest) = serde_json::from_str::<Manifest>(&content) {
                minecraft_version_dir = local_manifest.manifest_version.clone();
                println!(
                    "本地整合包版本: {} ver {}, Minecraft 版本目录: {}",
                    local_manifest.version, local_manifest.manifest_version, minecraft_version_dir
                );
            }
        }
    }

    // 3. 第二阶段：网络请求获取远程清单
    let remote_manifest: Manifest = client
        .get(remote_manifest_url(true))
        .header(CACHE_CONTROL, "no-cache")
        .header(PRAGMA, "no-cache")
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?
        .error_for_status()
        .map_err(|e| {
            let _ = VERSION.set(minecraft_version_dir.clone());
            format!("服务器响应异常: {}", e)
        })?
        .json::<Manifest>()
        .await
        .map_err(|e| format!("JSON 解析失败 (结构不匹配或非合法 JSON): {}", e))?;

    minecraft_version_dir = remote_manifest.manifest_version.clone();
    let _ = VERSION.set(minecraft_version_dir.clone());

    // 4. 第三阶段：版本对比逻辑
    let mut needs_sync = true;
    if local_manifest_path.exists() {
        if let Ok(content) = fs::read_to_string(&local_manifest_path).await {
            if let Ok(local_manifest) = serde_json::from_str::<Manifest>(&content) {
                if local_manifest.version == remote_manifest.version {
                    needs_sync = false;
                }
            }
        }
    }

    if !needs_sync {
        println!(
            "整合包已是最新 ({} ver {})，Minecraft 版本目录: {}，跳过下载。",
            remote_manifest.version, remote_manifest.manifest_version, minecraft_version_dir
        );
        ensure_minecraft_resources(
            &client,
            &app_handle,
            &base_dir,
            &minecraft_version_dir,
            &java_path,
            false,
        )
        .await?;
        return Ok(());
    }

    emit_progress(&app_handle, "/");

    // 5. 第四阶段：构建下载队列
    let mut download_queue = Vec::new();
    for (rel_path, info) in &remote_manifest.files {
        let normalized_path = manifest_file_path(rel_path);
        if is_user_managed_pack_path(&normalized_path) {
            println!(
                "[updater] skip user-managed pack path during update: {}",
                normalized_path
            );
            continue;
        }

        if is_assets_index_path(&normalized_path) {
            println!(
                "[updater] skip manifest assets index marker during pack sync: {}",
                normalized_path
            );
            continue;
        }

        let local_path = base_dir.join(normalized_path.replace('/', std::path::MAIN_SEPARATOR_STR));

        let file_needs_update = if !local_path.exists() {
            true
        } else {
            match download_engine::calculate_sha1(&local_path).await {
                Ok(h) => h != info.hash,
                Err(_) => true,
            }
        };

        if file_needs_update {
            download_queue.push((normalized_path, info.hash.clone(), info.size));
        }
    }

    // 6. 执行并发下载任务
    if !download_queue.is_empty() {
        let total = download_queue.len();
        let counter = Arc::new(AtomicUsize::new(0));
        let request_semaphore = download_request_semaphore();
        let client_arc = client.inner();
        let config_state_ref = config_state.inner();
        let downloaded_bytes = Arc::new(AtomicUsize::new(0));
        let speed_monitor_stop = Arc::new(AtomicBool::new(false));
        let speed_monitor = tokio::spawn(monitor_download_speed(
            app_handle.clone(),
            downloaded_bytes.clone(),
            speed_monitor_stop.clone(),
            true,
        ));

        let fetches =
            futures::stream::iter(download_queue.into_iter().map(|(path, target_hash, size)| {
                let c = client_arc.clone();
                let h = app_handle.clone();
                let cnt = counter.clone();
                let request_semaphore = request_semaphore.clone();
                let b_dir = base_dir.clone();
                let config_state = config_state_ref;
                let downloaded_bytes = downloaded_bytes.clone();

                async move {
                    let mut attempts = 0;
                    let max_retries = 3;

                    let dest = b_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));

                    loop {
                        let source_generation = PACK_SOURCE_GENERATION.load(Ordering::SeqCst);
                        let url = pack_download_url(config_state, &path);
                        println!("{}", url);

                        match download_pack_file_streamed(
                            &c,
                            &url,
                            &dest,
                            Some(&target_hash),
                            size,
                            source_generation,
                            &downloaded_bytes,
                            &request_semaphore,
                        )
                        .await
                        {
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
                            Err(e) if is_pack_source_switch_error(&e) => {
                                continue;
                            }
                            Err(_e) if attempts < max_retries => {
                                attempts += 1;
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            }
                            Err(e) => return Err(format!("文件 {} 同步失败: {}", path, e)),
                        }
                    }
                }
                .boxed()
            }))
            .buffer_unordered(PACK_FILE_CONCURRENCY);

        let results: Vec<_> = fetches.collect().await;
        speed_monitor_stop.store(true, Ordering::SeqCst);
        let _ = speed_monitor.await;
        for res in results {
            res?;
        }
    }

    // 7. 补全 Minecraft 官方资源
    ensure_minecraft_resources(
        &client,
        &app_handle,
        &base_dir,
        &minecraft_version_dir,
        &java_path,
        false,
    )
    .await?;

    // 8. 清理 mods 目录
    cleanup_unused_mods(&base_dir, &minecraft_version_dir, &remote_manifest).await;

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
    force_resource_install: bool,
) -> Result<(), String> {
    emit_progress(app_handle, format!("正在解析 Minecraft {} 资源", version));

    let version_dir = mc_dir.join("versions").join(version);
    let version_json_path = version_dir.join(format!("{}.json", version));
    if !version_json_path.exists() {
        return Err(format!(
            "缺少版本 JSON: {}。请确保 manifest.files 包含 versions/{}/{}.json 并已完成同步。",
            version_json_path.display(),
            version,
            version
        ));
    }

    let raw_json = fs::read_to_string(&version_json_path)
        .await
        .map_err(|e| format!("读取版本 JSON 失败: {}", e))?;
    let version_meta: MinecraftVersionMeta =
        serde_json::from_str(&raw_json).map_err(|e| format!("解析版本 JSON 失败: {}", e))?;
    emit_progress(app_handle, format!("{} 版本 JSON 已解析", version));

    ensure_client_jar(client, app_handle, &version_dir, version, &version_meta).await?;

    let _ = ensure_libraries(
        client,
        app_handle,
        mc_dir,
        &version_dir,
        version,
        &version_meta,
    )
    .await?;

    let prepared_assets =
        prepare_assets(client, mc_dir, &version_meta, force_resource_install).await?;

    if let Some(prepared_assets) = prepared_assets {
        let PreparedAssets {
            index_update,
            jobs,
        } = prepared_assets;
        let _ = download_engine::run_jobs(
            client,
            app_handle,
            jobs,
            "Minecraft assets",
        )
        .await?;

        ensure_mod_loader_install_step(client, app_handle, mc_dir, version, &version_meta, java_path)
            .await?;

        if let Some(index_update) = index_update {
            finish_assets_index(index_update).await?;
        }
    } else {
        ensure_mod_loader_install_step(client, app_handle, mc_dir, version, &version_meta, java_path)
            .await?;
    }

    emit_progress(app_handle, "资源安装完成");
    Ok(())
}

async fn ensure_client_jar(
    client: &Client,
    app_handle: &tauri::AppHandle,
    version_dir: &Path,
    version: &str,
    version_meta: &MinecraftVersionMeta,
) -> Result<(), String> {
    let Some(client_download) = version_meta
        .downloads
        .as_ref()
        .and_then(|d| d.client.as_ref())
    else {
        return Ok(());
    };

    emit_progress(app_handle, "正在检查 Minecraft client jar");
    let client_jar_path = version_dir.join(format!("{}.jar", version));
    let source_url = client_download
        .url
        .as_deref()
        .ok_or_else(|| format!("版本 JSON 缺少 downloads.client.url: {}", version))?;
    let urls = vec![version_client_url(source_url)?];
    download_engine::ensure_download(
        client,
        &urls,
        &client_jar_path,
        client_download.sha1.as_deref(),
        client_download.size,
        None,
    )
    .await
}

async fn ensure_mod_loader_install_step(
    client: &Client,
    app_handle: &tauri::AppHandle,
    mc_dir: &Path,
    version: &str,
    version_meta: &MinecraftVersionMeta,
    java_path: &str,
) -> Result<(), String> {
    emit_progress(app_handle, "正在检查 Forge/NeoForge 安装器");
    ensure_mod_loader_installer_outputs(client, app_handle, mc_dir, version, version_meta, java_path)
        .await
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
        emit_progress(app_handle, "未检测到 Forge/NeoForge 安装器");
        return Ok(());
    };

    let loader_name = match installer.kind {
        ModLoaderKind::Forge => "Forge",
        ModLoaderKind::NeoForge => "NeoForge",
    };
    emit_progress(
        app_handle,
        format!("检测到 {} {}", loader_name, installer.version),
    );

    let launcher_dir = mc_dir
        .parent()
        .ok_or("无法获取 launcher 目录")?
        .join("launcher")
        .join("installers");

    let installer_path = launcher_dir.join(
        Path::new(&installer.artifact_path)
            .file_name()
            .ok_or("安装器文件名无效")?,
    );
    let urls = library_urls(&installer.artifact_path);
    download_engine::ensure_download(client, &urls, &installer_path, None, None, None).await?;
    if matches!(installer.kind, ModLoaderKind::NeoForge) {
        let profile =
            ensure_installer_libraries(client, app_handle, mc_dir, &installer_path, &installer)
                .await?;
        let client_jar = neoforge_client_jar_path(mc_dir, &installer.version);
        if !client_jar.exists() {
            emit_progress(
                app_handle,
                format!("正在生成 NeoForge client jar {}", installer.version),
            );
            run_neoforge_processors(
                java_path,
                mc_dir,
                current_version,
                &installer_path,
                &installer,
                &profile,
            )?;
        }
        if !client_jar.exists() {
            return Err(format!("NeoForge client jar 未生成: {}", client_jar.display()));
        }
        println!(
            "[updater] {} {} libraries complete, skip installer",
            loader_name, installer.version
        );
        return Ok(());
    }

    emit_progress(
        app_handle,
        format!("正在安装 {} {}", loader_name, installer.version),
    );
    run_mod_loader_installer(
        java_path,
        &installer_path,
        mc_dir,
        current_version,
        &installer,
    )?;
    cleanup_installer_versions(mc_dir, current_version, &installer)?;

    Ok(())
}

fn neoforge_client_jar_path(mc_dir: &Path, version: &str) -> PathBuf {
    mc_dir
        .join("libraries")
        .join("net")
        .join("neoforged")
        .join("neoforge")
        .join(version)
        .join(format!("neoforge-{}-client.jar", version))
}

async fn ensure_installer_libraries(
    client: &Client,
    app_handle: &tauri::AppHandle,
    mc_dir: &Path,
    installer_path: &Path,
    installer: &ModLoaderInstaller,
) -> Result<InstallerProfile, String> {
    let libs_dir = mc_dir.join("libraries");
    let mut jobs = Vec::new();
    let mut seen = HashSet::new();
    let mut profile = None;

    for entry_name in ["version.json", "install_profile.json"] {
        println!(
            "[updater] reading NeoForge installer metadata: {} from {}",
            entry_name,
            installer_path.display()
        );
        let raw_json = read_zip_entry_text(installer_path, entry_name)?;
        let metadata: InstallerLibraries = serde_json::from_str(&raw_json)
            .map_err(|e| format!("解析安装器 {} 失败: {}", entry_name, e))?;
        if entry_name == "install_profile.json" {
            profile = Some(
                serde_json::from_str::<InstallerProfile>(&raw_json)
                    .map_err(|e| format!("解析安装器 install_profile.json 失败: {}", e))?,
            );
        }
        println!(
            "[updater] NeoForge installer {} libraries: {}",
            entry_name,
            metadata.libraries.len()
        );

        for library in metadata.libraries {
            add_installer_library_job(&libs_dir, library, &mut jobs, &mut seen);
        }
    }

    let client_data_path = copy_installer_lzma_data(installer_path, &libs_dir, installer, "client")?
        .ok_or_else(|| {
            format!(
                "NeoForge installer 缺少必要文件 data/client.lzma: {}",
                installer_path.display()
            )
        })?;
    if !client_data_path.exists() {
        return Err(format!(
            "NeoForge client binpatch 未成功写入: {}",
            client_data_path.display()
        ));
    }

    println!(
        "[updater] NeoForge installer library jobs collected: {}",
        jobs.len()
    );
    if !jobs.is_empty() {
        let _ = download_engine::run_jobs(client, app_handle, jobs, "NeoForge libraries").await?;
    }

    profile.ok_or_else(|| "NeoForge installer 缺少 install_profile.json".to_string())
}

fn add_installer_library_job(
    libs_dir: &Path,
    library: MinecraftLibrary,
    jobs: &mut Vec<DownloadJob>,
    seen: &mut HashSet<PathBuf>,
) {
    let artifact = library
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.artifact.as_ref());

    let path = if let Some(path) = artifact.and_then(|artifact| artifact.path.clone()) {
        path
    } else if library.url.as_deref().is_some_and(|url| !url.trim().is_empty()) {
        let Some(path) = library.name.as_deref().and_then(library_path_from_name) else {
            return;
        };
        path
    } else {
        if let Some(name) = library.name.as_deref() {
            println!(
                "[updater] NeoForge installer library skipped generated/local artifact: {}",
                name
            );
        }
        return;
    };

    let dest = libs_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !seen.insert(dest.clone()) {
        if let Some(name) = library.name.as_deref() {
            println!("[updater] NeoForge installer library duplicate: {}", name);
        }
        return;
    }

    let urls = library_urls(&path);

    println!(
        "[updater] NeoForge installer library job: {} -> {}",
        library.name.as_deref().unwrap_or(&path),
        dest.display()
    );

    jobs.push(DownloadJob {
        urls,
        dest,
        sha1: artifact.and_then(|artifact| artifact.sha1.clone()),
        size: artifact.and_then(|artifact| artifact.size),
        check_hash: true,
    });
}

fn copy_installer_lzma_data(
    installer_path: &Path,
    libs_dir: &Path,
    installer: &ModLoaderInstaller,
    side: &str,
) -> Result<Option<PathBuf>, String> {
    let entry_name = format!("data/{}.lzma", side);
    let Some(bytes) = read_zip_entry_bytes(installer_path, &entry_name)? else {
        return Ok(None);
    };

    let data_path = installer_data_lzma_path(libs_dir, installer, side);
    if let Some(parent) = data_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 LZMA 目录失败 {}: {}", parent.display(), e))?;
    }
    std::fs::write(&data_path, bytes)
        .map_err(|e| format!("写入 installer LZMA 失败 {}: {}", data_path.display(), e))?;
    println!(
        "[updater] copied installer binpatch {} -> {}",
        entry_name,
        data_path.display()
    );
    Ok(Some(data_path))
}

fn installer_data_lzma_path(libs_dir: &Path, installer: &ModLoaderInstaller, side: &str) -> PathBuf {
    let (group_path, artifact) = match installer.kind {
        ModLoaderKind::Forge => ("net/minecraftforge", "forge"),
        ModLoaderKind::NeoForge => ("net/neoforged", "neoforge"),
    };
    let classifier = format!("{}data", side);
    libs_dir
        .join(group_path.replace('/', std::path::MAIN_SEPARATOR_STR))
        .join(artifact)
        .join(&installer.version)
        .join(format!(
            "{}-{}-{}.lzma",
            artifact, installer.version, classifier
        ))
}

fn run_neoforge_processors(
    java_path: &str,
    mc_dir: &Path,
    current_version: &str,
    installer_path: &Path,
    installer: &ModLoaderInstaller,
    profile: &InstallerProfile,
) -> Result<(), String> {
    let libs_dir = mc_dir.join("libraries");
    let version_jar = mc_dir
        .join("versions")
        .join(current_version)
        .join(format!("{}.jar", current_version));
    let cp_sep = if cfg!(windows) { ";" } else { ":" };

    for processor in &profile.processors {
        if processor
            .sides
            .as_ref()
            .is_some_and(|sides| !sides.iter().any(|side| side == "client"))
        {
            continue;
        }

        let processor_jar = artifact_path_from_name(&libs_dir, &processor.jar)?;
        if !processor_jar.exists() {
            return Err(format!(
                "NeoForge processor jar 缺失: {} ({})",
                processor_jar.display(),
                processor.jar
            ));
        }

        let mut classpath = vec![processor_jar.clone()];
        let mut seen = HashSet::new();
        seen.insert(processor_jar.clone());
        for artifact in &processor.classpath {
            let path = artifact_path_from_name(&libs_dir, artifact)?;
            if seen.insert(path.clone()) {
                classpath.push(path);
            }
        }

        for path in &classpath {
            if !path.exists() {
                return Err(format!("NeoForge processor classpath 缺失: {}", path.display()));
            }
        }

        let main_class = read_jar_main_class(&processor_jar)?;
        let args = processor
            .args
            .iter()
            .map(|arg| {
                resolve_processor_arg(
                    arg,
                    mc_dir,
                    &libs_dir,
                    installer_path,
                    installer,
                    profile,
                    &version_jar,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        println!(
            "[updater] running NeoForge processor {} {}",
            processor.jar,
            args.join(" ")
        );
        let cp = classpath
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(cp_sep);

        let mut cmd = Command::new(java_path);
        cmd.arg("-cp").arg(cp).arg(main_class).args(&args);
        cmd.current_dir(mc_dir);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }

        let output = cmd
            .output()
            .map_err(|e| format!("无法执行 NeoForge processor {}: {}", processor.jar, e))?;
        if !output.status.success() {
            return Err(format!(
                "NeoForge processor 执行失败: {} | stdout: {} | stderr: {}",
                processor.jar,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }

    Ok(())
}

fn resolve_processor_arg(
    arg: &str,
    mc_dir: &Path,
    libs_dir: &Path,
    installer_path: &Path,
    installer: &ModLoaderInstaller,
    profile: &InstallerProfile,
    version_jar: &Path,
) -> Result<String, String> {
    let root = mc_dir.to_string_lossy().to_string();
    let installer_file = installer_path.to_string_lossy().to_string();
    let minecraft_jar = version_jar.to_string_lossy().to_string();
    let mut resolved = arg
        .replace("{ROOT}", &root)
        .replace("{INSTALLER}", &installer_file)
        .replace("{MINECRAFT_JAR}", &minecraft_jar)
        .replace("{SIDE}", "client");

    for key in profile.data.keys() {
        let token = format!("{{{}}}", key);
        if resolved.contains(&token) {
            let value = profile
                .data
                .get(key)
                .and_then(|side_values| side_values.get("client"))
                .ok_or_else(|| format!("NeoForge install_profile 缺少 client data: {}", key))?;
            let value = resolve_install_profile_value(value, libs_dir, installer)?;
            resolved = resolved.replace(&token, &value);
        }
    }

    if resolved.starts_with('[') && resolved.ends_with(']') {
        resolved = resolve_install_profile_value(&resolved, libs_dir, installer)?;
    }

    Ok(resolved)
}

fn resolve_install_profile_value(
    value: &str,
    libs_dir: &Path,
    installer: &ModLoaderInstaller,
) -> Result<String, String> {
    if let Some(artifact) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return Ok(artifact_path_from_name(libs_dir, artifact)?
            .to_string_lossy()
            .to_string());
    }

    if value == "/data/client.lzma" {
        return Ok(installer_data_lzma_path(libs_dir, installer, "client")
            .to_string_lossy()
            .to_string());
    }
    if value == "/data/server.lzma" {
        return Ok(installer_data_lzma_path(libs_dir, installer, "server")
            .to_string_lossy()
            .to_string());
    }

    if let Some(quoted) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return Ok(quoted.to_string());
    }

    Ok(value.to_string())
}

fn artifact_path_from_name(libs_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = library_path_from_name(name)
        .ok_or_else(|| format!("无法解析 Maven 坐标: {}", name))?;
    Ok(libs_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

fn read_jar_main_class(jar_path: &Path) -> Result<String, String> {
    let manifest = read_zip_entry_text(jar_path, "META-INF/MANIFEST.MF")?;
    let mut current_key = String::new();
    let mut current_value = String::new();
    for line in manifest.lines() {
        if let Some(rest) = line.strip_prefix(' ') {
            current_value.push_str(rest);
            continue;
        }

        if current_key.eq_ignore_ascii_case("Main-Class") {
            return Ok(current_value.trim().to_string());
        }

        if let Some((key, value)) = line.split_once(':') {
            current_key = key.trim().to_string();
            current_value = value.trim().to_string();
        }
    }

    if current_key.eq_ignore_ascii_case("Main-Class") {
        return Ok(current_value.trim().to_string());
    }

    Err(format!("processor jar 缺少 Main-Class: {}", jar_path.display()))
}

fn read_zip_entry_text(zip_path: &Path, entry_name: &str) -> Result<String, String> {
    let bytes = read_zip_entry_bytes(zip_path, entry_name)?
        .ok_or_else(|| format!("安装器缺少 {}", entry_name))?;
    String::from_utf8(bytes).map_err(|e| format!("安装器 {} 不是 UTF-8 文本: {}", entry_name, e))
}

fn read_zip_entry_bytes(zip_path: &Path, entry_name: &str) -> Result<Option<Vec<u8>>, String> {
    let data = std::fs::read(zip_path)
        .map_err(|e| format!("读取安装器失败 {}: {}", zip_path.display(), e))?;
    let eocd = find_end_of_central_directory(&data).ok_or("安装器 ZIP 结构无效: 缺少 EOCD")?;
    let entry_count = read_u16(&data, eocd + 10)? as usize;
    let mut central_dir_offset = read_u32(&data, eocd + 16)? as usize;

    for _ in 0..entry_count {
        if read_u32(&data, central_dir_offset)? != 0x0201_4b50 {
            return Err("安装器 ZIP 结构无效: central directory 损坏".to_string());
        }

        let compression_method = read_u16(&data, central_dir_offset + 10)?;
        let compressed_size = read_u32(&data, central_dir_offset + 20)? as usize;
        let name_len = read_u16(&data, central_dir_offset + 28)? as usize;
        let extra_len = read_u16(&data, central_dir_offset + 30)? as usize;
        let comment_len = read_u16(&data, central_dir_offset + 32)? as usize;
        let local_header_offset = read_u32(&data, central_dir_offset + 42)? as usize;
        let name_start = central_dir_offset + 46;
        let name_end = name_start + name_len;
        let name = std::str::from_utf8(
            data.get(name_start..name_end)
                .ok_or("安装器 ZIP 结构无效: 文件名越界")?,
        )
        .map_err(|e| format!("安装器 ZIP 文件名不是 UTF-8: {}", e))?;

        if name == entry_name {
            if read_u32(&data, local_header_offset)? != 0x0403_4b50 {
                return Err("安装器 ZIP 结构无效: local header 损坏".to_string());
            }

            let local_name_len = read_u16(&data, local_header_offset + 26)? as usize;
            let local_extra_len = read_u16(&data, local_header_offset + 28)? as usize;
            let payload_start = local_header_offset + 30 + local_name_len + local_extra_len;
            let payload_end = payload_start + compressed_size;
            let payload = data
                .get(payload_start..payload_end)
                .ok_or("安装器 ZIP 结构无效: 文件内容越界")?;

            let bytes = match compression_method {
                0 => payload.to_vec(),
                8 => {
                    let mut decoder = DeflateDecoder::new(payload);
                    let mut output = Vec::new();
                    decoder
                        .read_to_end(&mut output)
                        .map_err(|e| format!("解压安装器 {} 失败: {}", entry_name, e))?;
                    output
                }
                other => {
                    return Err(format!(
                        "安装器 {} 使用不支持的 ZIP 压缩方法: {}",
                        entry_name, other
                    ));
                }
            };

            return Ok(Some(bytes));
        }

        central_dir_offset += 46 + name_len + extra_len + comment_len;
    }

    Ok(None)
}

fn find_end_of_central_directory(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .enumerate()
        .rev()
        .find_map(|(index, window)| {
            (window == [0x50, 0x4b, 0x05, 0x06].as_slice()).then_some(index)
        })
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or("安装器 ZIP 结构无效: u16 越界")?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or("安装器 ZIP 结构无效: u32 越界")?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
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
                version,
            });
        }
    }

    detect_mod_loader_from_fml_args(version_meta)
}

fn detect_mod_loader_from_fml_args(
    version_meta: &MinecraftVersionMeta,
) -> Option<ModLoaderInstaller> {
    let mut args = Vec::new();
    collect_json_strings(version_meta.arguments.as_ref()?, &mut args);

    let get_arg_value = |key: &str| {
        args.windows(2)
            .find_map(|pair| (pair[0] == key).then(|| pair[1].clone()))
    };

    let mc_version = get_arg_value("--fml.mcVersion");
    let forge_version = get_arg_value("--fml.forgeVersion");
    let forge_group = get_arg_value("--fml.forgeGroup");
    let neo_forge_version = get_arg_value("--fml.neoForgeVersion");

    if let Some(version) = neo_forge_version {
        return Some(ModLoaderInstaller {
            kind: ModLoaderKind::NeoForge,
            artifact_path: format!(
                "net/neoforged/neoforge/{0}/neoforge-{0}-installer.jar",
                version
            ),
            version,
        });
    }

    if forge_group.as_deref() == Some("net.minecraftforge") {
        if let (Some(mc_version), Some(forge_version)) = (mc_version, forge_version) {
            let version = format!("{}-{}", mc_version, forge_version);
            return Some(ModLoaderInstaller {
                kind: ModLoaderKind::Forge,
                artifact_path: format!(
                    "net/minecraftforge/forge/{0}/forge-{0}-installer.jar",
                    version
                ),
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
        ModLoaderKind::Forge => vec![("--installClient", true), ("--installClient", false)],
        ModLoaderKind::NeoForge => vec![("--install-client", true), ("--installClient", true)],
    };
    let mut outputs = Vec::new();

    for (install_arg, include_dir) in attempts {
        let mut cmd = Command::new(java_path);
        cmd.arg("-jar").arg(installer_path).arg(install_arg);
        if include_dir {
            cmd.arg(mc_dir);
        }
        cmd.current_dir(mc_dir);
        if let Some(parent) = mc_dir.parent() {
            cmd.env("APPDATA", parent);
        }

        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    return Ok(());
                }

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

    Err(format!(
        "安装器执行失败: {} ({})",
        installer_path.display(),
        outputs.join(" || ")
    ))
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
    root.insert(
        "selectedProfile".to_string(),
        serde_json::json!(profile_id.clone()),
    );
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
        settings
            .entry("enableAdvanced".to_string())
            .or_insert(serde_json::json!(true));
        settings
            .entry("enableAnalytics".to_string())
            .or_insert(serde_json::json!(false));
        settings
            .entry("enableHistorical".to_string())
            .or_insert(serde_json::json!(false));
        settings
            .entry("enableReleases".to_string())
            .or_insert(serde_json::json!(true));
        settings
            .entry("enableSnapshots".to_string())
            .or_insert(serde_json::json!(false));
        settings
            .entry("keepLauncherOpen".to_string())
            .or_insert(serde_json::json!(false));
        settings
            .entry("profileSorting".to_string())
            .or_insert(serde_json::json!("ByLastPlayed"));
        settings
            .entry("showGameLog".to_string())
            .or_insert(serde_json::json!(false));
        settings
            .entry("showMenu".to_string())
            .or_insert(serde_json::json!(false));
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
) -> Result<bool, String> {
    let libs_dir = mc_dir.join("libraries");
    let natives_dir = version_dir.join(format!("{}-natives", version));
    let mut jobs = Vec::new();
    let mut seen_jobs = HashSet::new();
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
            let urls = library_urls(&path);
            let dest = libs_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            push_download_job(
                &mut jobs,
                &mut seen_jobs,
                DownloadJob {
                    urls,
                    dest,
                    sha1: artifact.and_then(|artifact| artifact.sha1.clone()),
                    size: artifact.and_then(|artifact| artifact.size),
                    check_hash: true,
                },
            );
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
            let urls = library_urls(path);
            let dest = libs_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            push_download_job(
                &mut jobs,
                &mut seen_jobs,
                DownloadJob {
                    urls,
                    dest: dest.clone(),
                    sha1: native_artifact.sha1.clone(),
                    size: native_artifact.size,
                    check_hash: true,
                },
            );
            native_extracts.push(dest);
        }
    }

    println!("[updater] Minecraft libraries collected: {}", jobs.len());
    let downloaded = download_engine::run_jobs(client, app_handle, jobs, "Minecraft libraries").await?;

    for jar_path in native_extracts {
        extract_native_jar(&jar_path, &natives_dir)?;
    }

    Ok(downloaded)
}

fn push_download_job(
    jobs: &mut Vec<DownloadJob>,
    seen_jobs: &mut HashSet<PathBuf>,
    job: DownloadJob,
) {
    if seen_jobs.insert(job.dest.clone()) {
        jobs.push(job);
    }
}

async fn load_local_asset_index(
    index_path: &Path,
    asset_index: &AssetIndexInfo,
    check_hash: bool,
) -> Option<AssetIndexJson> {
    let metadata = fs::metadata(index_path).await.ok()?;
    if asset_index
        .size
        .is_some_and(|expected_size| expected_size > 0 && metadata.len() != expected_size)
    {
        println!(
            "[updater] assets index size mismatch: {}",
            index_path.display()
        );
        return None;
    }

    if check_hash {
        if let Some(expected_hash) = asset_index.sha1.as_deref() {
            let actual_hash = download_engine::calculate_sha1(index_path).await.ok()?;
            if actual_hash != expected_hash {
                println!(
                    "[updater] assets index SHA-1 mismatch: {}",
                    index_path.display()
                );
                return None;
            }
        }
    }

    let raw_index = fs::read_to_string(index_path).await.ok()?;
    serde_json::from_str(&raw_index).ok()
}

async fn download_asset_index(
    client: &Client,
    index_path: &Path,
    tmp_index_path: &Path,
    asset_index: &AssetIndexInfo,
) -> Result<(AssetIndexJson, AssetIndexUpdate), String> {
    let source_index_url = asset_index
        .url
        .as_ref()
        .ok_or_else(|| format!("版本 JSON 缺少 assetIndex.url: {}", asset_index.id))?;
    let index_urls = vec![asset_index_url(source_index_url)?];
    download_engine::ensure_download(
        client,
        &index_urls,
        tmp_index_path,
        asset_index.sha1.as_deref(),
        asset_index.size.filter(|size| *size > 0),
        None,
    )
    .await?;

    let raw_index = fs::read_to_string(tmp_index_path)
        .await
        .map_err(|error| format!("读取 assets index 失败: {}", error))?;
    let parsed_index = serde_json::from_str(&raw_index)
        .map_err(|error| format!("解析 assets index 失败: {}", error))?;

    let backup_index_path = if index_path.exists() {
        let backup_index_path = index_path.with_extension("json.bak");
        let _ = fs::remove_file(&backup_index_path).await;
        fs::copy(index_path, &backup_index_path)
            .await
            .map_err(|error| format!("备份旧 assets index 失败: {}", error))?;
        Some(backup_index_path)
    } else {
        None
    };

    Ok((
        parsed_index,
        AssetIndexUpdate {
            index_path: index_path.to_path_buf(),
            tmp_index_path: tmp_index_path.to_path_buf(),
            backup_index_path,
        },
    ))
}

async fn prepare_assets(
    client: &Client,
    mc_dir: &Path,
    version_meta: &MinecraftVersionMeta,
    check_assets_hash: bool,
) -> Result<Option<PreparedAssets>, String> {
    let Some(asset_index) = &version_meta.asset_index else {
        return Ok(None);
    };

    let indexes_dir = mc_dir.join("assets").join("indexes");
    let objects_dir = mc_dir.join("assets").join("objects");
    let index_path = indexes_dir.join(format!("{}.json", asset_index.id));
    let tmp_index_path = index_path.with_extension("json.download");

    let (asset_index_json, index_update) =
        if let Some(local_index) =
            load_local_asset_index(&index_path, asset_index, check_assets_hash).await
        {
            println!(
                "[updater] reusing local assets index: {}",
                index_path.display()
            );
            (local_index, None)
        } else {
            let (downloaded_index, update) =
                download_asset_index(client, &index_path, &tmp_index_path, asset_index).await?;
            (downloaded_index, Some(update))
        };

    let mut jobs = Vec::new();
    for (name, object) in asset_index_json.objects {
        let prefix = object
            .hash
            .get(0..2)
            .ok_or_else(|| format!("资源 {} 的 hash 无效", name))?;
        let rel_path = format!("{}/{}", prefix, object.hash);
        let urls = vec![asset_object_url(&rel_path)];
        let dest = objects_dir.join(prefix).join(&object.hash);
        jobs.push(DownloadJob {
            urls,
            dest,
            sha1: Some(object.hash),
            size: (object.size > 0).then_some(object.size),
            check_hash: check_assets_hash,
        });
    }

    Ok(Some(PreparedAssets { index_update, jobs }))
}

async fn finish_assets_index(update: AssetIndexUpdate) -> Result<(), String> {
    let AssetIndexUpdate {
        index_path,
        tmp_index_path,
        backup_index_path,
    } = update;

    if let Some(backup_index_path) = backup_index_path {
        let _ = fs::remove_file(&index_path).await;
        if let Err(error) = fs::rename(&tmp_index_path, &index_path).await {
            let _ = fs::rename(&backup_index_path, &index_path).await;
            return Err(format!("保存 assets index 失败: {}", error));
        }
        let _ = fs::remove_file(backup_index_path).await;
    } else {
        fs::rename(&tmp_index_path, &index_path)
            .await
            .map_err(|error| format!("保存 assets index 失败: {}", error))?;
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

fn pack_source_switch_error() -> String {
    DOWNLOAD_SOURCE_SWITCHED_MESSAGE.to_string()
}

async fn ensure_pack_source_current(
    source_generation: usize,
    cleanup_path: &Path,
) -> Result<(), String> {
    if PACK_SOURCE_GENERATION.load(Ordering::SeqCst) != source_generation {
        let _ = fs::remove_file(cleanup_path).await;
        return Err(pack_source_switch_error());
    }

    Ok(())
}

async fn download_pack_file_single_stream(
    client: &Client,
    url: &str,
    dest: &Path,
    tmp_path: &Path,
    expected_hash: Option<&str>,
    source_generation: usize,
    downloaded_bytes: &Arc<AtomicUsize>,
    request_semaphore: &Arc<tokio::sync::Semaphore>,
) -> Result<(), String> {
    ensure_pack_source_current(source_generation, tmp_path).await?;

    let _permit = request_semaphore
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    let source_switched = PACK_SOURCE_SWITCH_NOTIFY.notified();
    tokio::pin!(source_switched);
    ensure_pack_source_current(source_generation, tmp_path).await?;

    let response = tokio::select! {
        _ = &mut source_switched => {
            let _ = fs::remove_file(tmp_path).await;
            return Err(pack_source_switch_error());
        }
        result = client.get(url).send() => result.map_err(|e| e.to_string())?,
    };
    if !response.status().is_success() {
        return Err(format!("{} -> HTTP {}", url, response.status()));
    }

    let mut file = File::create(tmp_path).await.map_err(|e| e.to_string())?;
    let mut hasher = Sha1::new();
    let mut stream = response.bytes_stream();

    loop {
        ensure_pack_source_current(source_generation, tmp_path).await?;

        let Some(item) = (tokio::select! {
            _ = &mut source_switched => {
                let _ = fs::remove_file(tmp_path).await;
                return Err(pack_source_switch_error());
            }
            item = stream.next() => item,
        }) else {
            break;
        };

        let chunk = item.map_err(|e| e.to_string())?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        downloaded_bytes.fetch_add(chunk.len(), Ordering::SeqCst);
    }

    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    let actual_hash = hex::encode(hasher.finalize());
    if expected_hash.is_some_and(|hash| actual_hash != hash) {
        let _ = fs::remove_file(tmp_path).await;
        return Err(format!("{} -> Hash 校验失败，文件可能损坏", url));
    }

    fs::rename(tmp_path, dest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn download_pack_file_streamed(
    client: &Client,
    url: &str,
    dest: &Path,
    expected_hash: Option<&str>,
    expected_size: u64,
    source_generation: usize,
    downloaded_bytes: &Arc<AtomicUsize>,
    request_semaphore: &Arc<tokio::sync::Semaphore>,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }

    let tmp_path = dest.with_extension("tmp");
    // 整合包文件保持单连接顺序下载；分段仅用于 Minecraft 官方资源和库文件。
    let _ = expected_size;

    download_pack_file_single_stream(
        client,
        url,
        dest,
        &tmp_path,
        expected_hash,
        source_generation,
        downloaded_bytes,
        request_semaphore,
    )
    .await
}

async fn cleanup_unused_mods(base_dir: &Path, version: &str, remote: &Manifest) {
    let remote_mods_set: std::collections::HashSet<String> = remote
        .files
        .keys()
        .filter(|k| k.contains("/mods/"))
        .map(|k| manifest_file_path(k))
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

#[cfg(test)]
mod tests {
    use super::is_user_managed_pack_path;

    #[test]
    fn user_managed_pack_paths_are_skipped() {
        for path in [
            "config",
            "config/sodium-options.json",
            ".minecraft/config/mod.toml",
            "minecraft\\config\\mod.toml",
            "CONFIG/mod.toml",
            "options.txt",
            ".minecraft/options.txt",
            "minecraft\\OPTIONS.TXT",
        ] {
            assert!(
                is_user_managed_pack_path(path),
                "expected user-managed path: {path}"
            );
        }
    }

    #[test]
    fn similarly_named_pack_paths_are_not_skipped() {
        for path in [
            "config.json",
            "configs/mod.toml",
            "mods/config/mod.toml",
            "profiles/options.txt",
            "options.txt.bak",
        ] {
            assert!(
                !is_user_managed_pack_path(path),
                "expected managed pack path: {path}"
            );
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
