#[cfg(target_os = "windows")]
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

mod app_core;
mod log_shell;
pub use keytao_app_core::logs::{DebugLogFile, DebugLogs, RuntimeLogLevel, RuntimeLogSettings};
mod scheme_shell;
mod ime_shell;
use ime_shell::*;
use keytao_app_core::ime_paths::*;
pub use keytao_app_core::{deploy::DeployResult, ime_settings::{ImeUiSettings, AndroidImeInputSettings}, component_versions::ComponentVersions, ime_status::macos::MacosImeStatus};
#[cfg(target_os = "linux")]
use keytao_app_core::ime_status::linux::{ManagedImeHelper, LinuxImeStatus};
use keytao_app_core::{Core, wanxiang};
use std::sync::Arc;
use scheme_shell::*;
pub use keytao_app_core::scheme_types::*;
#[cfg(target_os = "windows")]
use keytao_app_core::scheme_files::parse_schema_list;
#[cfg(target_os = "windows")]
use keytao_app_core::addon::{addon_schema_status_at, addon_schema_source_files, EASY_EN_ADDON_ID};
#[cfg(target_os = "android")]
use keytao_app_core::install::install_result_from_value;

pub use keytao_app_core::events::{InstallProgress, WindowsImeStatus};
#[cfg(target_os = "windows")]
mod windows_app_actions;
#[cfg(target_os = "windows")]
mod windows_public_schemas;


#[cfg(not(any(target_os = "android", target_os = "ios")))]
use keytao_core;

#[cfg(target_os = "linux")]
mod rime;

// Linux protocol frontends live in the keytao-ime daemon. The GUI only deploys
// assets and starts the daemon when needed.

pub use keytao_app_core::account::{AppAuthSession, AppAuthUser, UserDictionarySyncResult};
#[cfg(target_os = "ios")]
const IOS_APP_GROUP_IDENTIFIER: &str = "group.ink.rea.keytao-app";


#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "linux")))]
#[tauri::command]
fn rime_get_data_dir(core: tauri::State<'_, Arc<Core>>) -> Option<String> {
    keytao_app_core::scheme::rime_get_data_dir(&core).expect("data directory lookup is infallible")
}

#[cfg(target_os = "ios")]
#[tauri::command]
fn rime_get_data_dir<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Option<String> {
    keytao_app_core::scheme::rime_get_data_dir(&app.state::<Arc<Core>>(), ios_keytao_root(&app).ok())
        .expect("data directory lookup is infallible")
}


#[cfg(target_os = "ios")]
fn ios_app_group_container() -> Option<PathBuf> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSFileManager, NSString};

    autoreleasepool(|_| {
        let manager = NSFileManager::defaultManager();
        let group = NSString::from_str(IOS_APP_GROUP_IDENTIFIER);
        let url = manager.containerURLForSecurityApplicationGroupIdentifier(&group)?;
        url.to_file_path()
    })
}

#[cfg(target_os = "ios")]
fn ios_keytao_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    if let Ok(override_dir) = std::env::var("KEYTAO_IOS_USER_DATA_DIR") {
        if !override_dir.trim().is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }

    if let Some(container) = ios_app_group_container() {
        return Ok(container.join("keytao"));
    }

    app.path()
        .app_data_dir()
        .map(|dir| dir.join("keytao"))
        .map_err(|e| format!("Cannot determine iOS KeyTao data directory: {e}"))
}

#[cfg(target_os = "windows")]
use keytao_app_core::ime_status::windows::{WindowsImeEngineInitGuard, WindowsImeProfileDeploymentGuard};

#[tauri::command]
#[cfg(target_os = "windows")]
async fn windows_ime_status(app: AppHandle) -> Result<WindowsImeStatus, String> {
    keytao_app_core::ime_status::windows::windows_ime_status(app.state::<Arc<Core>>().inner().clone()).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "windows")]
