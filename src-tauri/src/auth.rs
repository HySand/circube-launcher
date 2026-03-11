use crate::models::*;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;
use std::sync::{Mutex};
use tauri::Emitter;
use tauri_plugin_opener::OpenerExt;
use tauri::Manager;

pub const CLIENT_ID: &str = match option_env!("MS_CLIENT_ID") {
    Some(id) => id,
    None => "DEFAULT_ID_OR_ERROR",
};

pub async fn process_user_info(
    client: &reqwest::Client,
    access_token: &serde_json::Value,
    profile_data: &serde_json::Value,
) -> Result<UserInfo, String> {
    let profile_id = profile_data["id"].as_str().ok_or("Invalid profile id")?;
    let profile_name = profile_data["name"].as_str().unwrap_or("Player");

    let texture_url = format!("https://littleskin.cn/api/yggdrasil/sessionserver/session/minecraft/profile/{}", profile_id);
    let texture_res = client.get(&texture_url).send().await
        .map_err(|e| e.to_string())?;

    let texture_data: Value = texture_res.json().await.map_err(|e| e.to_string())?;

    let mut skin_url = String::new();
    if let Some(props) = texture_data["properties"].as_array() {
        if let Some(val_str) = props[0]["value"].as_str() {
            if let Ok(decoded) = STANDARD.decode(val_str) {
                if let Ok(decoded_json) = serde_json::from_slice::<Value>(&decoded) {
                    skin_url = decoded_json["textures"]["SKIN"]["url"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
    }

    Ok(UserInfo {
        name: profile_name.to_string(),
        uuid: profile_id.to_string(),
        access_token: access_token.as_str().unwrap_or("").to_string(),
        refresh_token: "".into(),
        skin_url,
        auth_type: "Yggdrasil".into(),
    })
}

#[tauri::command]
pub async fn ms_login(
    app: tauri::AppHandle,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let params = [
        ("client_id", CLIENT_ID),
        ("scope", "XboxLive.signin offline_access"),
    ];

    let res = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: DeviceCodeResponse = res.json().await.map_err(|e| e.to_string())?;
    let user_code = data.user_code.clone();
    let device_code = data.device_code.clone();
    let interval = data.interval;

    // 打开浏览器
    app.opener().open_url(&data.verification_uri, None::<String>).map_err(|e| e.to_string())?;

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let poll_url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
        let poll_client = reqwest::Client::new();

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            let poll_params = [
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", &device_code),
            ];

            let poll_res = poll_client.post(poll_url).form(&poll_params).send().await;

            if let Ok(r) = poll_res {
                let status = r.status();
                let body: serde_json::Value = r.json().await.unwrap_or_default();

                if status.is_success() {
                    let access_token = body["access_token"].as_str().unwrap_or("");
                    let refresh_token = body["refresh_token"].as_str().unwrap_or("");

                    if let Err(e) = authenticate_minecraft(access_token, refresh_token, handle.clone()).await {
                        handle.emit("ms-login-error", format!("验证失败: {}", e)).unwrap();
                    }
                    break;
                } else {
                    let error = body["error"].as_str().unwrap_or("");
                    if error == "authorization_pending" {
                        continue;
                    } else {
                        handle.emit("ms-status", format!("错误: {}", error)).unwrap();
                        break;
                    }
                }
            }
        }
    });

    Ok(user_code)
}

pub async fn authenticate_minecraft(
    ms_access_token: &str,
    ms_refresh_token: &str,
    handle: tauri::AppHandle,
) -> Result<McProfile, String> {
    let client = reqwest::Client::new();

    // --- Step 1: XBL ---
    println!("[Auth] Step 1: Requesting XBL Token...");
    let xbl_res = client.post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms_access_token)
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .send().await.map_err(|e| format!("XBL Request Failed: {}", e))?;

    let xbl_data: serde_json::Value = xbl_res.json().await.map_err(|e| format!("XBL JSON Parse Error: {}", e))?;
    let xbl_token = xbl_data["Token"].as_str().ok_or("XBL Token missing")?;
    let user_hash = xbl_data["DisplayClaims"]["xui"][0]["uhs"].as_str().ok_or("UHS missing")?;

    // --- Step 2: XSTS ---
    println!("[Auth] Step 2: Requesting XSTS Token...");
    let xsts_res = client.post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .json(&serde_json::json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send().await.map_err(|e| format!("XSTS Request Failed: {}", e))?;

    let xsts_data: serde_json::Value = xsts_res.json().await.map_err(|e| format!("XSTS JSON Parse Error: {}", e))?;
    let xsts_token = xsts_data["Token"].as_str().ok_or("XSTS Token missing")?;

    // --- Step 3: Minecraft Login ---
    let identity_token = format!("XBL3.0 x={};{}", user_hash, xsts_token);
    let mc_login_res = client.post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({ "identityToken": identity_token }))
        .send().await.map_err(|e| format!("MC Login Request Failed: {}", e))?;

    let mc_data: serde_json::Value = mc_login_res.json().await.map_err(|e| e.to_string())?;
    let mc_access_token = mc_data["access_token"].as_str().ok_or("MC Access Token missing")?;

    // --- Step 4: Profile ---
    let profile_res = client.get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(mc_access_token)
        .send().await.map_err(|e| format!("Profile Request Failed: {}", e))?;

    let profile = profile_res.json::<McProfile>().await.map_err(|e| format!("Profile Parse Error: {}", e))?;
    let skin_url = profile.skins.iter()
            .find(|s| s.state == "ACTIVE")
            .or_else(|| profile.skins.first())
            .map(|s| s.url.clone())
            .unwrap_or_default();
    // --- Step 5: 获取状态并持久化 ---
    {
        let state = handle.state::<Mutex<AuthState>>();
        let mut s = state.lock().map_err(|_| "Failed to acquire lock")?;

        s.users.retain(|u| u.uuid != profile.id);
        s.users.push(UserInfo {
            uuid: profile.id.clone(),
            name: profile.name.clone(),
            access_token: mc_access_token.to_string(),
            refresh_token: ms_refresh_token.to_string(),
            auth_type: "Microsoft".into(),
            skin_url,
        });
        s.current_user_id = Some(profile.id.clone());

        // 执行同步保存到磁盘
        s.save().map_err(|e| format!("Disk save failed: {}", e))?;
        println!("[Auth] State memory and disk updated for: {}", profile.name);
    }

    // --- Step 6: 发送成功信号 ---
    handle.emit("ms-login-success", profile.clone()).unwrap();

    Ok(profile)
}

