use crate::models::*;
use std::path::{PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::fs;
use sha1::{Sha1, Digest};
use futures::StreamExt;
use tauri::{Emitter};
use walkdir::WalkDir;

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
    let local_manifest_path = exe_path.join("launcher").join("manifest.json");

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
    let client = reqwest::Client::new();

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
    let remote_manifest: Manifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;

    final_version_dir = remote_manifest.version.clone();

    let _ = VERSION.set(final_version_dir.clone());

    // 4. 版本对比逻辑
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

    // 5. 构建下载队列
    let mut download_queue = Vec::new();
    for (rel_path, info) in &remote_manifest.files {
        if rel_path.contains("options.txt") { continue; }

        let local_path = base_dir.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));

        let file_needs_update = if !local_path.exists() {
            true
        } else {
            let content = fs::read(&local_path).await.map_err(|e| e.to_string())?;
            let mut hasher = Sha1::new();
            hasher.update(&content);
            hex::encode(hasher.finalize()) != info.hash
        };

        if file_needs_update {
            download_queue.push(rel_path.clone());
        }
    }

    // 6. 执行并发下载任务
    if !download_queue.is_empty() {
        let total = download_queue.len();
        let counter = Arc::new(AtomicUsize::new(0));

        let fetches = futures::stream::iter(
            download_queue.into_iter().map(|path| {
                let c = client.clone();
                let h = app_handle.clone();
                let cnt = counter.clone();
                let b_dir = base_dir.clone();

                let normalized_path = path.replace('\\', "/").trim_start_matches('/').to_string();
                let base_url = DOWNLOAD_BASE_URL.trim_end_matches('/').to_string();
                let final_url = format!("{}/{}", base_url, normalized_path).replace(' ', "%20");

                let dest = b_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));

                async move {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent).await.ok();
                    }

                    let resp = c.get(&final_url).send().await.map_err(|e| e.to_string())?;
                    if !resp.status().is_success() {
                        return Err(format!("下载失败: {}, 状态: {}", path, resp.status()));
                    }

                    let data = resp.bytes().await.map_err(|e| e.to_string())?;
                    fs::write(dest, data).await.map_err(|e| e.to_string())?;

                    let current = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                    h.emit("download-progress", ProgressPayload { current, total, file: path }).ok();
                    Ok::<(), String>(())
                }
            })
        ).buffer_unordered(5);

        let results: Vec<_> = fetches.collect().await;
        for res in results { res?; }
    }

    // 7. 清理 mods 目录
    let remote_mods_set: std::collections::HashSet<String> = remote_manifest.files.keys()
        .filter(|k| k.contains("/mods/"))
        .map(|k| k.replace('\\', "/"))
        .collect();

    let mods_dir = base_dir.join("versions").join(&final_version_dir).join("mods");

    if mods_dir.exists() {
        for entry in WalkDir::new(&mods_dir).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let local_file_path = entry.path();
                if let Ok(rel_path) = local_file_path.strip_prefix(&base_dir) {
                    let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");
                    if !remote_mods_set.contains(&rel_path_str) {
                        let _ = std::fs::remove_file(local_file_path);
                    }
                }
            }
        }
    }

    // 8. 保存新清单
    save_local_manifest(&local_manifest_path, &remote_manifest).await?;
    println!("同步完成，当前版本: {}", final_version_dir);
    Ok(())
}

async fn save_local_manifest(path: &PathBuf, manifest: &Manifest) -> Result<(), String> {
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.ok();
    }
    fs::write(path, json).await.map_err(|e| e.to_string())?;
    Ok(())
}