async fn windows_prepare_search_schemas(app: AppHandle) -> Result<(), String> {
    static PREPARING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = PREPARING.lock().await;
    if windows_public_schemas::has_schemas() {
        return Ok(());
    }
    let root = default_keytao_user_root(&app)?;
    let installed = keytao_core::schema_install_state(&root);
    if !installed.installed {
        return Ok(());
    }
    // Infer the public distribution from schema IDs only. Personal source and
    // learned dictionaries must never be copied into the sandbox-readable seed.
    let scheme = installed.schemas.iter().find_map(|id| {
        if id.starts_with("xmjd6") { Some("xmjd") }
        else if id.starts_with("txjx") { Some("txjx") }
        else if id.starts_with("keydo") { Some("keydo") }
        else if id.starts_with("keytao") { Some("keytao") }
        else { None }
    }).ok_or("无法识别公共方案来源，请在方案页重新安装主方案以启用系统搜索框输入")?;
    let _ = app.emit("install-progress", InstallProgress {
        stage: "windows-search".into(), percent: 0,
        message: "正在准备 Windows 系统搜索框使用的公共方案…".into(),
    });
    let release = fetch_scheme_release(app.clone(), scheme.into()).await?;
    let response = build_client(&app)?.get(&release.download_url)
        .timeout(std::time::Duration::from_secs(180)).send().await
        .map_err(|e| format!("下载搜索框公共方案失败：{e}"))?
        .error_for_status().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if bytes.len().saturating_add(chunk.len()) > 128 * 1024 * 1024 {
            return Err("搜索框公共方案压缩包过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    windows_public_schemas::publish_package(&bytes)?;
    if addon_schema_status_at(&root, EASY_EN_ADDON_ID).installed {
        publish_windows_english_addon(&app)?;
    }
    windows_public_schemas::publish_english_settings(
        keytao_core::english_mode::read_english_mode(&root) == keytao_core::english_mode::EnglishMode::Schema,
    )?;
    let _ = app.emit("install-progress", InstallProgress {
        stage: "done".into(), percent: 100,
        message: "Windows 系统搜索框公共方案已就绪".into(),
    });
    Ok(())
}

#[tauri::command]
#[cfg(target_os = "windows")]
async fn windows_ime_ensure_registered(app: AppHandle) -> Result<WindowsImeStatus, String> {
    keytao_app_core::ime_status::windows::windows_ime_ensure_registered(app.state::<Arc<Core>>().inner().clone()).await.map_err(|e| e.to_string())
}


#[tauri::command]
async fn keytao_login(core: tauri::State<'_, Arc<Core>>, name: String, password: String) -> Result<AppAuthSession, String> {
    keytao_app_core::account::keytao_login(&core, name, password).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn keytao_me(core: tauri::State<'_, Arc<Core>>, token: String) -> Result<AppAuthUser, String> {
    keytao_app_core::account::keytao_me(&core, token).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn app_state_import_legacy(
    core: tauri::State<'_, Arc<Core>>,
    token: Option<String>,
    user: Option<serde_json::Value>,
    android_onboarding_completed: bool,
) -> Result<(), String> {
    core.import_legacy_state(token, user, android_onboarding_completed).map_err(|e| e.to_string())
}

#[tauri::command]
fn app_state_clear_auth(core: tauri::State<'_, Arc<Core>>) -> Result<(), String> {
    core.clear_auth().map_err(|e| e.to_string())
}

#[tauri::command]
fn app_state_complete_onboarding(core: tauri::State<'_, Arc<Core>>) -> Result<(), String> {
    core.complete_onboarding().map_err(|e| e.to_string())
}

#[tauri::command]
async fn app_state_onboarding(app: AppHandle) -> keytao_app_core::state::OnboardingState {
    let core = app.state::<Arc<Core>>();
    if let Err(error) = core.initialize_onboarding(default_keytao_user_root(&app).ok()) {
        tracing::warn!(%error, "Failed to initialize onboarding");
    }
    core.onboarding()
}

fn default_keytao_user_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        android_keytao_root(app)
    }
    #[cfg(target_os = "ios")]
    {
        ios_keytao_root(app)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        keytao_core::default_user_data_dir().ok_or("Cannot determine keytao data directory".into())
    }
}

fn write_default_reload_stamp<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    root: &Path,
) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        write_android_reload_stamp(root).map(Some)
    }
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        write_ios_reload_stamp(root).map(Some)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        write_keytao_ime_reload_stamp().map(|()| Some(keytao_core::ReloadStamp::path(root)))
    }
}

#[tauri::command]
async fn sync_user_dictionary<R: tauri::Runtime>(app: tauri::AppHandle<R>, token: String) -> Result<UserDictionarySyncResult, String> {
    keytao_app_core::account::sync_user_dictionary(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone()), token).await.map_err(|e| e.to_string())
}

