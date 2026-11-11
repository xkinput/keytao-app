use crate::account::{API_BASE, ApiClient};
use crate::{Core, CoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const PACKAGES_PATH: &str = "/api/user/scheme-packages";
const MAX_PACKAGE_SIZE: u64 = 30 * 1024 * 1024;

fn cloud_error(error: CoreError) -> CoreError {
    if error.to_string().starts_with("HTTP 503:") {
        CoreError::Other("云端方案暂不可用".into())
    } else {
        error
    }
}

fn account_id(core: &Core) -> Option<i64> {
    core.state()
        .user
        .as_ref()
        .and_then(|user| user["id"].as_i64())
}

fn same_account(state: &crate::AppState, expected: Option<i64>) -> bool {
    expected.is_some_and(|id| id > 0)
        && state.user.as_ref().and_then(|user| user["id"].as_i64()) == expected
        && state
            .auth_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
}

fn ensure_account(core: &Core, expected: Option<i64>) -> Result<(), CoreError> {
    if same_account(&core.state(), expected) {
        Ok(())
    } else {
        Err(CoreError::Permission("登录状态已更改，请重新登录".into()))
    }
}

fn record_reference(
    core: &Core,
    expected: Option<i64>,
    scheme_key: &str,
    id: &str,
    sha256: &str,
) -> Result<(), CoreError> {
    if !crate::packages::stored_archive(core, scheme_key)?
        .1
        .zip_sha256
        .eq_ignore_ascii_case(sha256)
    {
        return Err(CoreError::Other("本地方案包已更新，请重试".into()));
    }
    let mut recorded = false;
    core.update_state(|state| {
        if same_account(state, expected) {
            state.scheme_cloud_ids.insert(scheme_key.into(), id.into());
            recorded = true;
        }
    })?;
    if recorded {
        Ok(())
    } else {
        Err(CoreError::Permission("登录状态已更改，请重新登录".into()))
    }
}

#[cfg(test)]
#[path = "cloud_package_tests.rs"]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPackage {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: String,
    pub scheme_key: String,
    pub name: String,
    pub version: Option<String>,
    pub size: u64,
    pub sha256: String,
    pub create_at: String,
}

fn deserialize_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let id = match value {
        Value::String(id) => id,
        Value::Number(id) => id.to_string(),
        _ => return Err(serde::de::Error::custom("Invalid package id")),
    };
    valid_id(&id).map_err(serde::de::Error::custom)?;
    Ok(id)
}

fn valid_id(id: &str) -> Result<(), String> {
    if !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && id.parse::<i32>().is_ok_and(|id| id > 0)
    {
        Ok(())
    } else {
        Err("云端方案编号无效".into())
    }
}

pub async fn list(core: &Core) -> Result<Vec<CloudPackage>, CoreError> {
    list_at(core, API_BASE).await
}

pub(crate) async fn list_at(core: &Core, api_base: &str) -> Result<Vec<CloudPackage>, CoreError> {
    let value = ApiClient::new(core, api_base)
        .request(reqwest::Method::GET, PACKAGES_PATH, None)
        .await
        .map_err(cloud_error)?;
    let packages: Vec<CloudPackage> = serde_json::from_value(value["packages"].clone())?;
    for package in &packages {
        if package.size == 0
            || package.size > MAX_PACKAGE_SIZE
            || package.sha256.len() != 64
            || !package.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CoreError::Other("云端方案信息无效".into()));
        }
    }
    Ok(packages)
}

pub async fn upload(core: &Core, scheme_key: &str) -> Result<CloudPackage, CoreError> {
    upload_at(core, scheme_key, API_BASE).await
}

