use crate::{
    Core, CoreError,
    ime_host::ImeHost,
    ime_paths::path_string,
    scheme::{api_error_message, build_client},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[cfg(test)]
#[path = "account_tests.rs"]
mod account_tests;

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDictionaryExportResponse {
    file_name: String,
    content: String,
    count: usize,
    updated_at: String,
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
}

const API_BASE: &str = "https://keytao.rea.ink";

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
    keytao_me_at(core, token, API_BASE).await
}

async fn keytao_me_at(
    core: &Core,
    token: String,
    api_base: &str,
) -> Result<AppAuthUser, CoreError> {
    let result: Result<_, String> = async {
        let token = token.trim();
        if token.is_empty() {
            return Err("未登录".into());
        }

        let client = build_client(core)?;
        let response = client
            .get(format!("{api_base}/api/auth/me"))
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(12))
            .send()
            .await
            .map_err(|e| format!("校验登录状态失败: {e}"))?;

        if !response.status().is_success() {
            return Err(api_error_message(response, "校验登录状态失败").await);
        }

        let user = response
            .json::<AppAuthUser>()
            .await
            .map_err(|e| format!("解析账号信息失败: {e}"))?;
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
    .await;
    result.map_err(CoreError::Other)
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
    if active_import_exists(&content, import_name) {
        return Ok(false);
    }

    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = lines
        .iter()
        .position(|line| line.trim_start().starts_with("import_tables:"))
    else {
        return Ok(false);
    };

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

    std::fs::write(path, format!("{}\n", output.join("\n")))
        .map_err(|e| format!("写入词典导入表失败 {}: {e}", path.display()))?;
    Ok(true)
}

fn ensure_user_dictionary_imports(root: &Path) -> Result<bool, String> {
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
    Ok(patched)
}

pub async fn sync_user_dictionary(
    core: &Core,
    host: &impl ImeHost,
    token: String,
) -> Result<UserDictionarySyncResult, CoreError> {
    let result: Result<_, String> = async {
        let token = token.trim();
        if token.is_empty() {
            return Err("请先登录 KeyTao 账号".into());
        }

        let client = build_client(core)?;
        let response = client
            .get(format!("{API_BASE}/api/user/dictionary/export"))
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("同步用户词库失败: {e}"))?;

        if !response.status().is_success() {
            return Err(api_error_message(response, "同步用户词库失败").await);
        }

        let export = response
            .json::<UserDictionaryExportResponse>()
            .await
            .map_err(|e| format!("解析用户词库失败: {e}"))?;
        let root = host.user_root()?;
        std::fs::create_dir_all(&root).map_err(|e| format!("创建输入法目录失败: {e}"))?;

        let keytao_path = root.join("keytao.user.dict.yaml");
        std::fs::write(&keytao_path, export.content.as_bytes())
            .map_err(|e| format!("写入用户词库失败 {}: {e}", keytao_path.display()))?;

        let import_table_patched = ensure_user_dictionary_imports(&root)?;
        let reload_stamp_path = host.write_reload_stamp(&root)?.map(path_string);

        Ok(UserDictionarySyncResult {
            file_name: export.file_name,
            path: path_string(keytao_path),
            count: export.count,
            updated_at: export.updated_at,
            import_table_patched,
            reload_stamp_path,
            message: format!("已同步 {} 条用户词条", export.count),
        })
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;
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