fn rime_default_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        keytao_core::default_user_data_dir()
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir().map(|c| c.join("Rime"))
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir()?;
        let fcitx5 = home.join(".local/share/fcitx5/rime");
        let ibus = home.join(".config/ibus/rime");
        if fcitx5.exists() {
            Some(fcitx5)
        } else if ibus.exists() {
            Some(ibus)
        } else {
            Some(fcitx5)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

#[tauri::command]
async fn select_directory(
    #[allow(unused_variables)] app: AppHandle,
    im_type: Option<String>,
) -> Result<Option<String>, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        use tauri_plugin_dialog::{DialogExt, FilePath};
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut builder = app.dialog().file();
        let resolved_default = im_type
            .as_deref()
            .and_then(|im| {
                let home = dirs::home_dir()?;
                match im {
                    "fcitx5" => Some(home.join(".local/share/fcitx5/rime")),
                    "ibus" => Some(home.join(".config/ibus/rime")),
                    _ => None,
                }
            })
            .or_else(rime_default_path);
        if let Some(default) = resolved_default {
            builder = builder.set_directory(default);
        }
        builder.pick_folder(move |folder: Option<FilePath>| {
            let _ = tx.send(folder);
        });
        let result = rx.await.map_err(|e| e.to_string())?;
        Ok(result.map(|p| p.to_string()))
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        Err("Not supported on this platform".into())
    }
}




// ─── Android plugin ──────────────────────────────────────────────────────────

#[cfg(target_os = "android")]
struct ScopedStorageHandle<R: tauri::Runtime>(tauri::plugin::PluginHandle<R>);

fn scoped_storage_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("scopedStorage")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin("ink.rea.keytao_app", "ScopedStoragePlugin")?;
                _app.manage(ScopedStorageHandle(handle));
            }
            Ok(())
        })
        .build()
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AndroidImeStatus {
    pub package_name: String,
    pub service_name: String,
    pub input_method_id: Option<String>,
    pub default_input_method: Option<String>,
    pub enabled: bool,
    pub selected: bool,
    pub can_show_picker: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AndroidStoragePermissionStatus {
    pub path: String,
    pub granted: bool,
    pub writable: bool,
    pub requires_manage_all_files: bool,
    pub can_open_settings: bool,
    pub message: String,
    #[serde(default)]
    pub migration_error: Option<String>,
    #[serde(default)]
    pub deploy_error: Option<String>,
}

#[tauri::command]
async fn android_ime_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<AndroidImeStatus, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("imeStatus", ())
            .map_err(|e| e.to_string())?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_storage_permission_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<AndroidStoragePermissionStatus, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("storagePermissionStatus", ())
            .map_err(|e| e.to_string())?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_open_storage_permission_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("openStoragePermissionSettings", ())
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_open_input_method_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("openInputMethodSettings", ())
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_show_input_method_picker<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("showInputMethodPicker", ())
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[cfg(target_os = "android")]
fn android_keytao_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    let result: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin("keytaoRoot", ())
        .map_err(|e| e.to_string())?;
    let path = result
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or("Android KeyTao data directory is unavailable")?;
    let root = PathBuf::from(path);
    // Setup may have run before file access was granted. Init is idempotent.
    keytao_core::runtime_log::init(&root, "android-app");
    Ok(root)
}



#[tauri::command]
async fn android_keytao_data_dir<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Option<String>, String> {
    keytao_app_core::scheme::android_keytao_data_dir(&app.state::<Arc<Core>>(), || {
        #[cfg(target_os = "android")]
        { android_keytao_root(&app) }
        #[cfg(not(target_os = "android"))]
        { unreachable!("Android root lookup is not called on other platforms") }
    }).map_err(|e| e.to_string())
}

#[tauri::command]
async fn android_pick_directory<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("pickDirectory", ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_list_files<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tree_uri: String,
) -> Result<Vec<FileItem>, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("listFiles", serde_json::json!({ "treeUri": tree_uri }))
            .map_err(|e| e.to_string())?;

        let files = result["files"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| FileItem {
                        name: v["name"].as_str().unwrap_or("").to_string(),
                        is_dir: v["isDir"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(files)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_read_local_schemas<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tree_uri: String,
) -> Result<LocalSchemaInfo, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "readLocalSchemas",
                serde_json::json!({ "treeUri": tree_uri }),
            )
            .map_err(|e| e.to_string())?;

        let schemas = result["schemas"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let version = result["version"].as_str().map(String::from);

        let installed = result["installed"].as_bool().unwrap_or(false);

        Ok(LocalSchemaInfo {
            installed,
            deployed: result["deployed"].as_bool().unwrap_or(false),
            version,
            schemas,
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_smart_extract<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    zip_path: String,
    tree_uri: String,
) -> Result<InstallResult, String> {
    #[cfg(target_os = "android")]
    {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: "extracting".into(),
                percent: 61,
                message: "正在解压...".into(),
            },
        );

        let app2 = app.clone();
        let on_progress: tauri::ipc::Channel<serde_json::Value> =
            tauri::ipc::Channel::new(move |body: tauri::ipc::InvokeResponseBody| {
                let data: serde_json::Value = match body {
                    tauri::ipc::InvokeResponseBody::Json(s) => {
                        serde_json::from_str(&s).unwrap_or_default()
                    }
                    tauri::ipc::InvokeResponseBody::Raw(b) => {
                        serde_json::from_slice(&b).unwrap_or_default()
                    }
                };
                if let (Some(stage), Some(percent), Some(message)) = (
                    data.get("stage").and_then(|v| v.as_str()).map(String::from),
                    data.get("percent")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    data.get("message")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                ) {
                    let _ = app2.emit(
                        "install-progress",
                        InstallProgress {
                            stage,
                            percent,
                            message,
                        },
                    );
                }
                Ok(())
            });

        // Explicit SAF export does not mirror another copy into KeyTao's live root.
        android_keytao_root(&app)?;
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "smartExtractZip",
                serde_json::json!({ "zipPath": zip_path, "treeUri": tree_uri, "onProgress": on_progress }),
            )
            .map_err(|e| e.to_string())?;

        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: "done".into(),
                percent: 100,
                message: "安装完成！".into(),
            },
        );

        Ok(install_result_from_value(&result))
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

