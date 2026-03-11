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
const DOWNLOAD_BASE_URL: &str   = "https://drive.atmospherium.space/public/updater/.minecraft/versions/";

#[tauri::command]
pub async fn sync_versions(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 1. 获取基础目录
    let exe_path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Failed to get parent dir")?
        .to_path_buf();

    // 确保 base_dir 路径风格统一
    let base_dir = exe_path.join(".minecraft").join("versions");
    let local_manifest_path = base_dir.join("manifest.json");

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // 2. 获取远程清单
    let response = client
        .get(REMOTE_MANIFEST_URL)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("服务器拒绝请求: {}", response.status()));
    }

    let text = response.text().await.map_err(|e| format!("读取内容失败: {}", e))?;
    let remote_manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| format!("清单解析失败: {}", e))?;

    let current_version_dir = remote_manifest.version.clone();

    // 3. 版本对比
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
        println!("版本已是最新，跳过同步。");
        return Ok(());
    }

    // 4. 构建下载队列并预处理路径
    let mut download_queue = Vec::new();
    for (rel_path, info) in &remote_manifest.files {
        if rel_path.contains("options.txt") { continue; }

        let local_path = base_dir.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));

        let file_needs_update = if !local_path.exists() {
            true
        } else {
            // 注意：对于大文件，fs::read 可能占用大量内存，此处为简化逻辑
            let content = fs::read(&local_path).await.map_err(|e| e.to_string())?;
            let mut hasher = Sha1::new();
            hasher.update(&content);
            hex::encode(hasher.finalize()) != info.hash
        };

        if file_needs_update {
            download_queue.push(rel_path.clone());
        }
    }

    // 5. 执行下载任务
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
                        let url_string = format!("{}/{}", base_url, normalized_path);

                        let final_url = url_string.replace(' ', "%20");

                        let dest = b_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));

                        async move {
                            if let Some(parent) = dest.parent() {
                                fs::create_dir_all(parent).await.ok();
                            }

                            let resp = c.get(&final_url).send().await.map_err(|e| e.to_string())?;

                            if !resp.status().is_success() {
                                return Err(format!("下载失败: {}, 状态码: {}\n最终URL: {}", path, resp.status(), final_url));
                            }

                            let data = resp.bytes().await.map_err(|e| e.to_string())?;
                            fs::write(dest, data).await.map_err(|e| e.to_string())?;

                            let current = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                            h.emit("download-progress", ProgressPayload { current, total, file: path }).ok();
                            Ok::<(), String>(())
                        }
                    })
                ).buffer_unordered(3);

        let results: Vec<_> = fetches.collect().await;
        for res in results { res?; }
    }

    // 6. 针对性清理 mods 目录
    let remote_mods_set: std::collections::HashSet<String> = remote_manifest.files.keys()
        .filter(|k| k.contains("/mods/"))
        .map(|k| k.replace('\\', "/"))
        .collect();

    let mods_dir = base_dir.join(&current_version_dir).join("mods");

    if mods_dir.exists() {
        println!("正在清理冗余 Mods...");
        for entry in WalkDir::new(&mods_dir).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let local_file_path = entry.path();

                // 计算相对于 base_dir 的路径，用于比对清单
                if let Ok(rel_path) = local_file_path.strip_prefix(&base_dir) {
                    let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");

                    // 如果本地存在但远程没有，则删除
                    if !remote_mods_set.contains(&rel_path_str) {
                        println!("🗑️ 删除多余 Mod: {}", rel_path_str);
                        let _ = std::fs::remove_file(local_file_path);
                    }
                }
            }
        }
    }

    // 7. 保存清单
    save_local_manifest(&local_manifest_path, &remote_manifest).await?;
    println!("同步完成！");
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