pub(crate) async fn upload_at(
    core: &Core,
    scheme_key: &str,
    api_base: &str,
) -> Result<CloudPackage, CoreError> {
    let account = account_id(core);
    let (bytes, meta) = crate::packages::stored_archive(core, scheme_key)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_PACKAGE_SIZE {
        return Err(CoreError::Other("方案包不能超过 30 MiB".into()));
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    if let Some(package) = list_at(core, api_base)
        .await?
        .into_iter()
        .find(|package| package.sha256.eq_ignore_ascii_case(&sha256))
    {
        ensure_account(core, account)?;
        if package.scheme_key == scheme_key {
            record_reference(core, account, scheme_key, &package.id, &sha256)?;
        }
        return Ok(package);
    }
    ensure_account(core, account)?;
    let mut body =
        json!({"schemeKey": scheme_key, "name": scheme_key, "size": bytes.len(), "sha256": sha256});
    if let Some(version) = &meta.version {
        body["version"] = json!(version);
    }
    let api = ApiClient::new(core, api_base);
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Ticket {
        #[serde(deserialize_with = "deserialize_id")]
        id: String,
        upload_url: String,
        headers: std::collections::BTreeMap<String, String>,
    }
    let ticket: Ticket = serde_json::from_value(
        api.request(reqwest::Method::POST, PACKAGES_PATH, Some(body))
            .await
            .map_err(cloud_error)?,
    )?;
    let client = storage_client()?;
    let mut request = client
        .put(storage_url(&ticket.upload_url)?)
        .timeout(Duration::from_secs(120));
    for (name, value) in ticket.headers {
        request = request.header(name, value);
    }
    let response = request
        .body(bytes)
        .send()
        .await
        .map_err(|error| CoreError::Network(error.to_string()))?;
    if !response.status().is_success() {
        return Err(CoreError::Other(format!(
            "方案包上传失败: HTTP {}",
            response.status().as_u16()
        )));
    }
    api.request(
        reqwest::Method::POST,
        &format!("{PACKAGES_PATH}/{}/complete", ticket.id),
        None,
    )
    .await
    .map_err(cloud_error)?;
    let package = list_at(core, api_base)
        .await?
        .into_iter()
        .find(|package| package.id == ticket.id)
        .ok_or_else(|| CoreError::Other("上传完成后未找到云端方案".into()))?;
    record_reference(core, account, scheme_key, &package.id, &sha256)?;
    Ok(package)
}

fn storage_client() -> Result<reqwest::Client, CoreError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| CoreError::Other(error.to_string()))
}

fn storage_url(value: &str) -> Result<reqwest::Url, CoreError> {
    let url = reqwest::Url::parse(value).map_err(|_| CoreError::Other("方案包地址无效".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(CoreError::Other("方案包地址无效".into()));
    }
    Ok(url)
}

pub(crate) async fn download_at(
    core: &Core,
    id: &str,
    user_root: &Path,
    api_base: &str,
) -> Result<(), CoreError> {
    use futures_util::StreamExt;
    let account = account_id(core);
    valid_id(id).map_err(CoreError::Other)?;
    let package = list_at(core, api_base)
        .await?
        .into_iter()
        .find(|package| package.id == id)
        .ok_or_else(|| CoreError::Other("云端方案不存在".into()))?;
    crate::scheme::validate_scheme_key(&package.scheme_key).map_err(CoreError::Other)?;
    ensure_account(core, account)?;
    if core.state().active_scheme.as_deref() == Some(&package.scheme_key) {
        return Err(CoreError::Other("请先切换到其他方案".into()));
    }
    let url = ApiClient::new(core, api_base)
        .download_url(&format!("{PACKAGES_PATH}/{id}"))
        .await
        .map_err(cloud_error)?;
    let response = storage_client()?
        .get(storage_url(&url)?)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|error| CoreError::Network(error.to_string()))?;
    if !response.status().is_success() {
        return Err(CoreError::Other(format!(
            "方案包下载失败: HTTP {}",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_PACKAGE_SIZE)
    {
        return Err(CoreError::Other("方案包不能超过 30 MiB".into()));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| CoreError::Network(error.to_string()))?;
        if bytes.len() as u64 + chunk.len() as u64 > MAX_PACKAGE_SIZE {
            return Err(CoreError::Other("方案包不能超过 30 MiB".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != package.size
        || !format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(&package.sha256)
    {
        return Err(CoreError::Other("方案包 SHA-256 校验失败".into()));
    }
    ensure_account(core, account)?;
    crate::packages::store_cloud_archive(
        core,
        &package.scheme_key,
        package.version.as_deref(),
        &bytes,
        user_root,
    )?;
    record_reference(core, account, &package.scheme_key, id, &package.sha256)?;
    Ok(())
}

pub async fn download(core: &Core, id: &str, user_root: &Path) -> Result<(), CoreError> {
    download_at(core, id, user_root, API_BASE).await
}

pub(crate) async fn delete_at(core: &Core, id: &str, api_base: &str) -> Result<(), CoreError> {
    let account = account_id(core);
    valid_id(id).map_err(CoreError::Other)?;
    ApiClient::new(core, api_base)
        .request(
            reqwest::Method::DELETE,
            &format!("{PACKAGES_PATH}/{id}"),
            None,
        )
        .await
        .map_err(cloud_error)?;
    ensure_account(core, account)?;
    core.update_state(|state| {
        if same_account(state, account) {
            state.scheme_cloud_ids.retain(|_, cloud_id| cloud_id != id);
        }
    })
}

pub async fn delete(core: &Core, id: &str) -> Result<(), CoreError> {
    delete_at(core, id, API_BASE).await
}