// ─── macOS system IME management ─────────────────────────────────────────────

#[tauri::command]
fn macos_ime_status(app: AppHandle) -> MacosImeStatus {
    keytao_app_core::ime_status::macos::macos_ime_status(&app.state::<Arc<Core>>())
}

// ─── Local schema info ───────────────────────────────────────────────────────

#[tauri::command]
fn get_component_versions(app: AppHandle) -> ComponentVersions {
    keytao_app_core::component_versions::get_component_versions(
        &app.state::<Arc<Core>>(), &ShellImeHost(app.clone()), tauri::VERSION,
        option_env!("RIME_VERSION"), option_env!("OPENCC_VERSION"),
    )
}

#[cfg(target_os = "android")]
fn install_addon_schema_files<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    _root: &Path,
    id: &str,
) -> Result<(), String> {
    app.state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin::<serde_json::Value>(
            "copyAddonSchemaAssets",
            serde_json::json!({ "id": id }),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn publish_windows_english_addon(app: &AppHandle) -> Result<(), String> {
    let source = keytao_app_core::addon::bundled_addon_schema_dir(&app.state::<Arc<Core>>(), EASY_EN_ADDON_ID)?;
    let files = addon_schema_source_files(EASY_EN_ADDON_ID).into_iter()
        .map(|(source_relative, destination)| {
            std::fs::read(source.join(source_relative))
                .map(|bytes| (PathBuf::from(destination), bytes))
                .map_err(|e| e.to_string())
        }).collect::<Result<Vec<_>, _>>()?;
    windows_public_schemas::publish_files(&files)
}

#[tauri::command]
fn wanxiang_status(app: AppHandle) -> Result<wanxiang::Status, String> {
    wanxiang::wanxiang_status(&app.state::<Arc<Core>>(), &default_keytao_user_root(&app)?).map_err(|e| e.to_string())
}


// ─── Deploy librime to default keytao data dir ────────────────────────────────



#[tauri::command]
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
async fn rime_deploy_default(app: AppHandle) -> Result<DeployResult, String> {
    keytao_app_core::deploy::rime_deploy_default(&app.state::<Arc<Core>>(), ShellImeHost(app.clone()),
        #[cfg(target_os = "windows")]
        &app.state::<windows_app_actions::AppActions>(),
    ).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "android")]
async fn rime_deploy_default<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<DeployResult, String> {
    keytao_app_core::deploy::rime_deploy_default(&app.state::<Arc<Core>>(), ShellImeHost(app.clone()),
        #[cfg(target_os = "windows")]
        &app.state::<windows_app_actions::AppActions>(),
    ).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "ios")]
async fn rime_deploy_default(app: AppHandle) -> Result<DeployResult, String> {
    keytao_app_core::deploy::rime_deploy_default(&app.state::<Arc<Core>>(), ShellImeHost(app.clone()),
        #[cfg(target_os = "windows")]
        &app.state::<windows_app_actions::AppActions>(),
    ).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
)))]
async fn rime_deploy_default(_app: AppHandle) -> Result<DeployResult, String> {
    keytao_app_core::deploy::rime_deploy_default(&_app.state::<Arc<Core>>(), ShellImeHost(_app.clone()),
        #[cfg(target_os = "windows")]
        &_app.state::<windows_app_actions::AppActions>(),
    ).await.map_err(|e| e.to_string())
}


