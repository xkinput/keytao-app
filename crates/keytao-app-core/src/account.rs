use crate::{
    Core, CoreError,
    ime_host::ImeHost,
    ime_paths::path_string,
    scheme::{api_error_message, build_client},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "account_tests.rs"]
mod account_tests;

#[cfg(test)]
#[path = "account_sync_tests.rs"]
mod account_sync_tests;

#[derive(Serialize, Deserialize, Clone)]
pub struct AppAuthUser {
    pub id: i64,
    pub name: Option<String>,
    pub nickname: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AppAuthSession {
    pub token: String,
    pub user: AppAuthUser,
}

#[derive(Serialize, Clone)]
pub struct UserDictionarySyncResult {
    pub file_name: String,
    pub path: String,
    pub count: usize,
    pub updated_at: String,
    pub import_table_patched: bool,
    pub reload_stamp_path: Option<String>,
    pub message: String,
    pub pending: bool,
    pub local_deletions: Vec<UserDictionaryDeletion>,
    pub server_deletions: Vec<UserDictionaryDeletion>,
}

#[derive(Serialize, Clone)]
pub struct UserDictionaryDeletion {
    pub word: String,
    pub code: String,
    pub remove: bool,
}

pub struct UserDictionaryDeletionConfirmation {
    pub local_deletions: Vec<UserDictionaryDeletion>,
    pub server_deletions: Vec<UserDictionaryDeletion>,
}

pub(crate) const API_BASE: &str = "https://keytao.rea.ink";

async fn throttle(api_base: &str) {
    static REQUESTS: OnceLock<tokio::sync::Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let mut requests = REQUESTS.get_or_init(Default::default).lock().await;
    if let Some(previous) = requests.get(api_base) {
        tokio::time::sleep(Duration::from_millis(550).saturating_sub(previous.elapsed())).await;
    }
    requests.insert(api_base.into(), Instant::now());
}

pub(crate) struct ApiClient<'a> {
    core: &'a Core,
    api_base: &'a str,
    account_id: Option<i64>,
}

impl<'a> ApiClient<'a> {
    pub(crate) fn new(core: &'a Core, api_base: &'a str) -> Self {
        Self {
            core,
            api_base,
            account_id: account_id(core),
        }
    }

