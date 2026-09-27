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

#[cfg(target_os = "android")]
use jni::{
    objects::{JObject, JString},
    sys::{jboolean, jint, jlong, jstring},
    JNIEnv,
};

#[cfg(any(target_os = "linux", target_os = "android"))]
use std::sync::Mutex;


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

#[cfg(target_os = "android")]
fn optional_jni_path(env: &mut JNIEnv<'_>, value: JString<'_>) -> Option<PathBuf> {
    if value.is_null() {
        return None;
    }
    let value = env.get_string(&value).ok()?;
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(target_os = "android")]
fn optional_jni_text(env: &mut JNIEnv<'_>, value: JString<'_>) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let value = env.get_string(&value).ok()?;
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Like [`optional_jni_text`] but keeps the text as typed: a soft keyboard's
/// space key must stay a space instead of being trimmed away.
#[cfg(target_os = "android")]
fn optional_jni_raw_text(env: &mut JNIEnv<'_>, value: JString<'_>) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let value = env.get_string(&value).ok()?;
    Some(value.to_string_lossy().into_owned())
}

#[cfg(target_os = "android")]
fn optional_jni_effective_color_scheme(
    env: &mut JNIEnv<'_>,
    value: JString<'_>,
) -> Option<keytao_theme::EffectiveColorScheme> {
    if value.is_null() {
        return None;
    }
    let value = env.get_string(&value).ok()?;
    match value.to_string_lossy().trim().to_ascii_lowercase().as_str() {
        "dark" | "night" => Some(keytao_theme::EffectiveColorScheme::Dark),
        "light" | "day" => Some(keytao_theme::EffectiveColorScheme::Light),
        _ => None,
    }
}

#[cfg(target_os = "android")]
fn jni_string(env: &mut JNIEnv<'_>, value: &str) -> jstring {
    env.new_string(value)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(target_os = "android")]
mod android_log {
    use std::ffi::CString;
    use std::os::raw::c_char;

    const ANDROID_LOG_ERROR: i32 = 6;
    const TAG: &str = "KeytaoNative";

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
    }

    pub fn error(message: &str) {
        let (Ok(tag), Ok(text)) = (CString::new(TAG), CString::new(message)) else {
            return;
        };
        unsafe { __android_log_write(ANDROID_LOG_ERROR, tag.as_ptr(), text.as_ptr()) };
    }
}

/// Run `body` and turn a panic into `default`.
///
/// Unwinding out of an `extern "system"` function aborts the process, and for
/// the `:ime` process that means the keyboard disappears mid-typing, so every
/// JNI export funnels through here.
#[cfg(target_os = "android")]
fn android_jni_guard<T>(name: &str, default: T, body: impl FnOnce() -> T) -> T {
    let started = (!matches!(
        name,
        "nativeLogEnabled" | "nativeLogEvent" | "nativeLogFlush"
    )
        && keytao_core::runtime_log::enabled(keytao_core::runtime_log::Level::Verbose))
    .then(std::time::Instant::now);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(value) => {
            if let Some(started) = started {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    keytao_core::rt_log!(
                        keytao_core::runtime_log::Level::Verbose,
                        "rime",
                        "jni",
                        dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                        fn = name
                    );
                }));
            }
            value
        }
        Err(payload) => {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let message = if let Some(message) = payload.downcast_ref::<&'static str>() {
                    message
                } else if let Some(message) = payload.downcast_ref::<String>() {
                    message.as_str()
                } else {
                    "unknown panic"
                };
                keytao_core::rt_log!(
                    keytao_core::runtime_log::Level::Info,
                    "error",
                    "jni_panic",
                    fn = name,
                    msg = message
                );
                android_log::error(&format!("{name}: panicked: {message}"));
            }));
            default
        }
    }
}

#[cfg(target_os = "android")]
static ANDROID_IME_RUNTIME: Mutex<Option<keytao_core::ImeRuntime>> = Mutex::new(None);

/// Directories the live runtime was built for, so a reinitialize can tell
/// whether it may reuse it instead of tearing librime down behind its back.
#[cfg(target_os = "android")]
static ANDROID_IME_RUNTIME_DIRS: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