#[tauri::command]
async fn get_android_ime_input_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<AndroidImeInputSettings, String> {
    keytao_app_core::ime_settings::get_android_ime_input_settings(ShellImeHost(app)).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_android_ime_input_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    haptics_enabled: bool,
    haptic_intensity: u8,
    enter_key_behavior: String,
    key_preview_enabled: bool,
    long_press_delay_ms: u16,
    keyboard_height_scale: u8,
    delete_speed: String,
    english_mode: String,
    backspace_gesture_mode: String,
    keyboard_height_dp: u16,
    candidate_bar_height_dp: u8,
    swipe_threshold_dp: f32,
    key_sound_enabled: bool,
    key_sound_volume: u8,
    key_hint_visible: bool,
    flick_keys_enabled: bool,
    number_row_enabled: bool,
    candidate_font_scale: f32,
    double_space_period_enabled: bool,
    floating_portrait_enabled: bool,
    floating_portrait_scale: u8,
    floating_landscape_enabled: bool,
    floating_landscape_scale: u8,
) -> Result<AndroidImeInputSettings, String> {
    keytao_app_core::ime_settings::set_android_ime_input_settings(ShellImeHost(app), haptics_enabled, haptic_intensity, enter_key_behavior, key_preview_enabled, long_press_delay_ms, keyboard_height_scale, delete_speed, english_mode, backspace_gesture_mode, keyboard_height_dp, candidate_bar_height_dp, swipe_threshold_dp, key_sound_enabled, key_sound_volume, key_hint_visible, flick_keys_enabled, number_row_enabled, candidate_font_scale, double_space_period_enabled, floating_portrait_enabled, floating_portrait_scale, floating_landscape_enabled, floating_landscape_scale).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn get_desktop_english_mode(app: AppHandle) -> Result<keytao_core::english_mode::EnglishMode, String> {
    keytao_app_core::ime_settings::get_desktop_english_mode(ShellImeHost(app)).map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_desktop_english_mode(
    app: AppHandle,
    mode: keytao_core::english_mode::EnglishMode,
) -> Result<keytao_core::english_mode::EnglishMode, String> {
    keytao_app_core::ime_settings::set_desktop_english_mode(ShellImeHost(app), mode).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn get_ime_ui_settings() -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::get_ime_ui_settings().map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "android")]
fn get_ime_ui_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::get_ime_ui_settings(ShellImeHost(app)).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "ios")]
fn get_ime_ui_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::get_ime_ui_settings(ShellImeHost(app)).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn set_ime_embedded_composition(embedded: bool) -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::set_ime_embedded_composition(embedded).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn set_ime_ui_settings(
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
    font_size: f32,
) -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::set_ime_ui_settings(color_scheme, orientation, accent_color, font_size).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "android")]
fn set_ime_ui_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
    font_size: f32,
) -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::set_ime_ui_settings(ShellImeHost(app), color_scheme, orientation, accent_color, font_size).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "ios")]
fn set_ime_ui_settings(
    app: tauri::AppHandle,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
    font_size: f32,
) -> Result<ImeUiSettings, String> {
    keytao_app_core::ime_settings::set_ime_ui_settings(ShellImeHost(app), color_scheme, orientation, accent_color, font_size).map_err(|e| e.to_string())
}


// ─── Install schemas to default keytao data dir ───────────────────────────────

/// Download the given zip URL and smart-install it to `~/Library/keytao`
/// (or the platform-equivalent). Returns the same InstallResult as smart_install.
#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn rime_install_to_default(app: AppHandle, url: String) -> Result<InstallResult, String> {
    keytao_app_core::install::rime_install_to_default(&app.state::<Arc<Core>>(), url,
        #[cfg(target_os = "windows")] finish_windows_scheme_install,
    ).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "android")]
async fn rime_install_to_default<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    url: String,
) -> Result<InstallResult, String> {
    let permission_value: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin("storagePermissionStatus", ())
        .map_err(|e| e.to_string())?;
    let permission: AndroidStoragePermissionStatus =
        serde_json::from_value(permission_value).map_err(|e| e.to_string())?;
    if !permission.granted {
        return Err("需要文件访问权限".into());
    }

    let root = android_keytao_root(&app)?;
    let core = app.state::<Arc<Core>>();
    let temp = keytao_app_core::install::prepare_android_install(&core, &root, url).await.map_err(|e| e.to_string())?;
    let result: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin(
            "smartExtractZipToPrivate",
            serde_json::json!({ "zipPath": temp }),
        )
        .map_err(|e| e.to_string())?;
    keytao_app_core::install::finish_android_install(&core, &result).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "ios")]
async fn rime_install_to_default(app: AppHandle, url: String) -> Result<InstallResult, String> {
    let dest = ios_keytao_root(&app)?;
    keytao_app_core::install::rime_install_to_default(&app.state::<Arc<Core>>(), url, dest).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(target_os = "linux")]
fn linux_ime_status(app: AppHandle) -> LinuxImeStatus {
    keytao_app_core::ime_status::linux::linux_ime_status(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone()))
}

