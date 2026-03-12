use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct AuthPayload {
    pub email: String,
    pub password: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectPayload {
    pub access_token: String,
    pub client_token: String,
    pub profile: Profile,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub java_path: String,
    pub max_memory: u64,
}

impl Config {
    pub fn file_path() -> PathBuf {
        let mut path = dirs_next::data_dir().unwrap_or_else(|| std::env::current_dir().unwrap());
        path.push("circube-launcher");
        let _ = fs::create_dir_all(&path);
        path.push("config.json");
        path
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::file_path())
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or(Config {
                java_path: "".into(),
                max_memory: 4096,
            })
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(Self::file_path(), json)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub name: String,
    pub uuid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub skin_url: String,
    pub auth_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McProfile {
    pub id: String,
    pub name: String,
    pub skins: Vec<Skin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skin {
    pub id: String,
    pub state: String,
    pub url: String,
    pub variant: String,
}

#[derive(Serialize)]
#[serde(tag = "status", content = "data")]
pub enum AuthResponse {
    #[serde(rename = "success")]
    Success(UserInfo),
    #[serde(rename = "need_selection")]
    NeedSelection {
        profiles: Vec<Profile>,
        access_token: String,
        client_token: String,
    },
}

#[derive(Deserialize)]
pub struct DeviceCodeResponse {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub interval: u64,
}

#[derive(Serialize)]
pub struct JavaInfo {
    pub path: String,
    pub version: String,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct AuthState {
    pub current_user_id: Option<String>,
    pub users: Vec<UserInfo>,
}

impl AuthState {
    pub fn file_path() -> PathBuf {
        let mut path = dirs_next::data_dir().unwrap_or_else(|| std::env::current_dir().unwrap());
        path.push("circube-launcher");
        let _ = fs::create_dir_all(&path);
        path.push("auth_state.json");
        path
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::file_path())
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(Self::file_path(), json)
    }
}

#[derive(Deserialize, Serialize)]
pub struct Manifest {
    pub manifest_version: String,
    pub version: String,
    pub files: std::collections::HashMap<String, FileItem>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct FileItem {
    pub hash: String,
    pub size: u64,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ProgressPayload {
    pub current: usize,
    pub total: usize,
    pub file: String,
}

pub static VERSION: OnceLock<String> = OnceLock::new();