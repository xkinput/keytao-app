//! JNI entry points shared by Android hosts loading `libkeytao_app_lib.so`.
//! The host owns the cdylib; this module owns the IME runtime and JNI boundary.

use jni::{
    objects::{JObject, JString},
    sys::{jboolean, jint, jlong, jstring},
    JNIEnv,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

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
    unsafe extern "C" {
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDefaultKeyboardYaml(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    android_jni_guard("nativeDefaultKeyboardYaml", std::ptr::null_mut(), || {
        jni_string(&mut env, keytao_theme::default_keyboard_yaml())
    })
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeEngineAvailable(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jboolean {
    android_jni_guard("nativeEngineAvailable", 0, || 1)
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