#[tauri::command]
async fn get_runtime_log_settings<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<RuntimeLogSettings, String> {
    keytao_app_core::logs::get_runtime_log_settings(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone())).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_runtime_log_settings<R: tauri::Runtime>(app: tauri::AppHandle<R>, enabled: bool, level: RuntimeLogLevel) -> Result<(), String> {
    keytao_app_core::logs::set_runtime_log_settings(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone()), enabled, level).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_runtime_log<R: tauri::Runtime>(app: tauri::AppHandle<R>, max_lines: Option<usize>) -> Result<DebugLogFile, String> {
    keytao_app_core::logs::read_runtime_log(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone()), max_lines).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn clear_runtime_log<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    keytao_app_core::logs::clear_runtime_log(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone())).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn share_runtime_log<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<serde_json::Value, String> {
    keytao_app_core::logs::share_runtime_log(&app.state::<Arc<Core>>(), &ShellImeHost(app.clone()), |request| log_shell::share(&app, request)).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_debug_logs(core: tauri::State<'_, Arc<Core>>) -> Result<DebugLogs, String> {
    keytao_app_core::logs::read_debug_logs(&core).await.map_err(|e| e.to_string())
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "windows")]
    let builder = windows_app_actions::configure(builder);
    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(scoped_storage_plugin());

    #[cfg(not(target_os = "linux"))]
    let builder = builder.setup(|app| {
        app_core::manage_core(app.handle())?;
        #[cfg(target_os = "windows")]
        windows_app_actions::show_main_window(app.handle());
        #[cfg(target_os = "android")]
        {
            // The JNI engine initialization only covers the IME/deploy processes.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let root = android_keytao_root(&handle).ok();
                if let Err(error) = handle.state::<Arc<Core>>().initialize_onboarding(root.clone()) {
                    tracing::warn!("Failed to initialize onboarding: {error}");
                }
                if let Some(root) = root {
                    keytao_core::runtime_log::init(&root, "android-app");
                }
            });
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            if let Some(root) = keytao_core::default_user_data_dir() {
                keytao_core::runtime_log::init(&root, "desktop-app");
            }
        }
        Ok(())
    });

    #[cfg(target_os = "linux")]
    let builder = builder.manage(rime::RimeEngine::default());

    #[cfg(target_os = "linux")]
    let builder = builder.manage(ManagedImeHelper::default());

    #[cfg(target_os = "linux")]
    let builder = builder
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            app_core::manage_core(app.handle())?;
            if let Some(root) = keytao_core::default_user_data_dir() {
                keytao_core::runtime_log::init(&root, "desktop-app");
            }
            // Linux tray is handled by keytao-ime daemon now.

            // Ensure the single Linux IME daemon owns Wayland, XIM, and IBus frontends.
            match launch_keytao_ime(app.handle(), false) {
                Ok(status) => tracing::info!("{}", status.message),
                Err(e) => tracing::warn!("{e}"),
            }

            Ok(())
        });

    builder
        .invoke_handler(tauri::generate_handler![
            check_app_update,
            fetch_latest_release,
            fetch_scheme_release,
            keytao_login,
            keytao_me,
            app_state_import_legacy,
            app_state_clear_auth,
            app_state_complete_onboarding,
            app_state_onboarding,
            sync_user_dictionary,
            get_component_versions,
            select_directory,
            download_to_temp,
            list_dir,
            read_local_schemas,
            smart_install,
            macos_ime_status,
            rime_install_to_default,
            android_ime_status,
            android_storage_permission_status,
            android_open_storage_permission_settings,
            android_keytao_data_dir,
            get_android_ime_input_settings,
            set_android_ime_input_settings,
            android_open_input_method_settings,
            android_show_input_method_picker,
            android_pick_directory,
            android_list_files,
            android_read_local_schemas,
            android_smart_extract,
            check_local_schema,
            addon_schema_status,
            addon_schema_install,
            wanxiang_status,
            manage_wanxiang,
            get_desktop_english_mode,
            set_desktop_english_mode,
            addon_schema_uninstall,
            rime_deploy_default,
            get_ime_ui_settings,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            set_ime_embedded_composition,
            set_ime_ui_settings,
            read_debug_logs,
            get_runtime_log_settings,
            set_runtime_log_settings,
            read_runtime_log,
            clear_runtime_log,
            share_runtime_log,
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            rime_get_data_dir,
            #[cfg(target_os = "linux")]
            linux_ime_status,
            #[cfg(target_os = "windows")]
            windows_ime_status,
            #[cfg(target_os = "windows")]
            windows_ime_ensure_registered,
            #[cfg(target_os = "windows")]
            windows_prepare_search_schemas,
            #[cfg(target_os = "windows")]
            windows_app_actions::windows_pending_ime_action,
            #[cfg(target_os = "windows")]
            windows_app_actions::windows_dismiss_ime_action,
            #[cfg(target_os = "windows")]
            windows_app_actions::windows_redeploy_ime_action,
            #[cfg(target_os = "linux")]
            rime::rime_get_data_dir,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(target_os = "linux")]
            if matches!(event, tauri::RunEvent::Exit) {
                stop_managed_ime_helper(app);
            }
        });
}

