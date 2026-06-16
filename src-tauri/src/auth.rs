use crate::models::*;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use reqwest::Client;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

pub const CLIENT_ID: &str = match option_env!("MS_CLIENT_ID") {
    Some(id) => id,
    None => "DEFAULT_ID_OR_ERROR",
};
const MS_SCOPE: &str = "XboxLive.signin offline_access";
const DEVICE_CODE_EXPIRES_IN_FALLBACK: u64 = 900;
const MC_TOKEN_EXPIRES_IN_FALLBACK: u64 = 24 * 60 * 60;
const TOKEN_REFRESH_SKEW_SECS: i64 = 10 * 60;
static MS_LOGIN_SESSION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
enum TokenRefreshError {
    Expired(String),
    Temporary(String),
}

fn microsoft_client_id() -> Result<&'static str, String> {
    let client_id = CLIENT_ID.trim();
    if client_id.is_empty() || client_id == "DEFAULT_ID_OR_ERROR" {
        return Err("未配置 Microsoft OAuth 客户端 ID。请在构建时设置 MS_CLIENT_ID。".into());
    }
    Ok(client_id)
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn token_expires_at(expires_in: Option<u64>, fallback_secs: u64) -> Option<i64> {
    let secs = expires_in.unwrap_or(fallback_secs);
    if secs > i64::MAX as u64 {
        return None;
    }

    unix_timestamp().checked_add(secs as i64)
}

fn is_token_fresh(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|expires_at| {
        expires_at > unix_timestamp().saturating_add(TOKEN_REFRESH_SKEW_SECS)
    })
}

fn is_microsoft_refresh_expired(body: &Value) -> bool {
    body.get("error")
        .and_then(Value::as_str)
        .is_some_and(|error| error.eq_ignore_ascii_case("invalid_grant"))
}

fn yggdrasil_token_body(access_token: &str, client_token: &str, request_user: bool) -> Value {
    let mut body = serde_json::json!({ "accessToken": access_token });
    if !client_token.is_empty() {
        body["clientToken"] = serde_json::json!(client_token);
    }
    if request_user {
        body["requestUser"] = serde_json::json!(true);
    }
    body
}

async fn response_json(res: reqwest::Response, context: &str) -> Result<Value, String> {
    let status = res.status();
    let body = response_body_json(res, context).await?;
    auth_debug_json(context, &body);

    if status.is_success() {
        Ok(body)
    } else {
        Err(format_service_error(context, status.as_u16(), &body))
    }
}

async fn response_body_json(res: reqwest::Response, context: &str) -> Result<Value, String> {
    let text = res
        .text()
        .await
        .map_err(|e| format!("{}响应读取失败: {}", context, e))?;

    let body = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("{}响应不是有效 JSON: {} ({})", context, e, text))?
    };

    Ok(body)
}

fn service_error_message(body: &Value) -> String {
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| body.get("code").and_then(Value::as_str));

    let message = body
        .get("error_description")
        .and_then(Value::as_str)
        .or_else(|| body.get("message").and_then(Value::as_str))
        .or_else(|| body.get("Message").and_then(Value::as_str));

    match (error, message) {
        (Some(error), Some(message)) if !message.is_empty() => format!("{} - {}", error, message),
        (Some(error), _) => error.to_string(),
        (_, Some(message)) => message.to_string(),
        _ => body.to_string(),
    }
}

fn format_service_error(context: &str, status: u16, body: &Value) -> String {
    let detail = service_error_message(body);

    if detail
        .to_ascii_lowercase()
        .contains("invalid app registration")
    {
        return format!(
            "{}失败 (HTTP {}): Microsoft 应用注册无效或没有 Minecraft Services 登录权限。请确认 MS_CLIENT_ID 对应的应用仍可使用 Xbox Live/Minecraft 登录。原始信息: {}",
            context, status, detail
        );
    }

    if let Some(xerr) = body.get("XErr").and_then(Value::as_u64) {
        let reason = match xerr {
            2148916233 => "该 Microsoft 账号没有 Xbox 账号，请先在 xbox.com 创建档案。",
            2148916235 => "Xbox Live 在当前地区不可用。",
            2148916236 | 2148916237 => "该账号需要成年人同意后才能使用 Xbox Live。",
            2148916238 => "该账号是儿童账号，无法直接使用此登录方式。",
            _ => "Xbox Live 返回了未知限制。",
        };
        return format!(
            "{}失败 (HTTP {}, XErr {}): {}",
            context, status, xerr, reason
        );
    }

    format!("{}失败 (HTTP {}): {}", context, status, detail)
}