#[cfg(target_os = "android")]
static ANDROID_IME_USER_THEME_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(target_os = "android")]
struct AndroidThemeResolverState {
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    system_scheme: keytao_theme::EffectiveColorScheme,
    resolver: keytao_theme::ThemeResolver,
}

#[cfg(target_os = "android")]
impl AndroidThemeResolverState {
    fn new(
        default_theme_path: Option<PathBuf>,
        user_theme_path: Option<PathBuf>,
        system_scheme: keytao_theme::EffectiveColorScheme,
    ) -> Self {
        Self {
            resolver: keytao_theme::ThemeResolver::with_system_scheme(
                default_theme_path.clone(),
                user_theme_path.clone(),
                Some(system_scheme),
            ),
            default_theme_path,
            user_theme_path,
            system_scheme,
        }
    }

    fn matches(
        &self,
        default_theme_path: &Option<PathBuf>,
        user_theme_path: &Option<PathBuf>,
        system_scheme: keytao_theme::EffectiveColorScheme,
    ) -> bool {
        self.default_theme_path == *default_theme_path
            && self.user_theme_path == *user_theme_path
            && self.system_scheme == system_scheme
    }
}

#[cfg(target_os = "android")]
static ANDROID_IME_SYSTEM_COLOR_SCHEME: Mutex<keytao_theme::EffectiveColorScheme> =
    Mutex::new(keytao_theme::EffectiveColorScheme::Light);

#[cfg(target_os = "android")]
static ANDROID_IME_THEME_RESOLVER: Mutex<Option<AndroidThemeResolverState>> = Mutex::new(None);

#[cfg(target_os = "android")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AndroidImeStateJson {
    preedit: String,
    cursor: usize,
    sel_start: usize,
    sel_end: usize,
    candidates: Vec<keytao_core::Candidate>,
    highlighted_candidate_index: usize,
    page_size: usize,
    page: usize,
    is_last_page: bool,
    committed: String,
    select_keys: String,
    ascii_mode: bool,
    schema_name: String,
    accepted: bool,
    candidate_panel: keytao_theme::CandidatePanelModel,
    mode_hint: keytao_theme::ModeHintModel,
}