#[cfg(target_os = "windows")]
fn build_client<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<reqwest::Client, String> {
    keytao_app_core::scheme::build_client(&app.state::<Arc<Core>>())
}

#[tauri::command]
async fn fetch_latest_release(app: AppHandle) -> Result<ReleaseInfo, String> {
    keytao_app_core::scheme::fetch_latest_release(&app.state::<Arc<Core>>())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn fetch_scheme_release(app: AppHandle, scheme: String) -> Result<SchemeReleaseInfo, String> {
    keytao_app_core::scheme::fetch_scheme_release(&app.state::<Arc<Core>>(), scheme)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_dir(core: tauri::State<'_, Arc<Core>>, path: String) -> Result<Vec<FileItem>, String> {
    keytao_app_core::scheme::list_dir(&core, path).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_local_schemas(core: tauri::State<'_, Arc<Core>>, path: String) -> Vec<String> {
    keytao_app_core::scheme::read_local_schemas(&core, path).expect("schema reads are infallible")
}

#[tauri::command]
async fn download_to_temp(
    core: tauri::State<'_, Arc<Core>>,
    url: String,
) -> Result<String, String> {
    keytao_app_core::download::download_to_temp(&core, url)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn smart_install(
    core: tauri::State<'_, Arc<Core>>,
    zip_path: String,
    dest_path: String,
) -> Result<InstallResult, String> {
    keytao_app_core::install::smart_install(
        &core,
        zip_path,
        dest_path,
        #[cfg(target_os = "windows")]
        finish_windows_scheme_install,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_app_update(core: tauri::State<'_, Arc<Core>>) -> Result<AppUpdateInfo, String> {
    keytao_app_core::update::check_app_update(&core)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn addon_schema_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<AddonSchemaStatus, String> {
    keytao_app_core::addon::addon_schema_status(
        &app.state::<Arc<Core>>(),
        || default_keytao_user_root(&app),
        id,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn addon_schema_install(app: AppHandle, id: String) -> Result<AddonSchemaStatus, String> {
    keytao_app_core::addon::addon_schema_install(
        &app.state::<Arc<Core>>(),
        id,
        &ShellSchemeHost(&app),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn addon_schema_uninstall(app: AppHandle, id: String) -> Result<AddonSchemaStatus, String> {
    keytao_app_core::addon::addon_schema_uninstall(
        &app.state::<Arc<Core>>(),
        id,
        &ShellSchemeHost(&app),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn manage_wanxiang(app: AppHandle, installed: bool) -> Result<wanxiang::Status, String> {
    wanxiang::manage_wanxiang(&app.state::<Arc<Core>>(), installed, &ShellSchemeHost(&app))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn check_local_schema<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: Option<String>,
) -> LocalSchemaInfo {
    let dir: Option<PathBuf> = path.map(PathBuf::from).or_else(|| {
        #[cfg(target_os = "android")]
        {
            return android_keytao_root(&app).ok();
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            return keytao_core::default_user_data_dir();
        }
        #[cfg(target_os = "ios")]
        {
            return ios_keytao_root(&app).ok();
        }
    });

    keytao_app_core::scheme::check_local_schema(&app.state::<Arc<Core>>(), dir)
        .expect("schema inspection is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;



    // ── JNI export manifest ───────────────────────────────────────────────────

    /// Every `.rs` file of the shell and app core, so moving an export across
    /// the crate boundary cannot slip it past the manifest test below.
    fn crate_source_files() -> Vec<PathBuf> {
        fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, files);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    files.push(path);
                }
            }
        }

        let mut files = Vec::new();
        collect(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        collect(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/keytao-app-core/src"),
            &mut files,
        );
        files.sort();
        assert!(!files.is_empty(), "no crate sources found");
        files
    }

    /// Every `Java_*` export must funnel through `android_jni_guard`: unwinding
    /// out of an `extern "system"` function aborts the `:ime` process, so a
    /// panic in any export would take the keyboard down mid-typing.
    ///
    /// The exports are compiled out on the host, so the manifest is read off the
    /// sources: an export's body has to *open* with an
    /// `android_jni_guard("<method name>", …)` call, since that is the only
    /// shape in which the guard covers the whole body.
    ///
    /// Returns `(guarded, unguarded)` method names for the given sources.
    fn scan_jni_export_guards(sources: &[(String, String)]) -> (Vec<String>, Vec<String>) {
        // Built at run time so this scanner never matches its own source.
        let declaration = format!("fn {}_", "Java");

        let mut guarded: Vec<String> = Vec::new();
        let mut unguarded: Vec<String> = Vec::new();

        for (label, text) in sources {
            let lines: Vec<&str> = text.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                // Matched anywhere in the line rather than as a prefix, so an
                // indented, `unsafe`, or attribute-prefixed export cannot slip
                // past the scanner unnoticed.
                let Some(offset) = line.find(&declaration) else {
                    continue;
                };
                // `Java_<package>_<class>_<method>`, with `_1` standing in for
                // the underscores inside the Java identifiers.
                let symbol = line[offset + "fn ".len()..].trim();
                let symbol = symbol.split('(').next().unwrap_or(symbol).trim();
                let name = symbol.rsplit('_').next().unwrap_or(symbol);

                let mut cursor = index;
                while cursor < lines.len() && !lines[cursor].trim_end().ends_with('{') {
                    cursor += 1;
                }
                let body_start = lines
                    .get(cursor + 1..)
                    .unwrap_or_default()
                    .iter()
                    .take(4)
                    .copied()
                    .collect::<String>();
                let opens_with_guard = body_start.trim_start()
                    .strip_prefix("android_jni_guard")
                    .and_then(|rest| rest.trim_start().strip_prefix('('))
                    .is_some_and(|rest| rest.trim_start().starts_with(&format!("\"{name}\"")));
                if opens_with_guard {
                    guarded.push(name.to_owned());
                } else {
                    unguarded.push(format!("{label}: {name}"));
                }
            }
        }

        (guarded, unguarded)
    }

    #[test]
    fn every_jni_export_is_panic_guarded() {
        let sources: Vec<(String, String)> = crate_source_files()
            .into_iter()
            .map(|source| {
                let text = std::fs::read_to_string(&source)
                    .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
                (source.display().to_string(), text)
            })
            .collect();

        let (guarded, unguarded) = scan_jni_export_guards(&sources);

        assert!(
            unguarded.is_empty(),
            "JNI exports whose body does not open with android_jni_guard: {unguarded:?}"
        );
        // A stale scanner would otherwise report an empty manifest as a pass.
        for expected in ["nativeEngineAvailable", "nativeProcessKey", "nativeInit"] {
            assert!(
                guarded.iter().any(|name| name == expected),
                "scanner did not find `{expected}`; it no longer matches the exports"
            );
        }
    }

    /// The scanner has to fail on the shapes it exists to catch, including the
    /// ones a plain prefix match would walk past.
    ///
    /// The fixtures are assembled at run time for the same reason the scanner
    /// builds its needle that way: a literal export declaration in this file
    /// would be picked up by the crate-wide scan above.
    #[test]
    fn jni_export_scanner_rejects_unguarded_shapes() {
        let signature = format!(
            "pub extern \"system\" fn {}_a_B_nativeThing(env: JNIEnv<'_>) -> jboolean {{",
            "Java"
        );
        let unsafe_signature = format!(
            "    pub unsafe extern \"system\" fn {}_a_B_nativeThing(env: JNIEnv<'_>) -> jboolean {{",
            "Java"
        );
        let plain = format!("#[no_mangle]\n{signature}\n    1\n}}\n");
        let indented =
            format!("mod inner {{\n    #[no_mangle]\n{unsafe_signature}\n        1\n    }}\n}}\n");
        let guarded = format!(
            "#[no_mangle]\n{signature}\n    android_jni_guard(\"nativeThing\", 0, || 1)\n}}\n"
        );

        for (label, source) in [("plain", plain), ("indented", indented)] {
            let (_, unguarded) = scan_jni_export_guards(&[(label.into(), source)]);
            assert_eq!(
                unguarded,
                vec![format!("{label}: nativeThing")],
                "{label} export was not reported"
            );
        }

        let (found, unguarded) = scan_jni_export_guards(&[("guarded".into(), guarded)]);
        assert!(unguarded.is_empty());
        assert_eq!(found, vec!["nativeThing".to_owned()]);
        let multiline = format!(
            "{signature}\n    android_jni_guard(\n        \"nativeThing\",\n        0, || 1)\n}}\n"
        );
        let (found, unguarded) = scan_jni_export_guards(&[("multiline".into(), multiline)]);
        assert!(unguarded.is_empty());
        assert_eq!(found, vec!["nativeThing".to_owned()]);
    }
}