    fn token(&self) -> Result<String, CoreError> {
        let state = self.core.state();
        if !same_account(&state, self.account_id) {
            return Err(CoreError::Permission("请先登录 KeyTao 账号".into()));
        }
        state
            .auth_token
            .ok_or_else(|| CoreError::Permission("请先登录 KeyTao 账号".into()))
    }

    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, CoreError> {
        let client = build_client(self.core).map_err(CoreError::Other)?;
        for attempt in 0..2 {
            let token = self.token()?;
            throttle(self.api_base).await;
            self.token()?;
            let mut request = client
                .request(method.clone(), format!("{}{path}", self.api_base))
                .bearer_auth(&token)
                .timeout(Duration::from_secs(15));
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|error| CoreError::Network(error.to_string()))?;
            let status = response.status();
            self.token()?;
            if status == reqwest::StatusCode::UNAUTHORIZED {
                if attempt == 0 {
                    self.refresh(&token).await?;
                    continue;
                }
                clear_token(self.core, &token)?;
                return Err(CoreError::Permission(
                    api_error_message(response, "请先登录 KeyTao 账号").await,
                ));
            }
            if status.is_success() {
                return response
                    .json()
                    .await
                    .map_err(|error| CoreError::Other(format!("解析响应失败: {error}")));
            }
            let value: Value = response
                .json()
                .await
                .map_err(|error| CoreError::Other(error.to_string()))?;
            if status == reqwest::StatusCode::BAD_REQUEST
                && value["requiresConfirmation"].as_bool() == Some(true)
            {
                return Ok(value);
            }
            let message = value["error"].as_str().unwrap_or("请求失败");
            return Err(CoreError::Other(format!(
                "HTTP {}: {message}",
                status.as_u16()
            )));
        }
        unreachable!()
    }

    pub(crate) async fn download_url(&self, path: &str) -> Result<String, CoreError> {
        let client = reqwest::Client::builder()
            .user_agent(format!("keytao-app/{}", self.core.env().app_version))
            .connect_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| CoreError::Other(error.to_string()))?;
        for attempt in 0..2 {
            let token = self.token()?;
            throttle(self.api_base).await;
            self.token()?;
            let response = client
                .get(format!("{}{path}", self.api_base))
                .bearer_auth(&token)
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .map_err(|error| CoreError::Network(error.to_string()))?;
            self.token()?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                if attempt == 0 {
                    self.refresh(&token).await?;
                    continue;
                }
                clear_token(self.core, &token)?;
                return Err(CoreError::Permission("请先登录 KeyTao 账号".into()));
            }
            if status == reqwest::StatusCode::FOUND {
                return response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
                    .ok_or_else(|| CoreError::Other("方案包地址无效".into()));
            }
            return Err(CoreError::Other(format!(
                "HTTP {}: {}",
                status.as_u16(),
                api_error_message(response, "方案包下载失败").await
            )));
        }
        unreachable!()
    }

    async fn refresh(&self, token: &str) -> Result<(), CoreError> {
        if token_timing(token).is_none_or(|timing| {
            timing.exp <= now_seconds() || timing.id <= 0 || Some(timing.id) != self.account_id
        }) {
            clear_token(self.core, token)?;
            return Err(CoreError::Permission("登录已过期，请重新登录".into()));
        }
        let result = async {
            let client = build_client(self.core).map_err(CoreError::Other)?;
            throttle(self.api_base).await;
            if self.token()? != token {
                return Err(CoreError::Permission("登录状态已更改".into()));
            }
            if token_timing(token).is_none_or(|timing| {
                timing.exp <= now_seconds() || timing.id <= 0 || Some(timing.id) != self.account_id
            }) {
                return Err(CoreError::Permission("登录已过期，请重新登录".into()));
            }
            let response = client
                .post(format!("{}/api/auth/refresh", self.api_base))
                .bearer_auth(token)
                .timeout(Duration::from_secs(12))
                .send()
                .await
                .map_err(|error| CoreError::Network(error.to_string()))?;
            let status = response.status();
            if !status.is_success() {
                return Err(CoreError::Other(format!(
                    "HTTP {}: {}",
                    status.as_u16(),
                    api_error_message(response, "刷新登录失败").await
                )));
            }
            let value: Value = response
                .json()
                .await
                .map_err(|error| CoreError::Other(format!("解析登录响应失败: {error}")))?;
            let refreshed = value["token"]
                .as_str()
                .filter(|token| {
                    token.len() <= 4096
                        && token_timing(token).is_some_and(|timing| {
                            timing.exp > now_seconds()
                                && timing.iat <= timing.exp
                                && timing.id > 0
                                && Some(timing.id) == self.account_id
                        })
                })
                .ok_or_else(|| CoreError::Other("刷新登录响应格式不正确".into()))?;
            self.core.update_state(|state| {
                if state.auth_token.as_deref() == Some(token) {
                    state.auth_token = Some(refreshed.into());
                }
            })?;
            self.token()?;
            Ok(())
        }
        .await;
        if result.is_err() {
            clear_token(self.core, token)?;
        }
        result
    }
}

fn clear_token(core: &Core, token: &str) -> Result<(), CoreError> {
    core.update_state(|state| {
        if state.auth_token.as_deref() == Some(token) {
            state.auth_token = None;
            state.user = None;
            state.legacy_imported = true;
        }
    })
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Deserialize)]
struct TokenTiming {
    id: i64,
    iat: u64,
    exp: u64,
}

fn token_timing(token: &str) -> Option<TokenTiming> {
    let payload = token.split('.').nth(1)?;
    let mut bytes = Vec::new();
    let mut bits = 0u32;
    let mut count = 0;
    for byte in payload.bytes().take(4097) {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn account_id(core: &Core) -> Option<i64> {
    core.state()
        .user
        .as_ref()?
        .get("id")?
        .as_i64()
        .filter(|id| *id > 0)
}

pub(crate) fn same_account(state: &crate::AppState, expected: Option<i64>) -> bool {
    expected.is_some_and(|id| id > 0)
        && state.user.as_ref().and_then(|user| user["id"].as_i64()) == expected
        && state
            .auth_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
}

pub async fn startup_refresh(core: &Core) -> Result<(), CoreError> {
    startup_refresh_at(core, API_BASE).await
}

async fn startup_refresh_at(core: &Core, api_base: &str) -> Result<(), CoreError> {
    let Some(token) = core.state().auth_token else {
        return Ok(());
    };
    let Some(timing) = token_timing(&token) else {
        return clear_token(core, &token);
    };
    let now = now_seconds();
    if timing.exp <= now {
        return clear_token(core, &token);
    }
    if now.saturating_sub(timing.iat) > 86400 {
        ApiClient::new(core, api_base).refresh(&token).await?;
    }
    Ok(())
}

fn legacy_base_path(core: &Core, account: i64) -> PathBuf {
    core.env()
        .data_dir
        .join(format!("user-dictionary-sync-{account}.json"))
}

fn base_path(root: &Path, account: i64) -> PathBuf {
    root.join(format!(".keytao-sync-base-{account}.json"))
}

#[derive(Serialize, Deserialize)]
struct DictionaryBase {
    instance: String,
    #[serde(deserialize_with = "deserialize_base_entries")]
    entries: BTreeSet<(String, String, bool)>,
}

fn deserialize_base_entries<'de, D>(
    deserializer: D,
) -> Result<BTreeSet<(String, String, bool)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Current((String, String, bool)),
        Legacy((String, String)),
    }
    Ok(Vec::<Entry>::deserialize(deserializer)?
        .into_iter()
        .map(|entry| match entry {
            Entry::Current(key) => key,
            Entry::Legacy((word, code)) => (word, code, false),
        })
        .collect())
}