#[cfg(target_os = "android")]
fn android_state_json(state: keytao_core::ImeState, accepted: bool) -> String {
    let theme = android_current_theme();
    let mut ui_capabilities = keytao_theme::UiCapabilities::full_custom();
    ui_capabilities.supports_vertical = false;
    let candidate_panel = theme.candidate_panel_model(
        keytao_theme::CandidatePanelInput {
            preedit: state.preedit.clone(),
            candidates: state
                .candidates
                .iter()
                .map(|candidate| keytao_theme::ThemeCandidate {
                    text: candidate.text.clone(),
                    comment: candidate.comment.clone(),
                })
                .collect(),
            highlighted_candidate_index: state.highlighted_candidate_index,
            page: state.page,
            is_last_page: state.is_last_page,
            select_keys: state.select_keys.clone(),
        },
        &ui_capabilities,
    );
    let mode_hint = theme.mode_hint_model(state.ascii_mode);
    let value = AndroidImeStateJson {
        preedit: state.preedit,
        cursor: state.cursor,
        sel_start: state.sel_start,
        sel_end: state.sel_end,
        candidates: state.candidates,
        highlighted_candidate_index: state.highlighted_candidate_index,
        page_size: state.page_size,
        page: state.page,
        is_last_page: state.is_last_page,
        committed: state.committed.unwrap_or_default(),
        select_keys: state.select_keys.unwrap_or_default(),
        ascii_mode: state.ascii_mode,
        schema_name: state.schema_name,
        accepted,
        candidate_panel,
        mode_hint,
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".into())
}

#[cfg(target_os = "android")]
fn android_candidates_json(candidates: Vec<keytao_core::Candidate>) -> String {
    serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".into())
}

#[cfg(target_os = "android")]
fn android_current_theme() -> keytao_theme::ResolvedImeTheme {
    let user_path = ANDROID_IME_USER_THEME_PATH
        .lock()
        .ok()
        .and_then(|path| path.clone())
        .filter(|path| path.is_file());
    let system_scheme = ANDROID_IME_SYSTEM_COLOR_SCHEME
        .lock()
        .map(|scheme| *scheme)
        .unwrap_or(keytao_theme::EffectiveColorScheme::Light);
    android_cached_theme(None, user_path, system_scheme)
}

#[cfg(target_os = "android")]
fn android_cached_theme(
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    system_scheme: keytao_theme::EffectiveColorScheme,
) -> keytao_theme::ResolvedImeTheme {
    let Ok(mut slot) = ANDROID_IME_THEME_RESOLVER.lock() else {
        return keytao_theme::resolve_theme_from_paths_with_system_scheme(
            default_theme_path.as_deref(),
            user_theme_path.as_deref(),
            system_scheme,
        );
    };
    let needs_resolver = slot
        .as_ref()
        .map(|state| !state.matches(&default_theme_path, &user_theme_path, system_scheme))
        .unwrap_or(true);
    if needs_resolver {
        *slot = Some(AndroidThemeResolverState::new(
            default_theme_path,
            user_theme_path,
            system_scheme,
        ));
    }
    slot.as_ref()
        .map(|state| state.resolver.current())
        .unwrap_or_default()
}

#[cfg(target_os = "android")]
fn android_result_json(result: keytao_core::KeyProcessResult) -> String {
    android_state_json(result.state, result.accepted)
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

/// Point the cached theme at this user directory and drop the resolver built
/// for the previous one.
#[cfg(target_os = "android")]
fn android_set_user_theme_path(path: PathBuf) {
    if let Ok(mut theme_path) = ANDROID_IME_USER_THEME_PATH.lock() {
        *theme_path = Some(path);
    }
    if let Ok(mut resolver) = ANDROID_IME_THEME_RESOLVER.lock() {
        *resolver = None;
    }
}

#[cfg(target_os = "android")]
fn android_install_runtime(runtime: keytao_core::ImeRuntime, user_dir: &Path, shared_dir: &str) {
    if let Ok(mut dirs) = ANDROID_IME_RUNTIME_DIRS.lock() {
        *dirs = Some((user_dir.to_path_buf(), shared_dir.to_string()));
    }
    if let Ok(mut slot) = ANDROID_IME_RUNTIME.lock() {
        *slot = Some(runtime);
    }
}

/// The live runtime when it was built for these directories. Reloading through
/// it makes every session it handed out drop its engine before librime is
/// finalized, so a session the IME still holds cannot keep the previous
/// deployment's config and dictionaries alive.
#[cfg(target_os = "android")]
fn android_runtime_for(user_dir: &Path, shared_dir: &str) -> Option<keytao_core::ImeRuntime> {
    let matches = ANDROID_IME_RUNTIME_DIRS.lock().ok().is_some_and(|dirs| {
        dirs.as_ref().is_some_and(|(known_user, known_shared)| {
            known_user == user_dir && known_shared == shared_dir
        })
    });
    if !matches {
        return None;
    }
    ANDROID_IME_RUNTIME.lock().ok()?.as_ref().cloned()
}

#[cfg(target_os = "android")]
fn android_session<'a>(session: jlong) -> Option<&'a keytao_core::ImeRuntimeSession> {
    if session == 0 {
        return None;
    }
    Some(unsafe { &*(session as *mut keytao_core::ImeRuntimeSession) })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeLogEnabled(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    level: jint,
) -> jboolean {
    android_jni_guard("nativeLogEnabled", 0, || {
        let level = match level {
            1 => keytao_core::runtime_log::Level::Info,
            2 => keytao_core::runtime_log::Level::Verbose,
            _ => return 0,
        };
        keytao_core::runtime_log::enabled(level) as jboolean
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeLogEvent(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    level: jint,
    cat: JString<'_>,
    ev: JString<'_>,
    dur_ms: jni::sys::jdouble,
    kv_json: JString<'_>,
) {
    android_jni_guard("nativeLogEvent", (), || {
        let level = match level {
            1 => keytao_core::runtime_log::Level::Info,
            2 => keytao_core::runtime_log::Level::Verbose,
            _ => return,
        };
        if !keytao_core::runtime_log::enabled(level) {
            return;
        }
        let Some(cat) = optional_jni_text(&mut env, cat) else {
            return;
        };
        let Some(ev) = optional_jni_text(&mut env, ev) else {
            return;
        };
        let kv = if kv_json.is_null() {
            serde_json::Map::new()
        } else {
            let Some(json) = optional_jni_raw_text(&mut env, kv_json) else {
                return;
            };
            let Ok(kv) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
            else {
                return;
            };
            kv
        };
        keytao_core::runtime_log::log_event(
            level,
            &cat,
            &ev,
            (!dur_ms.is_nan()).then_some(dur_ms),
            kv,
        );
    });
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeLogFlush(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    timeout_ms: jint,
) {
    android_jni_guard("nativeLogFlush", (), || {
        keytao_core::runtime_log::flush_blocking(std::time::Duration::from_millis(
            timeout_ms.max(0) as u64,
        ));
    });
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeResolveThemeJson(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    default_theme_path: JString<'_>,
    user_theme_path: JString<'_>,
    system_color_scheme: JString<'_>,
) -> jstring {
    android_jni_guard("nativeResolveThemeJson", std::ptr::null_mut(), || {
        let default_path = optional_jni_path(&mut env, default_theme_path);
        let user_path = optional_jni_path(&mut env, user_theme_path);
        let system_scheme = optional_jni_effective_color_scheme(&mut env, system_color_scheme)
            .unwrap_or(keytao_theme::EffectiveColorScheme::Light);
        if let Ok(mut current) = ANDROID_IME_SYSTEM_COLOR_SCHEME.lock() {
            *current = system_scheme;
        }
        let theme = android_cached_theme(default_path, user_path, system_scheme);
        match keytao_theme::resolved_theme_json(&theme) {
            Ok(json) => jni_string(&mut env, &json),
            Err(error) => jni_string(&mut env, &format!(r#"{{"error":"{error}"}}"#)),
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeWriteThemeUi(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    path: JString<'_>,
    color_scheme: JString<'_>,
    accent_hex: JString<'_>,
) -> jboolean {
    android_jni_guard("nativeWriteThemeUi", 0, || {
        let Some(path) = optional_jni_path(&mut env, path) else {
            return 0;
        };
        let Some(color_scheme) = optional_jni_text(&mut env, color_scheme) else {
            return 0;
        };
        let color_scheme = match color_scheme.to_ascii_lowercase().as_str() {
            "auto" => keytao_theme::UiColorScheme::Auto,
            "light" => keytao_theme::UiColorScheme::Light,
            "dark" => keytao_theme::UiColorScheme::Dark,
            _ => return 0,
        };
        let accent_hex = if accent_hex.is_null() {
            None
        } else {
            let Ok(accent_hex) = env.get_string(&accent_hex) else {
                return 0;
            };
            Some(accent_hex.to_string_lossy().trim().to_owned())
        };
        if keytao_theme::write_theme_ui_to_path(&path, color_scheme, accent_hex).is_ok() {
            if let Ok(mut resolver) = ANDROID_IME_THEME_RESOLVER.lock() {
                *resolver = None;
            }
            1
        } else {
            0
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDefaultKeyboardYaml(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    android_jni_guard("nativeDefaultKeyboardYaml", std::ptr::null_mut(), || {
        jni_string(&mut env, keytao_theme::default_keyboard_yaml())
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeResolveKeyboardJson(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    default_keyboard_path: JString<'_>,
    user_keyboard_path: JString<'_>,
) -> jstring {
    android_jni_guard("nativeResolveKeyboardJson", std::ptr::null_mut(), || {
        let default_path = optional_jni_path(&mut env, default_keyboard_path);
        let user_path = optional_jni_path(&mut env, user_keyboard_path);
        let keyboard = keytao_theme::resolve_keyboard_from_paths(
            default_path.as_deref(),
            user_path.as_deref(),
        );
        match keytao_theme::resolved_keyboard_json(&keyboard) {
            Ok(json) => jni_string(&mut env, &json),
            Err(error) => jni_string(&mut env, &format!(r#"{{"error":"{error}"}}"#)),
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeEngineAvailable(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jboolean {
    android_jni_guard("nativeEngineAvailable", 0, || 1)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDeployStep(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    user_dir: JString<'_>,
    _shared_dir: JString<'_>,
    schema_id: JString<'_>,
) -> jstring {
    android_jni_guard("nativeDeployStep", std::ptr::null_mut(), || {
        let Some(user_dir) = optional_jni_path(&mut env, user_dir) else {
            return jni_string(
                &mut env,
                r#"{"success":false,"error":"missing user directory"}"#,
            );
        };
        let shared_dir = user_dir.join("rime-data").to_string_lossy().into_owned();
        let schema_id = optional_jni_text(&mut env, schema_id);
        let result = match schema_id {
            Some(schema_id) => keytao_core::deploy_android_schema(
                user_dir.to_string_lossy().into_owned(),
                shared_dir,
                schema_id,
            ),
            None => keytao_core::deploy_android_config(
                user_dir.to_string_lossy().into_owned(),
                shared_dir,
            ),
        };
        let payload = match result {
            Ok(schemas) => serde_json::json!({ "success": true, "schemas": schemas }),
            Err(error) => serde_json::json!({ "success": false, "error": error }),
        };
        jni_string(&mut env, &payload.to_string())
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeInit(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    user_dir: JString<'_>,
    _shared_dir: JString<'_>,
    deploy: jboolean,
) -> jboolean {
    android_jni_guard("nativeInit", 0, || {
        let Some(user_dir) = optional_jni_path(&mut env, user_dir) else {
            return 0;
        };
        let user_theme_path = user_dir.join("theme.yaml");
        let shared_dir = user_dir.join("rime-data").to_string_lossy().into_owned();

        let runtime = keytao_core::ImeRuntime::with_dirs(user_dir.clone(), shared_dir.clone());
        let init_result = if deploy != 0 {
            runtime.init()
        } else {
            runtime.init_without_deploy()
        };
        if init_result.is_err() {
            return 0;
        }
        android_set_user_theme_path(user_theme_path);
        android_install_runtime(runtime, &user_dir, &shared_dir);
        1
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeReinitialize(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    user_dir: JString<'_>,
    _shared_dir: JString<'_>,
) -> jboolean {
    android_jni_guard("nativeReinitialize", 0, || {
        let Some(user_dir) = optional_jni_path(&mut env, user_dir) else {
            return 0;
        };
        let user_theme_path = user_dir.join("theme.yaml");
        let shared_dir = user_dir.join("rime-data").to_string_lossy().into_owned();

        // The runtime knows every session it handed out, so reloading through it
        // drops their engines before librime is finalized. Going around it would
        // leave a session the IME still holds pointing at a torn-down librime.
        if let Some(runtime) = android_runtime_for(&user_dir, &shared_dir) {
            if runtime.reload_without_deploy().is_ok() {
                android_set_user_theme_path(user_theme_path);
                return 1;
            }
        }

        if let Ok(mut slot) = ANDROID_IME_RUNTIME.lock() {
            *slot = None;
        }
        if keytao_core::reinitialize(user_dir.to_string_lossy().into_owned(), shared_dir.clone())
            .is_err()
        {
            return 0;
        }

        let runtime = keytao_core::ImeRuntime::with_dirs(user_dir.clone(), shared_dir.clone());
        if runtime.init_without_deploy().is_err() {
            return 0;
        }
        android_set_user_theme_path(user_theme_path);
        android_install_runtime(runtime, &user_dir, &shared_dir);
        1
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeCreateSession(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jlong {
    android_jni_guard("nativeCreateSession", 0, || {
        let Ok(slot) = ANDROID_IME_RUNTIME.lock() else {
            return 0;
        };
        let Some(runtime) = slot.as_ref().cloned() else {
            return 0;
        };
        drop(slot);
        match runtime.create_session() {
            Ok(session) => Box::into_raw(Box::new(session)) as jlong,
            Err(_) => 0,
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDestroySession(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) {
    android_jni_guard("nativeDestroySession", (), || {
        if session == 0 {
            return;
        }
        unsafe {
            drop(Box::from_raw(
                session as *mut keytao_core::ImeRuntimeSession,
            ));
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSessionState(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeSessionState", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(session.state(), false))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeProcessKey(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    keyval: jint,
    modifiers: jint,
) -> jstring {
    android_jni_guard("nativeProcessKey", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(result) = session.process_key_result(keyval as u32, modifiers as u32) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_result_json(result))
    })
}

/// `Return` handling shared by every frontend: librime decides, and only if it
/// passes the key back is the raw input committed.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeProcessEnter(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeProcessEnter", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(result) = session.process_enter() else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_result_json(result))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSelectCandidate(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jstring {
    android_jni_guard("nativeSelectCandidate", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.select_candidate_on_page(index.max(0) as usize) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

/// Move the highlight without committing, for candidate hover and navigation.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeHighlightCandidate(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jstring {
    android_jni_guard("nativeHighlightCandidate", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.highlight_candidate_on_page(index.max(0) as usize) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

/// Forget a learned phrase, the action behind "delete candidate" gestures.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDeleteCandidate(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jstring {
    android_jni_guard("nativeDeleteCandidate", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some((state, deleted)) = session.delete_candidate_on_page_result(index.max(0) as usize)
        else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, deleted))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeCandidateIsUserPhrase(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jboolean {
    android_jni_guard(
        "nativeCandidateIsUserPhrase",
        0,
        || match android_session(session)
            .and_then(|session| session.candidate_is_user_phrase_on_page(index.max(0) as usize))
        {
            Some(true) => 1,
            _ => 0,
        },
    )
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSelectCandidateGlobal(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jstring {
    android_jni_guard("nativeSelectCandidateGlobal", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.select_candidate_global(index.max(0) as usize) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeAllCandidates(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    limit: jint,
) -> jstring {
    android_jni_guard("nativeAllCandidates", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(candidates) = session.all_candidates_limited(limit.max(0) as usize) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_candidates_json(candidates))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeListSchemas(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeListSchemas", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(schemas) = session.list_schemas() else {
            return std::ptr::null_mut();
        };
        let Ok(json) = serde_json::to_string(&schemas) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &json)
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSchemaSwitches(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeSchemaSwitches", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(switches) = session.schema_switches() else {
            return std::ptr::null_mut();
        };
        let Ok(json) = serde_json::to_string(&switches) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &json)
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeCurrentSchema(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeCurrentSchema", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(schema) = session.current_schema() else {
            return std::ptr::null_mut();
        };
        let Ok(json) = serde_json::to_string(&schema) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &json)
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSelectSchema(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    schema_id: JString<'_>,
) -> jstring {
    android_jni_guard("nativeSelectSchema", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(schema_id) = optional_jni_text(&mut env, schema_id) else {
            return std::ptr::null_mut();
        };
        let Ok(state) = session.select_schema(&schema_id) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, false))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeGetOption(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    option_name: JString<'_>,
) -> jboolean {
    android_jni_guard("nativeGetOption", 0, || {
        let Some(session) = android_session(session) else {
            return 0;
        };
        let Some(option_name) = optional_jni_text(&mut env, option_name) else {
            return 0;
        };
        if session.get_option(&option_name) {
            1
        } else {
            0
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSetOption(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    option_name: JString<'_>,
    enabled: jboolean,
) -> jstring {
    android_jni_guard("nativeSetOption", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(option_name) = optional_jni_text(&mut env, option_name) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.set_option(&option_name, enabled != 0) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, false))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeChangePage(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    backward: jboolean,
) -> jstring {
    android_jni_guard("nativeChangePage", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.change_page(backward != 0) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeReset(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeReset", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.reset() else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

/// Commit what is being composed; the path to take when the input context ends
/// and the composition must not be lost.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeCommitComposition(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeCommitComposition", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.commit_composition() else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

/// Discard what is being composed; the path to take when the input context ends
/// and the composition must not reach the editor.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeClearComposition(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    android_jni_guard("nativeClearComposition", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.clear_composition() else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

/// Declare what the current editor allows. Password and PIN fields pass
/// `composing = false`, which stops keys from reaching librime at all, so no
/// preedit appears and nothing is learned. Returns the state to apply.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSetInputPolicy(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    composing: jboolean,
    learning: jboolean,
) -> jstring {
    android_jni_guard("nativeSetInputPolicy", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.set_input_policy(keytao_core::InputContextPolicy {
            composing: composing != 0,
            learning: learning != 0,
        }) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeInputPolicyComposing(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jboolean {
    android_jni_guard("nativeInputPolicyComposing", 0, || {
        match android_session(session) {
            Some(session) if session.input_policy().composing => 1,
            _ => 0,
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeInputPolicyLearning(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jboolean {
    android_jni_guard("nativeInputPolicyLearning", 0, || {
        match android_session(session) {
            Some(session) if session.input_policy().learning => 1,
            _ => 0,
        }
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeGetAsciiMode(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jboolean {
    android_jni_guard("nativeGetAsciiMode", 0, || match android_session(session) {
        Some(session) if session.is_ascii_mode() => 1,
        _ => 0,
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSetAsciiMode(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    enabled: jboolean,
) -> jstring {
    android_jni_guard("nativeSetAsciiMode", std::ptr::null_mut(), || {
        let Some(session) = android_session(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = session.set_ascii_mode(enabled != 0) else {
            return std::ptr::null_mut();
        };
        jni_string(&mut env, &android_state_json(state, true))
    })
}

/// The X11 keysym a soft keyboard key should send, or 0 when the text is not a
/// single typable character and has to be committed directly instead.
///
/// Latin-1 maps onto itself and everything else uses X11's `0x01000000 | cp`
/// encoding, so `（` (U+FF08) never arrives as `XK_BackSpace`.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeTextToKeysym(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    text: JString<'_>,
) -> jint {
    android_jni_guard("nativeTextToKeysym", 0, || {
        let Some(text) = optional_jni_raw_text(&mut env, text) else {
            return 0;
        };
        keytao_core::key_policy::keysym_for_text(&text).unwrap_or(0) as jint
    })
}

/// Whether a keysym is `Return` or keypad `Return`.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeIsEnterKey(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    keyval: jint,
) -> jboolean {
    android_jni_guard("nativeIsEnterKey", 0, || {
        if keytao_core::key_policy::is_enter_key(keyval as u32) {
            1
        } else {
            0
        }
    })
}

/// Whether a key must be handed straight to the editor instead of librime.
///
/// `ascii_mode` deliberately plays no part: English mode still needs librime's
/// `ascii_composer` to see the key, and Control/Alt chords carry Rime's own
/// switcher hotkeys, so only window-system modifiers pass through early.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeShouldBypassKey(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    keyval: jint,
    modifiers: jint,
) -> jboolean {
    android_jni_guard("nativeShouldBypassKey", 0, || {
        let Some(session) = android_session(session) else {
            return 0;
        };
        let state = session.state();
        if keytao_core::key_policy::should_bypass_empty_composition(
            keyval as u32,
            modifiers as u32,
            &state,
        ) {
            1
        } else {
            0
        }
    })
}

/// Map a Unicode scalar offset into `text` to a UTF-16 code unit offset, the
/// unit `InputConnection` counts in.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeUtf16OffsetFromChars(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    text: JString<'_>,
    char_offset: jint,
) -> jint {
    android_jni_guard("nativeUtf16OffsetFromChars", 0, || {
        let Some(text) = optional_jni_raw_text(&mut env, text) else {
            return 0;
        };
        keytao_core::utf16_offset_from_chars(&text, char_offset.max(0) as usize) as jint
    })
}

/// Signature of the reload signal in `user_dir`, or null when no deployment has
/// requested a reload yet. The format is keytao-core's; the IME must not invent
/// its own, otherwise the same deployment reloads on some platforms only.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeReloadStampSignature(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    user_dir: JString<'_>,
) -> jstring {
    android_jni_guard("nativeReloadStampSignature", std::ptr::null_mut(), || {
        let Some(user_dir) = optional_jni_path(&mut env, user_dir) else {
            return std::ptr::null_mut();
        };
        match keytao_core::ReloadStamp::current_signature(&user_dir) {
            Some(signature) => jni_string(&mut env, &signature),
            None => std::ptr::null_mut(),
        }
    })
}

/// Path of the reload signal inside `user_dir`.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeReloadStampPath(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    user_dir: JString<'_>,
) -> jstring {
    android_jni_guard("nativeReloadStampPath", std::ptr::null_mut(), || {
        let Some(user_dir) = optional_jni_path(&mut env, user_dir) else {
            return std::ptr::null_mut();
        };
        jni_string(
            &mut env,
            &keytao_core::ReloadStamp::path(&user_dir).to_string_lossy(),
        )
    })
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

    /// Every `.rs` file of this crate, so that moving an export into a new
    /// module cannot slip it past the manifest test below.
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