#[tauri::command]
pub async fn yggdrasil_login(
    payload: AuthPayload,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<AuthResponse, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "agent": { "name": "Minecraft", "version": 1 },
        "username": payload.email,
        "password": payload.password,
        "requestUser": true
    });

    let res = client.post("https://littleskin.cn/api/yggdrasil/authserver/authenticate")
        .json(&body).send().await.map_err(|e| e.to_string())?;

    let data: Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(msg) = data.get("errorMessage").and_then(|v| v.as_str()) {
        return Err(msg.to_string());
    }

    if let Some(selected) = data.get("selectedProfile").filter(|v| !v.is_null()) {
        let user = process_user_info(&client, &data["accessToken"], selected).await?;
        let mut s = state.lock().unwrap();
        s.users.retain(|u| u.uuid != user.uuid);
        s.users.push(user.clone());
        s.current_user_id = Some(user.uuid.clone());
        let _ = s.save();
        Ok(AuthResponse::Success(user))
    } else {
        Ok(AuthResponse::NeedSelection {
            profiles: serde_json::from_value(data["availableProfiles"].clone()).unwrap_or_default(),
            access_token: data["accessToken"].as_str().unwrap_or("").to_string(),
            client_token: data["clientToken"].as_str().unwrap_or("").to_string(),
        })
    }
}