fn emit_text(handle: &tauri::AppHandle, event: &str, payload: impl Into<String>) {
    let _ = handle.emit(event, payload.into());
}

fn auth_debug(step: &str, message: impl AsRef<str>) {
    println!("[Auth][{}] {}", step, message.as_ref());
}

fn auth_debug_json(step: &str, value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => auth_debug(step, format!("response JSON:\n{}", json)),
        Err(e) => auth_debug(step, format!("response JSON stringify failed: {}", e)),
    }
}

fn emit_status(handle: &tauri::AppHandle, step: &str, message: impl AsRef<str>) {
    let text = format!("[{}] {}", step, message.as_ref());
    auth_debug(step, message);
    emit_text(handle, "ms-status", text);
}

fn is_current_ms_session(session_id: u64) -> bool {
    MS_LOGIN_SESSION.load(Ordering::SeqCst) == session_id
}

async fn check_minecraft_entitlements(
    client: &reqwest::Client,
    mc_access_token: &str,
    handle: &tauri::AppHandle,
) -> Result<(), String> {
    emit_status(handle, "Entitlements", "正在验证 Minecraft 拥有权");
    let res = client
        .get("https://api.minecraftservices.com/entitlements/mcstore")
        .bearer_auth(mc_access_token)
        .send()
        .await
        .map_err(|e| format!("Entitlements Request Failed: {}", e))?;

    let status = res.status();
    auth_debug("Entitlements", format!("mcstore HTTP {}", status));
    let data = response_body_json(res, "Minecraft 拥有权验证").await?;
    auth_debug_json("Entitlements", &data);

    if !status.is_success() {
        return Err(format_service_error(
            "Minecraft 拥有权验证",
            status.as_u16(),
            &data,
        ));
    }

    let has_items = data
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());

    if !has_items {
        return Err("该账号尚未购买 Minecraft Java，或 Xbox Game Pass 已到期。".into());
    }

    emit_status(handle, "Entitlements", "Minecraft 拥有权验证通过");
    Ok(())
}

pub async fn process_user_info(
    client: &reqwest::Client,
    access_token: &serde_json::Value,
    profile_data: &serde_json::Value,
) -> Result<UserInfo, String> {
    let profile_id = profile_data["id"].as_str().ok_or("Invalid profile id")?;
    let profile_name = profile_data["name"].as_str().unwrap_or("Player");

    let texture_url = format!(
        "https://littleskin.cn/api/yggdrasil/sessionserver/session/minecraft/profile/{}",
        profile_id
    );
    let texture_res = client
        .get(&texture_url)
        .send()
        .await
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
        client_token: "".into(),
        skin_url,
        auth_type: "Yggdrasil".into(),
        expires_at: None,
    })
}