const INSTANCE_PREFIX: &str = "# keytao-sync-instance: ";
pub(crate) const USER_DICTIONARY: &str = "keytao.user.dict.yaml";
pub(crate) const USER_WORD_REMOVALS: &str = "keytao.user.remove.txt";

fn dictionary_instance(content: &str) -> Option<&str> {
    let mut instances = content
        .lines()
        .filter_map(|line| line.strip_prefix(INSTANCE_PREFIX));
    let instance = instances.next()?.trim();
    (!instance.is_empty() && instances.next().is_none()).then_some(instance)
}

fn load_base(
    core: &Core,
    root: &Path,
    account: i64,
    content: &[u8],
    local: &BTreeSet<(String, String, bool)>,
) -> Result<BTreeSet<(String, String, bool)>, CoreError> {
    match std::fs::read(base_path(root, account)) {
        Ok(bytes) => {
            let saved: DictionaryBase = serde_json::from_slice(&bytes)?;
            let content = std::str::from_utf8(content)
                .map_err(|error| CoreError::Other(error.to_string()))?;
            return Ok(
                if dictionary_instance(content) == Some(saved.instance.as_str()) {
                    saved.entries
                } else {
                    BTreeSet::new()
                },
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let legacy = legacy_base_path(core, account);
    let created = std::fs::metadata(root.join("keytao.user.dict.yaml"))
        .and_then(|metadata| metadata.created())
        .ok();
    let saved = std::fs::metadata(&legacy)
        .and_then(|metadata| metadata.modified())
        .ok();
    // Unknown or newer dictionary instances must start with a union.
    if !created
        .zip(saved)
        .is_some_and(|(created, saved)| created <= saved)
    {
        return Ok(BTreeSet::new());
    }
    let base = deserialize_base_entries(&mut serde_json::Deserializer::from_slice(
        &std::fs::read(legacy)?,
    ))?;
    // Only legacy bases lack an instance marker proving that an empty file is an edit.
    Ok(if local.is_disjoint(&base) {
        BTreeSet::new()
    } else {
        base
    })
}

fn save_base(
    root: &Path,
    account: i64,
    entries: &BTreeSet<(String, String, bool)>,
) -> Result<(), CoreError> {
    let dictionary = root.join("keytao.user.dict.yaml");
    let content = std::fs::read_to_string(&dictionary)?;
    let instance = if let Some(instance) = dictionary_instance(&content) {
        instance.to_owned()
    } else {
        // A comment survives normal atomic edits but is absent from a recreated dictionary.
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        let instance = format!(
            "{}-{}",
            temporary.path().file_name().unwrap().to_string_lossy(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let mut content: String = content
            .split_inclusive('\n')
            .filter(|line| !line.starts_with(INSTANCE_PREFIX))
            .collect();
        if !content.ends_with('\n') {
            content.push('\n');
        }
        temporary.write_all(content.as_bytes())?;
        writeln!(temporary, "{INSTANCE_PREFIX}{instance}")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(dictionary)
            .map_err(|error| CoreError::Io(error.error))?;
        instance
    };
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    temporary.write_all(&serde_json::to_vec(&DictionaryBase {
        instance,
        entries: entries.clone(),
    })?)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(base_path(root, account))
        .map_err(|error| CoreError::Io(error.error))?;
    Ok(())
}

pub(crate) fn invalidate_dictionary_bases(core: &Core, root: &Path) -> Result<(), CoreError> {
    for (directory, prefix) in [
        (root, ".keytao-sync-base-"),
        (core.env().data_dir.as_path(), "user-dictionary-sync-"),
    ] {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_name().to_str().is_some_and(|name| {
                name.strip_prefix(prefix)
                    .and_then(|name| name.strip_suffix(".json"))
                    .and_then(|account| account.parse::<i64>().ok())
                    .is_some_and(|account| account > 0)
            }) {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct RemoteDictionary {
    entries: Vec<RemoteEntry>,
    total: usize,
}

#[derive(Deserialize)]
struct RemoteEntry {
    id: i64,
    #[serde(rename = "userId")]
    user_id: i64,
    word: String,
    code: String,
    #[serde(default)]
    remove: bool,
    weight: i64,
    #[serde(rename = "type")]
    kind: String,
}

pub async fn keytao_login(
    core: &Core,
    name: String,
    password: String,
) -> Result<AppAuthSession, CoreError> {
    keytao_login_at(core, name, password, API_BASE).await
}

async fn keytao_login_at(
    core: &Core,
    name: String,
    password: String,
    api_base: &str,
) -> Result<AppAuthSession, CoreError> {
    let result: Result<_, String> = async {
        let name = name.trim();
        if name.is_empty() || password.is_empty() {
            return Err("用户名和密码不能为空".into());
        }

        let client = build_client(core)?;
        throttle(api_base).await;
        let response = client
            .post(format!("{api_base}/api/auth/login"))
            .json(&serde_json::json!({ "name": name, "password": password }))
            .timeout(std::time::Duration::from_secs(12))
            .send()
            .await
            .map_err(|e| format!("登录请求失败: {e}"))?;

        if !response.status().is_success() {
            return Err(api_error_message(response, "登录失败").await);
        }

        let session = response
            .json::<AppAuthSession>()
            .await
            .map_err(|e| format!("解析登录响应失败: {e}"))?;
        if let Err(error) = serde_json::to_value(&session.user)
            .map_err(CoreError::from)
            .and_then(|user| {
                core.update_state(|state| {
                    state.auth_token = Some(session.token.clone());
                    state.user = Some(user);
                })
            })
        {
            tracing::warn!(%error, "Failed to persist login session");
        }
        Ok(session)
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn keytao_me(core: &Core, token: String) -> Result<AppAuthUser, CoreError> {
    if core.state().auth_token.as_deref() == Some(token.trim()) && account_id(core).is_some() {
        let api = ApiClient::new(core, API_BASE);
        let user: AppAuthUser = serde_json::from_value(
            api.request(reqwest::Method::GET, "/api/auth/me", None)
                .await?,
        )?;
        if Some(user.id) != api.account_id {
            return Err(CoreError::Permission("账号信息不匹配".into()));
        }
        let token = api.token()?;
        let value = serde_json::to_value(&user)?;
        core.update_state(|state| {
            if state.auth_token.as_deref() == Some(&token) {
                state.user = Some(value);
            }
        })?;
        return Ok(user);
    }
    keytao_me_at(core, token, API_BASE).await
}

async fn keytao_me_at(
    core: &Core,
    token: String,
    api_base: &str,
) -> Result<AppAuthUser, CoreError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(CoreError::Other("未登录".into()));
    }

    let client = build_client(core).map_err(CoreError::Other)?;
    let response = client
        .get(format!("{api_base}/api/auth/me"))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| CoreError::Other(format!("校验登录状态失败: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let message = api_error_message(response, "校验登录状态失败").await;
        // Only an explicit rejection of the token may end the session; every
        // other failure is treated as transient by callers.
        return Err(match status {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                CoreError::Permission(message)
            }
            _ => CoreError::Other(message),
        });
    }

    let user = response
        .json::<AppAuthUser>()
        .await
        .map_err(|e| CoreError::Other(format!("解析账号信息失败: {e}")))?;
    if let Err(error) = serde_json::to_value(&user)
        .map_err(CoreError::from)
        .and_then(|stored_user| {
            core.update_state(|state| {
                // Compare under the state lock after the request completes: a stale
                // response must not update a newer login or restore a cleared session.
                if state.auth_token.as_deref() == Some(token) {
                    state.user = Some(stored_user);
                }
            })
        })
    {
        tracing::warn!(%error, "Failed to persist refreshed account");
    }
    Ok(user)
}

fn active_import_exists(content: &str, import_name: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#')
            && trimmed
                .strip_prefix("-")
                .map(|rest| rest.trim().split_whitespace().next() == Some(import_name))
                .unwrap_or(false)
    })
}

fn ensure_import_table_entry(path: &Path, import_name: &str) -> Result<bool, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("读取词典导入表失败 {}: {e}", path.display()))?;
    let Some(updated) = patch_import_table(&content, import_name) else {
        return Ok(false);
    };
    crate::scheme_files::write_file_atomic(path, updated.as_bytes())
        .map_err(|e| format!("写入词典导入表失败 {}: {e}", path.display()))?;
    Ok(true)
}

fn patch_import_table(content: &str, import_name: &str) -> Option<String> {
    if active_import_exists(content, import_name) {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("import_tables:"))?;

    let indent = lines
        .iter()
        .skip(index + 1)
        .find_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("-") && !trimmed.starts_with("#") {
                Some(line.len() - trimmed.len())
            } else {
                None
            }
        })
        .unwrap_or(2);

    let mut output = Vec::with_capacity(lines.len() + 1);
    for (line_index, line) in lines.iter().enumerate() {
        output.push((*line).to_string());
        if line_index == index {
            output.push(format!("{}- {import_name}", " ".repeat(indent)));
        }
    }

    Some(format!("{}\n", output.join("\n")))
}

pub(crate) fn ensure_user_dictionary_imports(root: &Path) -> Result<bool, String> {
    if keytao_core::RuntimeDirs::isolation_enabled(root)
        && keytao_core::RuntimeDirs::active_package_key(root).is_some()
    {
        // Isolated deployment generates the import overrides in each package's runtime.
        return Ok(false);
    }
    let mut patched = false;
    let entries = std::fs::read_dir(root).map_err(|e| format!("读取输入法目录失败: {e}"))?;
    for entry in entries.filter_map(|item| item.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !(filename.starts_with("keytao") && filename.ends_with(".extended.dict.yaml")) {
            continue;
        }

        patched |= ensure_import_table_entry(&path, "keytao.user")?;
    }
    patched |= crate::remove_filter::ensure_remove_filter(root)?;
    Ok(patched)
}

pub(crate) fn runtime_dictionary_usage<'a>(
    schemas: impl Iterator<Item = &'a [u8]>,
    dictionaries: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeSet<String>, CoreError> {
    fn dictionary_references(config: &serde_yaml::Value, names: &mut BTreeSet<String>) {
        match config {
            serde_yaml::Value::Mapping(mapping) => {
                for (key, value) in mapping {
                    if key.as_str().is_some_and(|key| {
                        key == "dictionary" || key.ends_with("/dictionary")
                    }) {
                        if let Some(name) = value.as_str() {
                            names.insert(name.to_owned());
                        }
                    }
                    dictionary_references(value, names);
                }
            }
            serde_yaml::Value::Sequence(items) => {
                for item in items {
                    dictionary_references(item, names);
                }
            }
            _ => {}
        }
    }

    let mut pending = BTreeSet::new();
    for schema in schemas {
        dictionary_references(&crate::upload_bundle::yaml(schema, "schema")?, &mut pending);
    }
    let mut used = BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if let Some(bytes) = dictionaries.get(&format!("{name}.dict.yaml")) {
            let config = crate::upload_bundle::yaml(bytes, &name)?;
            if let Some(imports) = config["import_tables"].as_sequence() {
                pending.extend(imports.iter().filter_map(serde_yaml::Value::as_str)
                    .filter(|import| *import != name && !used.contains(*import))
                    .map(str::to_owned));
            }
        }
        used.insert(name);
    }
    Ok(used)
}

pub(crate) fn add_runtime_user_dictionary(
    control_root: &Path,
    dictionaries: &BTreeMap<String, Vec<u8>>,
    used: &BTreeSet<String>,
    generated: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), CoreError> {
    if !used.iter().any(|dictionary| dictionary.starts_with("keytao")) {
        return Ok(());
    }

    for name in [USER_DICTIONARY, USER_WORD_REMOVALS] {
        let path = crate::packages::safe_path(control_root, name).map_err(CoreError::Other)?;
        match std::fs::read(path) {
            Ok(bytes) => { generated.insert(name.into(), bytes); }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if generated.contains_key(USER_DICTIONARY) {
        for dictionary in used.iter().filter(|name| {
            name.starts_with("keytao") && name.ends_with(".extended")
        }) {
            let name = format!("{dictionary}.dict.yaml");
            if let Some(bytes) = dictionaries.get(&name) {
                let content = std::str::from_utf8(bytes)
                    .map_err(|error| CoreError::Other(format!("{name}: {error}")))?;
                if let Some(patched) = patch_import_table(content, "keytao.user") {
                    generated.insert(name, patched.into_bytes());
                }
            }
        }
    }
    Ok(())
}

pub async fn sync_user_dictionary(
    core: &Core,
    host: &impl ImeHost,
    token: String,
    confirmed_deletions: Option<UserDictionaryDeletionConfirmation>,
) -> Result<UserDictionarySyncResult, CoreError> {
    sync_user_dictionary_at(core, host, token, API_BASE, confirmed_deletions).await
}

async fn sync_user_dictionary_at(
    core: &Core,
    host: &impl ImeHost,
    token: String,
    api_base: &str,
    confirmed_deletions: Option<UserDictionaryDeletionConfirmation>,
) -> Result<UserDictionarySyncResult, CoreError> {
    let api = ApiClient::new(core, api_base);
    if api.token()?.trim() != token.trim() {
        return Err(CoreError::Permission("登录状态已更改".into()));
    }
    let account = api.account_id.unwrap();
    let remote = api
        .request(reqwest::Method::GET, "/api/user/dictionary", None)
        .await?;
    let supports_removals = remote["entries"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry.get("remove").and_then(Value::as_bool).is_some())
    });
    let remote: RemoteDictionary = serde_json::from_value(remote)?;
    let ids: BTreeSet<_> = remote.entries.iter().map(|entry| entry.id).collect();
    if remote.total != remote.entries.len()
        || ids.len() != remote.entries.len()
        || remote.entries.iter().any(|entry| {
            entry.id <= 0
                || entry.user_id != account
                || entry.word.trim().is_empty()
                || entry.word.trim() != entry.word
                || entry.word.encode_utf16().count() > 100
                || entry.word.contains(['\t', '\r', '\n'])
                || entry.code.is_empty()
                || entry.code.encode_utf16().count() > 64
                || entry.code.chars().any(char::is_whitespace)
                || entry.word.starts_with('#')
                || entry.code.starts_with('#')
                || !(0..=1_000_000).contains(&entry.weight)
                || !matches!(
                    entry.kind.as_str(),
                    "Single"
                        | "Phrase"
                        | "Supplement"
                        | "Symbol"
                        | "Link"
                        | "CSS"
                        | "CSSSingle"
                        | "English"
                )
        })
    {
        return Err(CoreError::Other("用户词库响应格式不正确".into()));
    }
    api.token()?;
    let root = host.user_root().map_err(CoreError::Other)?;
    let dictionary = root.join("keytao.user.dict.yaml");
    let (mut snapshot, marker_snapshot, local, local_set, base) = {
        let mut state = core
            .user_words
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let dictionary_existed = dictionary.try_exists()?;
        let (snapshot, local) =
            crate::user_words::read_user_words_locked(core, &mut state, "keytao", &root)?;
        let marker_snapshot = crate::user_words::read_user_word_removals(&root)?;
        let local_set: BTreeSet<_> = local
            .iter()
            .map(|entry| (entry.word.clone(), entry.code.clone(), entry.remove))
            .collect();
        // A matching instance remains authoritative even after deleting every base entry.
        let base = if dictionary_existed {
            load_base(core, &root, account, &snapshot, &local_set)?
        } else {
            BTreeSet::new()
        };
        (snapshot, marker_snapshot, local, local_set, base)
    };
    let mut remote_by_key: BTreeMap<(String, String, bool), Vec<&RemoteEntry>> = BTreeMap::new();
    for entry in &remote.entries {
        remote_by_key
            .entry((entry.word.clone(), entry.code.clone(), entry.remove))
            .or_default()
            .push(entry);
    }
    let local_deletions: Vec<_> = local
        .iter()
        .filter(|entry| {
            let key = (entry.word.clone(), entry.code.clone(), entry.remove);
            (!entry.remove || supports_removals)
                && base.contains(&key)
                && !remote_by_key.contains_key(&key)
        })
        .map(|entry| UserDictionaryDeletion {
            word: entry.word.clone(),
            code: entry.code.clone(),
            remove: entry.remove,
        })
        .collect();
    let server_deletions: Vec<_> = remote
        .entries
        .iter()
        .filter(|entry| {
            let key = (entry.word.clone(), entry.code.clone(), entry.remove);
            base.contains(&key) && !local_set.contains(&key)
        })
        .map(|entry| UserDictionaryDeletion {
            word: entry.word.clone(),
            code: entry.code.clone(),
            remove: entry.remove,
        })
        .collect();
    let confirmed_local: BTreeSet<_> = confirmed_deletions
        .as_ref()
        .into_iter()
        .flat_map(|confirmed| &confirmed.local_deletions)
        .map(|entry| (entry.word.clone(), entry.code.clone(), entry.remove))
        .collect();
    let confirmed_server: BTreeSet<_> = confirmed_deletions
        .as_ref()
        .into_iter()
        .flat_map(|confirmed| &confirmed.server_deletions)
        .map(|entry| (entry.word.clone(), entry.code.clone(), entry.remove))
        .collect();
    let unconfirmed_local: BTreeSet<_> = local_deletions
        .iter()
        .map(|entry| (entry.word.clone(), entry.code.clone(), entry.remove))
        .filter(|key| !confirmed_local.contains(key))
        .collect();
    let unconfirmed_server: BTreeSet<_> = server_deletions
        .iter()
        .map(|entry| (entry.word.clone(), entry.code.clone(), entry.remove))
        .filter(|key| !confirmed_server.contains(key))
        .collect();
    let needs_confirmation = if confirmed_deletions.is_some() {
        unconfirmed_local.len() + unconfirmed_server.len() > 3
    } else {
        unconfirmed_local.len() > 3 || unconfirmed_server.len() > 3
    };
    if needs_confirmation {
        return Ok(UserDictionarySyncResult {
            file_name: "keytao.user.dict.yaml".into(),
            path: path_string(root.join("keytao.user.dict.yaml")),
            count: local_set.len(),
            updated_at: String::new(),
            import_table_patched: false,
            reload_stamp_path: None,
            message: "批量删除需要确认".into(),
            pending: true,
            local_deletions,
            server_deletions,
        });
    }
    if std::str::from_utf8(&snapshot)
        .ok()
        .and_then(dictionary_instance)
        .is_none()
    {
        let _guard = core
            .user_words
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if std::fs::read(&dictionary)? != snapshot
            || crate::user_words::read_user_word_removals(&root)? != marker_snapshot
        {
            return Err(CoreError::Other(
                "User dictionary changed during sync; retry the sync".into(),
            ));
        }
        // Establish an instance before uploads so an interrupted first sync can advance its base.
        save_base(&root, account, &base)?;
        snapshot = std::fs::read(&dictionary)?;
    }
    let mut completed_base = base.clone();
    completed_base.retain(|key| {
        (!supports_removals && key.2) || local_set.contains(key) || remote_by_key.contains_key(key)
    });
    completed_base.extend(
        local_set
            .iter()
            .filter(|key| remote_by_key.contains_key(*key))
            .cloned(),
    );
    let result = async {
        let mut resulting = local_set.clone();
        for key in &local_set {
            if key.2 && !supports_removals {
                continue;
            }
            if !remote_by_key.contains_key(key) {
                if base.contains(key) {
                    if confirmed_deletions.is_none() || confirmed_local.contains(key) {
                        resulting.remove(key);
                    }
                } else {
                    let mut body = serde_json::json!({
                        "word": key.0,
                        "code": key.1,
                        "type": "Phrase",
                        "replacePublic": false,
                    });
                    if key.2 {
                        body["remove"] = Value::Bool(true);
                    }
                    let uploaded = api
                        .request(reqwest::Method::POST, "/api/user/dictionary", Some(body))
                        .await?;
                    let uploaded: RemoteEntry = serde_json::from_value(uploaded["entry"].clone())?;
                    if uploaded.word != key.0
                        || uploaded.code != key.1
                        || uploaded.remove != key.2
                        || uploaded.user_id != account
                        || uploaded.id <= 0
                        || uploaded.kind != "Phrase"
                        || !(100..=1_000_000).contains(&uploaded.weight)
                    {
                        return Err(CoreError::Other("保存用户词条响应格式不正确".into()));
                    }
                    completed_base.insert(key.clone());
                }
            }
        }
        for (key, entries) in &remote_by_key {
            if !local_set.contains(key) {
                if base.contains(key) {
                    if confirmed_deletions.is_some() && !confirmed_server.contains(key) {
                        continue;
                    }
                    for entry in entries {
                        let deleted = api
                            .request(
                                reqwest::Method::DELETE,
                                &format!("/api/user/dictionary/{}", entry.id),
                                None,
                            )
                            .await?;
                        if deleted["success"].as_bool() != Some(true) {
                            return Err(CoreError::Other("删除用户词条响应格式不正确".into()));
                        }
                    }
                    completed_base.remove(key);
                } else {
                    resulting.insert(key.clone());
                }
            }
        }
        api.token()?;
        let (import_table_patched, reload_stamp_path) = {
            let mut state = core
                .user_words
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Hold the edit lock from the content check through dictionary and base writes.
            let current = std::fs::read(&dictionary)?;
            if current != snapshot
                || crate::user_words::read_user_word_removals(&root)? != marker_snapshot
            {
                return Err(CoreError::Other(
                    "User dictionary changed during sync; retry the sync".into(),
                ));
            }
            let import_table_patched =
                ensure_user_dictionary_imports(&root).map_err(CoreError::Other)?;
            for entry in local.iter().rev().filter(|entry| {
                !resulting.contains(&(entry.word.clone(), entry.code.clone(), entry.remove))
            }) {
                if entry.remove {
                    crate::user_words::set_user_word_removed_locked(
                        core,
                        &mut state,
                        "keytao",
                        &root,
                        &entry.word,
                        &entry.code,
                        false,
                    )?;
                } else {
                    crate::user_words::delete_user_word_locked(
                        core,
                        &mut state,
                        "keytao",
                        &root,
                        entry.id,
                        &entry.word,
                        &entry.code,
                    )?;
                }
            }
            for (key, entries) in &remote_by_key {
                if !local_set.contains(key) && resulting.contains(key) {
                    if key.2 {
                        crate::user_words::set_user_word_removed_locked(
                            core, &mut state, "keytao", &root, &key.0, &key.1, true,
                        )?;
                    } else {
                        crate::user_words::add_user_word_locked(
                            core,
                            &mut state,
                            "keytao",
                            &root,
                            &key.0,
                            &key.1,
                            Some(entries[0].weight.to_string()),
                        )?;
                    }
                }
            }
            let reload_stamp_path = host
                .write_reload_stamp(&root)
                .map_err(CoreError::Other)?
                .map(path_string);
            let mut next_base = resulting.clone();
            if confirmed_deletions.is_some() {
                // Keep deferred deletions in the base so the next sync still detects them.
                next_base.extend(unconfirmed_server);
            }
            if !supports_removals {
                // Preserve marker history until GET explicitly advertises removal support.
                next_base.retain(|key| !key.2);
                next_base.extend(base.iter().filter(|key| key.2).cloned());
            }
            save_base(&root, account, &next_base)?;
            (import_table_patched, reload_stamp_path)
        };
        Ok((resulting, import_table_patched, reload_stamp_path))
    }
    .await;
    let (resulting, import_table_patched, reload_stamp_path) = match result {
        Ok(result) => result,
        Err(error) => {
            let _guard = core
                .user_words
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let current = std::fs::read(&dictionary).ok();
            let instance = std::str::from_utf8(&snapshot)
                .ok()
                .and_then(dictionary_instance);
            if instance.is_some()
                && instance
                    == current
                        .as_deref()
                        .and_then(|content| std::str::from_utf8(content).ok())
                        .and_then(dictionary_instance)
            {
                // Preserve acknowledged server changes without overwriting newer local edits.
                if let Err(save_error) = save_base(&root, account, &completed_base) {
                    tracing::warn!(%save_error, "Failed to persist user dictionary sync base");
                }
            }
            return Err(error);
        }
    };
    match std::fs::remove_file(legacy_base_path(core, account)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(UserDictionarySyncResult {
        file_name: "keytao.user.dict.yaml".into(),
        path: path_string(root.join("keytao.user.dict.yaml")),
        count: resulting.len(),
        updated_at: String::new(),
        import_table_patched,
        reload_stamp_path,
        message: format!("已同步 {} 条用户词条", resulting.len()),
        pending: false,
        local_deletions: Vec::new(),
        server_deletions: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dictionary_follows_imports_and_preserves_the_control_source() {
        let root = tempfile::tempdir().unwrap();
        let source = b"---\nname: keytao.user\n...\nuser\taa\n";
        let removals = b"removed\tbb\n";
        std::fs::write(root.path().join(USER_DICTIONARY), source).unwrap();
        std::fs::write(root.path().join(USER_WORD_REMOVALS), removals).unwrap();
        let dictionaries = BTreeMap::from([
            ("wrapper.dict.yaml".into(), b"import_tables: [keytao.extended]\n".to_vec()),
            ("keytao.extended.dict.yaml".into(), b"import_tables:\n  - keytao.phrase\n".to_vec()),
        ]);
        let schema: &[u8] = b"patch: {translator/dictionary: wrapper}";
        let used = runtime_dictionary_usage(std::iter::once(schema), &dictionaries).unwrap();
        let mut generated = BTreeMap::new();
        add_runtime_user_dictionary(root.path(), &dictionaries, &used, &mut generated).unwrap();
        assert_eq!(generated[USER_DICTIONARY], source);
        assert_eq!(generated[USER_WORD_REMOVALS], removals);
        assert_eq!(generated["keytao.extended.dict.yaml"], b"import_tables:\n  - keytao.user\n  - keytao.phrase\n");
        assert_eq!(std::fs::read(root.path().join(USER_DICTIONARY)).unwrap(), source);

        generated.clear();
        let schema: &[u8] = b"translator: {dictionary: other}";
        let unrelated = runtime_dictionary_usage(std::iter::once(schema), &dictionaries).unwrap();
        add_runtime_user_dictionary(root.path(), &dictionaries, &unrelated, &mut generated).unwrap();
        assert!(generated.is_empty());
    }

    #[test]
    fn test_user_dictionary_imports_only_patch_keytao_extended_dicts() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-user-dict-import-test-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("keytao.extended.dict.yaml"),
            "name: keytao.extended\nimport_tables:\n  - keytao.phrase\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("txjx.extended.dict.yaml"),
            "name: txjx.extended\nimport_tables:\n  - txjx.core\n",
        )
        .unwrap();

        assert!(ensure_user_dictionary_imports(&dir).unwrap());
        let keytao = std::fs::read_to_string(dir.join("keytao.extended.dict.yaml")).unwrap();
        let txjx = std::fs::read_to_string(dir.join("txjx.extended.dict.yaml")).unwrap();
        assert!(keytao.contains("- keytao.user"));
        assert!(!txjx.contains("- user"));
        assert!(!txjx.contains("- keytao.user"));

        let _ = std::fs::remove_dir_all(dir);
    }
}
