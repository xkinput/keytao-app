use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct DownloadUrls {
    pub macos: Option<String>,
    pub windows: Option<String>,
    pub linux: Option<String>,
    pub android: Option<String>,
    pub ios: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PlatformRelease {
    pub version: String,
    pub download_urls: DownloadUrls,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub name: String,
    pub published_at: String,
    pub body: String,
    pub github: Option<PlatformRelease>,
    pub gitee: Option<PlatformRelease>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SchemeReleaseInfo {
    pub scheme: String,
    pub source_type: Option<String>,
    pub label: String,
    pub version: String,
    pub name: String,
    pub published_at: Option<String>,
    pub download_url: String,
    pub asset_name: String,
}

#[derive(Serialize, Clone)]
pub struct FileItem {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Serialize, Clone)]
pub struct VerifyEntry {
    pub path: String,
    pub ok: bool,
    pub note: String,
}

#[derive(Serialize, Clone)]
pub struct InstallResult {
    pub merged_schemas: Vec<String>,
    pub logs: Vec<String>,
    pub verify: Vec<VerifyEntry>,
}

#[derive(Serialize, Clone)]
pub struct AddonSchemaStatus {
    pub installed: bool,
    pub deployed: bool,
    pub version: String,
    pub english_available: bool,
}

#[derive(Serialize)]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
}

#[derive(Serialize, Clone)]
pub struct LocalSchemaInfo {
    pub installed: bool,
    pub deployed: bool,
    pub version: Option<String>,
    pub schemas: Vec<String>,
}
