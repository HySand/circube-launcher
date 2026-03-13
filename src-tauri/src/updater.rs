use crate::models::*;
use std::path::{PathBuf, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use sha1::{Sha1, Digest};
use futures::StreamExt;
use tauri::Emitter;
use walkdir::WalkDir;
use reqwest::Client;

const REMOTE_MANIFEST_URL: &str = "https://drive.atmospherium.space/public/updater/manifest.json";
const DOWNLOAD_BASE_URL: &str   = "https://drive.atmospherium.space/public/updater/.minecraft/";

#[tauri::command]
pub async fn sync_versions(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 1. 基础路径准备
    let exe_path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Failed to get parent dir")?
        .to_path_buf();

    let base_dir = exe_path.join(".minecraft");
    let launcher_dir = exe_path.join("launcher");
    let local_manifest_path = launcher_dir.join("manifest.json");

    let mut final_version_dir = String::from("UNKNOWN");

    // 2. 第一阶段：尝试从本地清单读取保底版本
    if local_manifest_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&local_manifest_path) {
            if let Ok(local_manifest) = serde_json::from_str::<Manifest>(&content) {
                final_version_dir = local_manifest.version;
                println!("本地版本: {} ver {}", final_version_dir, local_manifest.manifest_version);
            }
        }
    }

    // 3. 第二阶段：网络请求获取远程清单
    let client = Client::builder()
        .user_agent("Mozilla/5.0 AtmospheriumLauncher/1.0")
        .build()
        .map_err(|e| e.to_string())?;

    let response = match client.get(REMOTE_MANIFEST_URL).send().await {
        Ok(res) => res,
        Err(e) => {
            let _ = VERSION.set(final_version_dir);
            return Err(format!("网络请求失败，已恢复本地版本配置: {}", e));
        }
    };

    if !response.status().is_success() {
        let _ = VERSION.set(final_version_dir);
        return Err(format!("服务器返回错误状态码: {}", response.status()));
    }

    let text = response.text().await.map_err(|e| e.to_string())?;

    let remote_manifest: Manifest = serde_json::from_str(&text).map_err(|e| {
        format!("清单解析失败 (可能是收到了报错网页): {}. 响应内容前缀: {}", e, text.chars().take(100).collect::<String>())
    })?;

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
        println!("版本已是最新 ({} ver {})，跳过下载。", final_version_dir, remote_manifest.manifest_version);
        return Ok(());
    }

    // 5. 第四阶段：构建下载队列
    let mut download_queue = Vec::new();
    for (rel_path, info) in &remote_manifest.files {
        if rel_path.contains("options.txt") { continue; }

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
        let client_arc = Arc::new(client);

        let fetches = futures::stream::iter(
            download_queue.into_iter().map(|(path, target_hash)| {
                let c = client_arc.clone();
                let h = app_handle.clone();
                let cnt = counter.clone();
                let sem = semaphore.clone();
                let b_dir = base_dir.clone();

                async move {
                    let _permit = sem.acquire().await.map_err(|e| e.to_string())?;
                    let mut attempts = 0;
                    let max_retries = 3;

                    let normalized_path = path.replace('\\', "/").trim_start_matches('/').to_string();
                    let url = format!("{}/{}", DOWNLOAD_BASE_URL.trim_end_matches('/'), normalized_path).replace(' ', "%20");
                    let dest = b_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));

                    loop {
                        match download_file_streamed(&c, &url, &dest, &target_hash).await {
                            Ok(_) => {
                                let current = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                                let _ = h.emit("download-progress", ProgressPayload {
                                    current,
                                    total,
                                    file: path.clone()
                                });
                                return Ok::<(), String>(());
                            }
                            Err(e) if attempts < max_retries => {
                                attempts += 1;
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            }
                            Err(e) => return Err(format!("文件 {} 同步失败: {}", path, e)),
                        }
                    }
                }
            })
        ).buffer_unordered(5);

        let results: Vec<_> = fetches.collect().await;
        for res in results { res?; }
    }

    // 7. 清理 mods 目录
    cleanup_unused_mods(&base_dir, &final_version_dir, &remote_manifest).await;

    // 8. 保存新清单
    save_local_manifest(&local_manifest_path, &remote_manifest).await?;
    println!("同步完成，当前版本: {}", final_version_dir);
    Ok(())
}

async fn download_file_streamed(client: &Client, url: &str, dest: &PathBuf, expected_hash: &str) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }

    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
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

    if hex::encode(hasher.finalize()) != expected_hash {
        let _ = fs::remove_file(&tmp_path).await;
        return Err("Hash 校验失败，文件可能损坏".to_string());
    }

    fs::rename(tmp_path, dest).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn calculate_sha1(path: &Path) -> tokio::io::Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn cleanup_unused_mods(base_dir: &Path, version: &str, remote: &Manifest) {
    let remote_mods_set: std::collections::HashSet<String> = remote.files.keys()
        .filter(|k| k.contains("/mods/"))
        .map(|k| k.replace('\\', "/"))
        .collect();

    let mods_dir = base_dir.join("versions").join(version).join("mods");
    if !mods_dir.exists() { return; }

    let entries: Vec<_> = WalkDir::new(&mods_dir).into_iter().filter_map(|e| e.ok()).collect();
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