use crate::{
    Core, CoreError,
    account::{API_BASE, ApiClient},
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, io::Write, time::Duration};
use tokio::{sync::Mutex, time::Instant};

#[cfg(test)]
#[path = "submission_tests.rs"]
mod submission_tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionWord {
    pub word: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubmissionPreview {
    pub account_id: i64,
    pub items: Vec<SubmissionWord>,
    pub already_published: Vec<SubmissionWord>,
    pub already_pending: Vec<SubmissionWord>,
    pub conflicts: Vec<Value>,
    pub warnings: Vec<Value>,
    pub batch_id: Option<String>,
    pub content_version: Option<u64>,
    pub warning_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubmissionResult {
    pub batch_id: String,
    pub status: String,
    pub items: Vec<SubmissionWord>,
    pub confirmation: Option<SubmissionPreview>,
}

static LAST_LOOKUP: Mutex<Option<Instant>> = Mutex::const_new(None);
static SUBMISSION_LOCK: Mutex<()> = Mutex::const_new(());

fn invalid_response() -> CoreError {
    CoreError::Other("加词平台响应格式错误".into())
}

fn account_id(core: &Core) -> Result<i64, CoreError> {
    let state = core.state();
    if state
        .auth_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
    {
        return Err(CoreError::Permission("请先登录 KeyTao 账号".into()));
    }
    state
        .user
        .as_ref()
        .and_then(|user| user["id"].as_i64())
        .ok_or_else(|| CoreError::Permission("请先登录 KeyTao 账号".into()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    page: u64,
    total_pages: u64,
}

pub async fn pending_user_words(core: &Core) -> Result<Vec<SubmissionWord>, CoreError> {
    pending_user_words_at(core, API_BASE).await
}

async fn pending_user_words_at(
    core: &Core,
    base_url: &str,
) -> Result<Vec<SubmissionWord>, CoreError> {
    account_id(core)?;
    let api = ApiClient::new(core, base_url);
    let mut pending = Vec::new();
    for page in 1..=100 {
        let response = api
            .request(
                Method::GET,
                &format!("/api/batches?onlyMine=true&page={page}&pageSize=100"),
                None,
            )
            .await?;
        let batches = response["batches"]
            .as_array()
            .ok_or_else(invalid_response)?;
        let pagination: Pagination = serde_json::from_value(response["pagination"].clone())?;
        if pagination.page != page || pagination.total_pages > 100 {
            return Err(invalid_response());
        }
        for batch in batches {
            let status = batch["status"].as_str().ok_or_else(invalid_response)?;
            if !matches!(status, "Draft" | "Submitted" | "Approved" | "Rejected") {
                return Err(invalid_response());
            }
            if matches!(status, "Draft" | "Rejected") {
                continue;
            }
            let mut batch = batch.clone();
            let preview_count = batch["pullRequests"]
                .as_array()
                .ok_or_else(invalid_response)?
                .len() as u64;
            let count = batch["_count"]["pullRequests"]
                .as_u64()
                .ok_or_else(invalid_response)?;
            if count < preview_count {
                return Err(invalid_response());
            }
            if count > preview_count {
                let id = batch["id"].as_str().ok_or_else(invalid_response)?;
                let response = api
                    .request(Method::GET, &batch_path(id)?, None)
                    .await?;
                if response["batch"]["id"].as_str() != Some(id) {
                    return Err(invalid_response());
                }
                batch = response["batch"].clone();
                let status = batch["status"].as_str().ok_or_else(invalid_response)?;
                if matches!(status, "Draft" | "Rejected") {
                    continue;
                }
                if !matches!(status, "Submitted" | "Approved") {
                    return Err(invalid_response());
                }
            }
            for pr in batch["pullRequests"]
                .as_array()
                .ok_or_else(invalid_response)?
            {
                let action = pr["action"].as_str().ok_or_else(invalid_response)?;
                let status = pr["status"].as_str().ok_or_else(invalid_response)?;
                if action == "Create" && status != "Rejected" {
                    pending.push(SubmissionWord {
                        word: pr["word"].as_str().ok_or_else(invalid_response)?.into(),
                        code: pr["code"].as_str().ok_or_else(invalid_response)?.into(),
                        remark: None,
                    });
                }
            }
        }
        if page >= pagination.total_pages {
            return Ok(pending);
        }
    }
    Err(invalid_response())
}

pub async fn prepare_submission(
    core: &Core,
    entries: Vec<SubmissionWord>,
) -> Result<SubmissionPreview, CoreError> {
    prepare_submission_at(core, entries, API_BASE).await
}

async fn prepare_submission_at(
    core: &Core,
    entries: Vec<SubmissionWord>,
    base_url: &str,
) -> Result<SubmissionPreview, CoreError> {
    let account_id = account_id(core)?;
    if entries.is_empty() || entries.len() > 500 {
        return Err(CoreError::Other("请选择 1–500 个词条".into()));
    }
    let api = ApiClient::new(core, base_url);
    let pending = pending_user_words_at(core, base_url).await?;
    let pending: HashSet<_> = pending
        .iter()
        .map(|word| (word.word.as_str(), word.code.as_str()))
        .collect();
    let mut preview = SubmissionPreview {
        account_id,
        items: Vec::new(),
        already_published: Vec::new(),
        already_pending: Vec::new(),
        conflicts: Vec::new(),
        warnings: Vec::new(),
        batch_id: None,
        content_version: None,
        warning_digest: None,
    };
    let mut seen = HashSet::new();
    for mut entry in entries {
        entry.word = entry.word.trim().into();
        entry.code = entry.code.trim().into();
        entry.remark = entry.remark.filter(|remark| !remark.is_empty());
        if entry.word.is_empty() || entry.code.is_empty() {
            return Err(CoreError::Other("词条和编码不能为空".into()));
        }
        if !seen.insert((entry.word.clone(), entry.code.clone())) {
            continue;
        }
        if pending.contains(&(entry.word.as_str(), entry.code.as_str())) {
            preview.already_pending.push(entry);
            continue;
        }
        if is_published(&api, &entry).await? {
            preview.already_published.push(entry);
        } else {
            preview.items.push(entry);
        }
    }
    if !preview.items.is_empty() {
        let items: Vec<_> = preview.items.iter().enumerate().map(|(index, entry)| json!({
            "id": index.to_string(), "action":"Create", "word":entry.word,"code":entry.code,"type":"Phrase",
        })).collect();
        let response = api
            .request(
                Method::POST,
                "/api/pull-requests/check-conflicts-batch",
                Some(json!({"items":items})),
            )
            .await?;
        let results = response["results"]
            .as_array()
            .ok_or_else(invalid_response)?;
        if results.len() != preview.items.len() {
            return Err(invalid_response());
        }
        for result in results {
            let conflict = &result["conflict"];
            let has_conflict = conflict["hasConflict"]
                .as_bool()
                .ok_or_else(invalid_response)?;
            let resolved = conflict["suggestions"]
                .as_array()
                .is_some_and(|suggestions| {
                    suggestions
                        .iter()
                        .any(|suggestion| suggestion["action"] == "Resolved")
                });
            if has_conflict && !resolved {
                preview.conflicts.push(conflict.clone());
            }
        }
        preview.warnings = response["warnings"]
            .as_array()
            .ok_or_else(invalid_response)?
            .clone();
    }
    if !preview.items.is_empty() {
        if let Some(draft) = read_drafts(core, account_id)?
            .into_iter()
            .find(|draft| same_items(&draft.items, &preview.items))
        {
            preview.batch_id = draft.batch_id;
            preview.content_version = draft.content_version;
        }
    }
    Ok(preview)
}

async fn is_published(api: &ApiClient<'_>, entry: &SubmissionWord) -> Result<bool, CoreError> {
    let mut page = 1u64;
    loop {
        let mut lookup = LAST_LOOKUP.lock().await;
        if let Some(last) = *lookup {
            tokio::time::sleep_until(last + Duration::from_secs(1)).await;
        }
        let page_string = page.to_string();
        let query = reqwest::Url::parse_with_params(
            "http://localhost/api/phrases/by-word",
            &[
                ("word", entry.word.as_str()),
                ("page", page_string.as_str()),
            ],
        )
        .map_err(|_| invalid_response())?;
        let response = api
            .request(
                Method::GET,
                &format!("{}?{}", query.path(), query.query().unwrap()),
                None,
            )
            .await;
        *lookup = Some(Instant::now());
        drop(lookup);
        let response = response?;
        let phrases = response["phrases"]
            .as_array()
            .ok_or_else(invalid_response)?;
        let pagination: Pagination = serde_json::from_value(response["pagination"].clone())?;
        let total = response["pagination"]["total"]
            .as_u64()
            .ok_or_else(invalid_response)?;
        if pagination.page != page
            || response["pagination"]["pageSize"] != 6
            || pagination.total_pages != total.div_ceil(6)
            || phrases.len() > 6
        {
            return Err(invalid_response());
        }
        let mut found = false;
        for phrase in phrases {
            if phrase["word"].as_str() != Some(&entry.word) {
                return Err(invalid_response());
            }
            let code = phrase["code"].as_str().ok_or_else(invalid_response)?;
            found |= code == entry.code;
        }
        if found {
            return Ok(true);
        }
        if page >= pagination.total_pages {
            return Ok(false);
        }
        page += 1;
    }
}

pub async fn submit(
    core: &Core,
    preview: SubmissionPreview,
) -> Result<SubmissionResult, CoreError> {
    let _submission = SUBMISSION_LOCK.lock().await;
    submit_at(core, preview, API_BASE).await
}

async fn submit_at(
    core: &Core,
    mut preview: SubmissionPreview,
    base_url: &str,
) -> Result<SubmissionResult, CoreError> {
    if account_id(core)? != preview.account_id {
        return Err(CoreError::Permission("账号已切换，请重新确认词条".into()));
    }
    if !preview.conflicts.is_empty() || preview.items.is_empty() {
        return Err(CoreError::Other("没有可提交的词条".into()));
    }
    let api = ApiClient::new(core, base_url);
    if let Some(batch_id) = preview.batch_id.clone() {
        let response = api
            .request(Method::GET, &batch_path(&batch_id)?, None)
            .await?;
        let batch = &response["batch"];
        if batch["id"].as_str() != Some(&batch_id)
            || batch["creatorId"].as_i64() != Some(preview.account_id)
        {
            return Err(invalid_response());
        }
        let remote_items: Vec<_> = batch["pullRequests"]
            .as_array()
            .ok_or_else(invalid_response)?
            .iter()
            .map(|pr| {
                if pr["action"] != "Create" || pr["type"] != "Phrase" {
                    return Err(invalid_response());
                }
                Ok(SubmissionWord {
                    word: pr["word"].as_str().ok_or_else(invalid_response)?.into(),
                    code: pr["code"].as_str().ok_or_else(invalid_response)?.into(),
                    remark: match &pr["remark"] {
                        Value::Null => None,
                        Value::String(remark) => Some(remark.clone()),
                        _ => return Err(invalid_response()),
                    },
                })
            })
            .collect::<Result<_, CoreError>>()?;
        if !same_items(&preview.items, &remote_items) {
            remove_draft(core, &preview)?;
            return Err(CoreError::Other("批次内容已被修改，请重新选择词条".into()));
        }
        match batch["status"].as_str() {
            Some("Submitted" | "Approved") => {
                remove_draft(core, &preview)?;
                return Ok(SubmissionResult {
                    batch_id,
                    status: "Submitted".into(),
                    items: preview.items,
                    confirmation: None,
                });
            }
            Some("Draft" | "Rejected") => {
                let remote_version = batch["contentVersion"]
                    .as_u64()
                    .ok_or_else(invalid_response)?;
                if preview.content_version != Some(remote_version) {
                    preview.warning_digest = None;
                }
                preview.content_version = Some(remote_version);
            }
            _ => return Err(invalid_response()),
        }
    }
    if preview.batch_id.is_none() {
        let pending = pending_user_words_at(core, base_url).await?;
        preview.items.retain(|entry| {
            !pending
                .iter()
                .any(|other| entry.word == other.word && entry.code == other.code)
        });
        if preview.items.is_empty() {
            return Err(CoreError::Other("词条已在审核中".into()));
        }
        let items: Vec<_> = preview
            .items
            .iter()
            .map(|entry| {
                let mut item =
                    json!({"action":"Create","word":entry.word,"code":entry.code,"type":"Phrase"});
                if let Some(remark) = &entry.remark {
                    item["remark"] = json!(remark);
                }
                item
            })
            .collect();
        let response = api
            .request(
                Method::POST,
                "/api/pull-requests/batch",
                Some(json!({"items":items})),
            )
            .await?;
        preview.batch_id = Some(
            response["batch"]["id"]
                .as_str()
                .ok_or_else(invalid_response)?
                .into(),
        );
        preview.content_version = Some(
            response["contentVersion"]
                .as_u64()
                .ok_or_else(invalid_response)?,
        );
    }
    save_draft(core, &preview)?;
    let batch_id = preview.batch_id.clone().ok_or_else(invalid_response)?;
    let version = preview.content_version.ok_or_else(invalid_response)?;
    let confirmed = preview.warning_digest.is_some();
    let mut body = json!({"confirmed":confirmed,"expectedContentVersion":version});
    if let Some(digest) = &preview.warning_digest {
        body["expectedWarningDigest"] = json!(digest);
    }
    let path = format!("{}/submit", batch_path(&batch_id)?);
    let mut response = api.request(Method::POST, &path, Some(body)).await?;
    if response["requiresConfirmation"] == true {
        let warnings = response["warnings"]
            .as_array()
            .ok_or_else(invalid_response)?
            .clone();
        let content_version = response["contentVersion"]
            .as_u64()
            .ok_or_else(invalid_response)?;
        let digest = response["warningDigest"]
            .as_str()
            .ok_or_else(invalid_response)?
            .to_string();
        if response["batchId"].as_str() != Some(&batch_id)
            || content_version != version
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(invalid_response());
        }
        let unchanged = same_warnings(&preview.warnings, &warnings);
        preview.warnings = warnings;
        preview.warning_digest = Some(digest.clone());
        save_draft(core, &preview)?;
        if !unchanged {
            return Ok(SubmissionResult {
                batch_id,
                status: "Draft".into(),
                items: preview.items.clone(),
                confirmation: Some(preview),
            });
        }
        response = api.request(Method::POST, &path, Some(json!({
            "confirmed":true,"expectedContentVersion":content_version,"expectedWarningDigest":digest,
        }))).await?;
    }
    if response["batch"]["id"].as_str() != Some(&batch_id)
        || response["batch"]["status"] != "Submitted"
    {
        return Err(invalid_response());
    }
    remove_draft(core, &preview)?;
    Ok(SubmissionResult {
        batch_id,
        status: "Submitted".into(),
        items: preview.items,
        confirmation: None,
    })
}

fn batch_path(id: &str) -> Result<String, CoreError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err(invalid_response());
    }
    Ok(format!("/api/batches/{id}"))
}

fn same_items(left: &[SubmissionWord], right: &[SubmissionWord]) -> bool {
    left.len() == right.len() && left.iter().all(|item| right.contains(item))
}

fn read_drafts(core: &Core, account_id: i64) -> Result<Vec<SubmissionPreview>, CoreError> {
    let path = core
        .env()
        .data_dir
        .join(format!("submission-drafts-{account_id}.json"));
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn write_drafts(
    core: &Core,
    account_id: i64,
    drafts: &[SubmissionPreview],
) -> Result<(), CoreError> {
    let path = core
        .env()
        .data_dir
        .join(format!("submission-drafts-{account_id}.json"));
    let mut file = tempfile::NamedTempFile::new_in(&core.env().data_dir)?;
    file.write_all(&serde_json::to_vec(drafts)?)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|error| CoreError::Io(error.error))?;
    Ok(())
}

fn save_draft(core: &Core, preview: &SubmissionPreview) -> Result<(), CoreError> {
    let mut drafts = read_drafts(core, preview.account_id)?;
    drafts.retain(|draft| draft.batch_id != preview.batch_id);
    drafts.push(preview.clone());
    write_drafts(core, preview.account_id, &drafts)
}

fn remove_draft(core: &Core, preview: &SubmissionPreview) -> Result<(), CoreError> {
    let mut drafts = read_drafts(core, preview.account_id)?;
    drafts.retain(|draft| draft.batch_id != preview.batch_id);
    write_drafts(core, preview.account_id, &drafts)
}

fn same_warnings(left: &[Value], right: &[Value]) -> bool {
    let without_ids = |warnings: &[Value]| -> Vec<Value> {
        warnings
            .iter()
            .map(|warning| {
                let mut warning = warning.clone();
                if let Some(object) = warning.as_object_mut() {
                    object.remove("id");
                }
                warning
            })
            .collect()
    };
    without_ids(left) == without_ids(right)
}