#[tauri::command]
pub async fn ms_login(
    client: tauri::State<'_, Client>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let client_id = microsoft_client_id()?;
    let session_id = MS_LOGIN_SESSION.fetch_add(1, Ordering::SeqCst) + 1;
    emit_status(
        &app,
        "DeviceCode",
        format!("正在向 Microsoft 请求设备码，session={}", session_id),
    );
    let params = [
        ("client_id", client_id),
        ("tenant", "consumers"),
        ("scope", MS_SCOPE),
    ];

    let res = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    auth_debug("DeviceCode", format!("devicecode HTTP {}", res.status()));

    let data: DeviceCodeResponse =
        serde_json::from_value(response_json(res, "获取微软设备码").await?)
            .map_err(|e| format!("获取微软设备码响应解析失败: {}", e))?;
    let user_code = data.user_code.clone();
    let device_code = data.device_code.clone();
    let interval = data.interval;
    let expires_in = data.expires_in.unwrap_or(DEVICE_CODE_EXPIRES_IN_FALLBACK);

    // 打开浏览器
    let verification_uri = data
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&data.verification_uri);
    app.opener()
        .open_url(verification_uri, None::<String>)
        .map_err(|e| e.to_string())?;
    emit_status(
        &app,
        "DeviceCode",
        format!(
            "设备码已生成，user_code={}，interval={}s，expires_in={}s",
            user_code, interval, expires_in
        ),
    );

    let handle = app.clone();
    let poll_client = client.inner().clone();
    tauri::async_runtime::spawn(async move {
        let poll_url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
        let mut poll_interval = interval.max(1);
        let mut poll_attempt = 0u32;

        while std::time::Instant::now() < expires_at {
            if !is_current_ms_session(session_id) {
                auth_debug(
                    "Poll",
                    format!("检测到新登录会话，停止旧轮询 session={}", session_id),
                );
                break;
            }

            tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
            if !is_current_ms_session(session_id) {
                auth_debug(
                    "Poll",
                    format!("检测到新登录会话，停止旧轮询 session={}", session_id),
                );
                break;
            }

            poll_attempt += 1;
            emit_status(
                &handle,
                "Poll",
                format!(
                    "正在轮询 Microsoft Token，第 {} 次，session={}",
                    poll_attempt, session_id
                ),
            );
            let poll_params = [
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", &device_code),
            ];

            match poll_client.post(poll_url).form(&poll_params).send().await {
                Ok(r) => {
                    let status = r.status();
                    auth_debug("Poll", format!("token HTTP {}", status));
                    let body = match r.json::<serde_json::Value>().await {
                        Ok(body) => body,
                        Err(e) => {
                            emit_text(
                                &handle,
                                "ms-login-error",
                                format!("微软登录响应解析失败: {}", e),
                            );
                            break;
                        }
                    };
                    auth_debug_json("Poll", &body);

                    if status.is_success() {
                        let access_token = body["access_token"].as_str().unwrap_or("");
                        let refresh_token = body["refresh_token"].as_str().unwrap_or("");
                        if !is_current_ms_session(session_id) {
                            auth_debug(
                                "Poll",
                                format!("Token 已返回但会话已过期，丢弃 session={}", session_id),
                            );
                            break;
                        }
                        emit_status(
                            &handle,
                            "Poll",
                            format!(
                                "Microsoft Token 获取成功，access_token_len={}，refresh_token_present={}",
                                access_token.len(),
                                !refresh_token.is_empty()
                            ),
                        );

                        if let Err(e) = authenticate_minecraft(
                            poll_client.clone(),
                            access_token,
                            refresh_token,
                            handle.clone(),
                            Some(session_id),
                        )
                        .await
                        {
                            if is_current_ms_session(session_id) {
                                emit_text(&handle, "ms-login-error", format!("验证失败: {}", e));
                            }
                        }
                        break;
                    } else {
                        let error = body["error"].as_str().unwrap_or("");
                        match error {
                            "authorization_pending" => {
                                auth_debug("Poll", "用户尚未完成 Microsoft 授权");
                                continue;
                            }
                            "slow_down" => {
                                poll_interval += 5;
                                emit_status(
                                    &handle,
                                    "Poll",
                                    format!("Microsoft 要求放慢轮询，新的间隔={}s", poll_interval),
                                );
                                continue;
                            }
                            "authorization_declined" => {
                                emit_text(&handle, "ms-login-error", "用户取消了 Microsoft 授权。");
                                break;
                            }
                            "expired_token" => {
                                emit_text(
                                    &handle,
                                    "ms-login-error",
                                    "Microsoft 设备码已过期，请重新获取。",
                                );
                                break;
                            }
                            _ => {
                                emit_text(
                                    &handle,
                                    "ms-login-error",
                                    format_service_error("轮询微软登录", status.as_u16(), &body),
                                );
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    emit_text(
                        &handle,
                        "ms-login-error",
                        format!("轮询微软登录失败: {}", e),
                    );
                    break;
                }
            }
        }

        if is_current_ms_session(session_id) && std::time::Instant::now() >= expires_at {
            emit_text(
                &handle,
                "ms-login-error",
                "Microsoft 设备码已过期，请重新获取。",
            );
        }
    });

    Ok(user_code)
}

pub async fn authenticate_minecraft(
    client: reqwest::Client,
    ms_access_token: &str,
    ms_refresh_token: &str,
    handle: tauri::AppHandle,
    login_session_id: Option<u64>,
) -> Result<McProfile, String> {
    // --- Step 1: XBL ---
    emit_status(&handle, "XBL", "正在请求 Xbox Live Token");
    let xbl_res = client
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms_access_token)
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .send()
        .await
        .map_err(|e| format!("XBL Request Failed: {}", e))?;

    auth_debug(
        "XBL",
        format!("user/authenticate HTTP {}", xbl_res.status()),
    );
    let xbl_data = response_json(xbl_res, "Xbox Live 认证").await?;
    let xbl_token = xbl_data["Token"].as_str().ok_or("XBL Token missing")?;
    let user_hash = xbl_data["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .ok_or("UHS missing")?;
    emit_status(
        &handle,
        "XBL",
        format!(
            "Xbox Live Token 获取成功，token_len={}，uhs_present={}",
            xbl_token.len(),
            !user_hash.is_empty()
        ),
    );

    // --- Step 2: XSTS ---
    emit_status(&handle, "XSTS", "正在请求 XSTS Token");
    let xsts_res = client
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .json(&serde_json::json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send()
        .await
        .map_err(|e| format!("XSTS Request Failed: {}", e))?;

    auth_debug("XSTS", format!("xsts/authorize HTTP {}", xsts_res.status()));
    let xsts_data = response_json(xsts_res, "XSTS 授权").await?;
    let xsts_token = xsts_data["Token"].as_str().ok_or("XSTS Token missing")?;
    emit_status(
        &handle,
        "XSTS",
        format!("XSTS Token 获取成功，token_len={}", xsts_token.len()),
    );

    // --- Step 3: Minecraft Login ---
    let identity_token = format!("XBL3.0 x={};{}", user_hash, xsts_token);
    if login_session_id.is_some_and(|session_id| !is_current_ms_session(session_id)) {
        return Err("登录会话已被新的 Microsoft 登录取代".into());
    }
    emit_status(&handle, "Minecraft", "正在登录 Minecraft Services");
    let mc_login_res = client
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({ "identityToken": identity_token }))
        .send()
        .await
        .map_err(|e| format!("MC Login Request Failed: {}", e))?;

    let mc_login_status = mc_login_res.status();
    auth_debug(
        "Minecraft",
        format!("login_with_xbox HTTP {}", mc_login_status),
    );
    let mc_data = response_body_json(mc_login_res, "Minecraft Services 登录").await?;
    auth_debug_json("Minecraft", &mc_data);

    if !mc_login_status.is_success() {
        auth_debug(
            "Minecraft",
            format!("login_with_xbox error body: {}", mc_data),
        );
        return Err(format_service_error(
            "Minecraft Services 登录",
            mc_login_status.as_u16(),
            &mc_data,
        ));
    }

    emit_status(&handle, "Minecraft", "Minecraft Services 登录成功");
    if login_session_id.is_some_and(|session_id| !is_current_ms_session(session_id)) {
        return Err("登录会话已被新的 Microsoft 登录取代".into());
    }
    let mc_access_token = mc_data["access_token"]
        .as_str()
        .ok_or("MC Access Token missing")?;
    let mc_expires_at = token_expires_at(
        mc_data.get("expires_in").and_then(Value::as_u64),
        MC_TOKEN_EXPIRES_IN_FALLBACK,
    );
    emit_status(
        &handle,
        "Minecraft",
        format!(
            "Minecraft Access Token 获取成功，token_len={}",
            mc_access_token.len()
        ),
    );

    // --- Step 4: Entitlements ---
    check_minecraft_entitlements(&client, mc_access_token, &handle).await?;

    // --- Step 5: Profile ---
    emit_status(&handle, "Profile", "正在获取 Minecraft 档案");
    let profile_res = client
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(mc_access_token)
        .send()
        .await
        .map_err(|e| format!("Profile Request Failed: {}", e))?;

    auth_debug(
        "Profile",
        format!("minecraft/profile HTTP {}", profile_res.status()),
    );
    let profile =
        serde_json::from_value::<McProfile>(response_json(profile_res, "Minecraft 档案").await?)
            .map_err(|e| format!("Profile Parse Error: {}", e))?;
    if login_session_id.is_some_and(|session_id| !is_current_ms_session(session_id)) {
        return Err("登录会话已被新的 Microsoft 登录取代".into());
    }
    emit_status(
        &handle,
        "Profile",
        format!("档案获取成功，name={}，uuid={}", profile.name, profile.id),
    );
    let skin_url = profile
        .skins
        .iter()
        .find(|s| s.state == "ACTIVE")
        .or_else(|| profile.skins.first())
        .map(|s| s.url.clone())
        .unwrap_or_default();
    // --- Step 6: 获取状态并持久化 ---
    {
        let state = handle.state::<Mutex<AuthState>>();
        let mut s = state.lock().map_err(|_| "Failed to acquire lock")?;

        s.users.retain(|u| u.uuid != profile.id);
        s.users.push(UserInfo {
            uuid: profile.id.clone(),
            name: profile.name.clone(),
            access_token: mc_access_token.to_string(),
            refresh_token: ms_refresh_token.to_string(),
            client_token: "".into(),
            auth_type: "Microsoft".into(),
            skin_url,
            expires_at: mc_expires_at,
        });
        s.current_user_id = Some(profile.id.clone());

        // 执行同步保存到磁盘
        s.save().map_err(|e| format!("Disk save failed: {}", e))?;
        println!("[Auth] State memory and disk updated for: {}", profile.name);
    }

    // --- Step 6: 发送成功信号 ---
    if match login_session_id {
        Some(session_id) => is_current_ms_session(session_id),
        None => true,
    } {
        handle.emit("ms-login-success", profile.clone()).unwrap();
    }

    Ok(profile)
}

#[tauri::command]
pub async fn yggdrasil_login(
    client: tauri::State<'_, Client>,
    payload: AuthPayload,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<AuthResponse, String> {
    let body = serde_json::json!({
        "agent": { "name": "Minecraft", "version": 1 },
        "username": payload.email,
        "password": payload.password,
        "requestUser": true
    });

    let res = client
        .post("https://littleskin.cn/api/yggdrasil/authserver/authenticate")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(msg) = data.get("errorMessage").and_then(|v| v.as_str()) {
        return Err(msg.to_string());
    }

    if let Some(selected) = data.get("selectedProfile").filter(|v| !v.is_null()) {
        let mut user = process_user_info(&client, &data["accessToken"], selected).await?;
        user.client_token = data["clientToken"].as_str().unwrap_or("").to_string();
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
    client: tauri::State<'_, Client>,
    payload: SelectPayload,
    state: tauri::State<'_, Mutex<AuthState>>,
) -> Result<UserInfo, String> {
    let client_token = payload.client_token.clone();
    let res = client
        .post("https://littleskin.cn/api/yggdrasil/authserver/refresh")
        .json(&serde_json::json!({
            "accessToken": payload.access_token,
            "clientToken": &client_token,
            "selectedProfile": payload.profile
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: Value = res.json().await.map_err(|e| e.to_string())?;
    let mut user =
        process_user_info(&client, &data["accessToken"], &data["selectedProfile"]).await?;
    user.client_token = data["clientToken"]
        .as_str()
        .unwrap_or(&client_token)
        .to_string();

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
    client: tauri::State<'_, Client>,
) -> Result<(), String> {
    match auth_type {
        "Yggdrasil" => {
            let base_url = "https://littleskin.cn/api/yggdrasil/authserver";
            let current_client_token = {
                let s = state.lock().unwrap();
                s.users
                    .iter()
                    .find(|u| u.uuid == uuid)
                    .map(|u| u.client_token.clone())
                    .unwrap_or_default()
            };

            // 1. Validate
            let val_res = client
                .post(format!("{}/validate", base_url))
                .json(&yggdrasil_token_body(
                    access_token,
                    &current_client_token,
                    false,
                ))
                .send()
                .await
                .map_err(|e| format!("认证失败: Yggdrasil 验证请求失败: {}", e))?;

            if val_res.status() == 204 {
                return Ok(());
            }
            let validate_status = val_res.status();
            if !matches!(validate_status.as_u16(), 400 | 401 | 403) {
                return Err(format!(
                    "认证失败: Yggdrasil 验证服务暂时不可用 (HTTP {})",
                    validate_status.as_u16()
                ));
            }

            // 2. Refresh
            let ref_res = client
                .post(format!("{}/refresh", base_url))
                .json(&yggdrasil_token_body(
                    access_token,
                    &current_client_token,
                    true,
                ))
                .send()
                .await
                .map_err(|e| format!("认证失败: Yggdrasil 刷新请求失败: {}", e))?;

            let refresh_status = ref_res.status();
            let data = response_body_json(ref_res, "刷新 Yggdrasil Token")
                .await
                .map_err(|e| format!("认证失败: {}", e))?;
            if refresh_status.is_success() {
                let mut updated_user =
                    process_user_info(&client, &data["accessToken"], &data["selectedProfile"])
                        .await?;
                updated_user.client_token = data["clientToken"]
                    .as_str()
                    .unwrap_or(&current_client_token)
                    .to_string();

                let mut s = state.lock().unwrap();
                s.users.retain(|u| u.uuid != uuid);
                s.current_user_id = Some(updated_user.uuid.clone());
                s.users.push(updated_user);
                let _ = s.save();
                return Ok(());
            }

            if matches!(refresh_status.as_u16(), 400 | 401 | 403) {
                Err("YGGDRASIL_TOKEN_EXPIRED".into())
            } else {
                Err(format!(
                    "认证失败: Yggdrasil Token 刷新失败 (HTTP {}): {}",
                    refresh_status.as_u16(),
                    service_error_message(&data)
                ))
            }
        }
        "Microsoft" => {
            let (current_refresh_token, expires_at) = {
                let s = state.lock().unwrap();
                let user = s
                    .users
                    .iter()
                    .find(|u| u.uuid == uuid)
                    .ok_or("User credential not found in state")?;
                (user.refresh_token.clone(), user.expires_at)
            };

            if current_refresh_token.trim().is_empty() {
                return Err("MS_TOKEN_EXPIRED".into());
            }

            if is_token_fresh(expires_at) {
                auth_debug("Microsoft", "cached Minecraft access token is still fresh");
                return Ok(());
            }

            if expires_at.is_none() {
                let prof_res = client
                    .get("https://api.minecraftservices.com/minecraft/profile")
                    .bearer_auth(access_token)
                    .send()
                    .await
                    .map_err(|e| format!("认证失败: Microsoft 档案验证请求失败: {}", e))?;

                let profile_status = prof_res.status();
                if profile_status.is_success() {
                    return Ok(());
                }

                if !matches!(profile_status.as_u16(), 401 | 403) {
                    return Err(format!(
                        "认证失败: Microsoft 档案验证失败 (HTTP {})",
                        profile_status.as_u16()
                    ));
                }
            }

            println!("[Auth] Microsoft Access Token is stale, attempting silent refresh...");

            let (new_ms_access, new_ms_refresh) =
                match refresh_ms_token(client.clone(), &current_refresh_token).await {
                    Ok(tokens) => tokens,
                    Err(TokenRefreshError::Expired(message)) => {
                        auth_debug("MSRefresh", message);
                        return Err("MS_TOKEN_EXPIRED".into());
                    }
                    Err(TokenRefreshError::Temporary(message)) => {
                        return Err(format!(
                            "认证失败: Microsoft Token 刷新暂时不可用: {}",
                            message
                        ));
                    }
                };

            authenticate_minecraft(
                client.inner().clone(),
                &new_ms_access,
                &new_ms_refresh,
                handle,
                None,
            )
            .await?;

            Ok(())
        }
        _ => Ok(()),
    }
}

async fn refresh_ms_token(
    client: tauri::State<'_, Client>,
    refresh_token: &str,
) -> Result<(String, String), TokenRefreshError> {
    let url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
    let client_id = microsoft_client_id().map_err(TokenRefreshError::Temporary)?;

    let params = [
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", MS_SCOPE),
    ];

    let res =
        client.post(url).form(&params).send().await.map_err(|e| {
            TokenRefreshError::Temporary(format!("MS Refresh Network Error: {}", e))
        })?;

    let status = res.status();
    auth_debug("MSRefresh", format!("refresh_token HTTP {}", status));
    let data = response_body_json(res, "刷新 Microsoft Token")
        .await
        .map_err(TokenRefreshError::Temporary)?;
    auth_debug_json("MSRefresh", &data);

    if !status.is_success() {
        let message = format_service_error("刷新 Microsoft Token", status.as_u16(), &data);
        if is_microsoft_refresh_expired(&data) {
            return Err(TokenRefreshError::Expired(message));
        }
        return Err(TokenRefreshError::Temporary(message));
    }

    let new_access = data["access_token"].as_str().ok_or_else(|| {
        TokenRefreshError::Temporary("Microsoft 刷新响应缺少 access_token".into())
    })?;
    let new_refresh = data["refresh_token"].as_str().unwrap_or(refresh_token);

    Ok((new_access.to_string(), new_refresh.to_string()))
}