#[tauri::command]
pub async fn yggdrasil_select(
    payload: SelectPayload,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<UserInfo, String> {
    let client = reqwest::Client::new();
    let res = client.post("https://littleskin.cn/api/yggdrasil/authserver/refresh")
        .json(&serde_json::json!({
            "accessToken": payload.access_token,
            "clientToken": payload.client_token,
            "selectedProfile": payload.profile
        })).send().await.map_err(|e| e.to_string())?;

    let data: Value = res.json().await.map_err(|e| e.to_string())?;
    let user = process_user_info(&client, &data["accessToken"], &data["selectedProfile"]).await?;

    let mut s = state.lock().unwrap();
    s.users.retain(|u| u.uuid != user.uuid);
    s.users.push(user.clone());
    s.current_user_id = Some(user.uuid.clone());
    let _ = s.save();
    Ok(user)
}

pub async fn ensure_authenticated(
    uuid: &str,
    auth_type: &str,
    access_token: &str,
    state: &Mutex<AuthState>,
    handle: tauri::AppHandle,
) -> Result<(), String> {
    let client = reqwest::Client::new();

    match auth_type {
        "Yggdrasil" => {
            let base_url = "https://littleskin.cn/api/yggdrasil/authserver";

            // 1. Validate
            let val_res = client.post(format!("{}/validate", base_url))
                .json(&serde_json::json!({ "accessToken": access_token }))
                .send().await.map_err(|e| e.to_string())?;

            if val_res.status() == 204 { return Ok(()); }

            // 2. Refresh
            let ref_res = client.post(format!("{}/refresh", base_url))
                .json(&serde_json::json!({ "accessToken": access_token, "requestUser": true }))
                .send().await.map_err(|e| e.to_string())?;

            if ref_res.status().is_success() {
                let data: serde_json::Value = ref_res.json().await.map_err(|e| e.to_string())?;
                let updated_user = process_user_info(&client, &data["accessToken"], &data["selectedProfile"]).await?;

                let mut s = state.lock().unwrap();
                s.users.retain(|u| u.uuid != uuid);
                s.users.push(updated_user);
                let _ = s.save();
                return Ok(());
            }

            Err("YGGDRASIL_TOKEN_EXPIRED".into())
        },
        "Microsoft" => {
                    let prof_res = client.get("https://api.minecraftservices.com/minecraft/profile")
                        .bearer_auth(access_token)
                        .send().await.map_err(|e| e.to_string())?;

                    if prof_res.status().is_success() {
                        return Ok(());
                    }

                    println!("[Auth] Microsoft Access Token expired, attempting silent refresh...");

                    let current_refresh_token = {
                        let s = state.lock().unwrap();
                        s.users.iter()
                            .find(|u| u.uuid == uuid)
                            .map(|u| u.refresh_token.clone())
                            .ok_or("User credential not found in state")?
                    };

                    let (new_ms_access, new_ms_refresh) = refresh_ms_token(&current_refresh_token).await
                        .map_err(|_| "MS_TOKEN_EXPIRED".to_string())?;

                    authenticate_minecraft(&new_ms_access, &new_ms_refresh, handle).await?;

                    Ok(())
                },
                _ => Ok(()),
    }
}

pub async fn refresh_ms_token(refresh_token: &str) -> Result<(String, String), String> {
    let client = reqwest::Client::new();
    let url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";

    let params = [
        ("client_id", CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", "XboxLive.signin offline_access"),
    ];

    let res = client.post(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("MS Refresh Network Error: {}", e))?;

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

    if let Some(err) = data["error"].as_str() {
        return Err(format!("MS Refresh OAuth Error: {}", err));
    }

    let new_access = data["access_token"].as_str().ok_or("No access token")?;
    let new_refresh = data["refresh_token"].as_str().unwrap_or(refresh_token);

    Ok((new_access.to_string(), new_refresh.to_string()))
}