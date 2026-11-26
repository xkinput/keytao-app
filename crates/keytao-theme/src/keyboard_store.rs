//! Keyboard profile library, compilation to `keyboard.yaml`, schema recommendation,
//! and portable share / import / export.
//!
//! All paths are explicit: pass the active `keyboard.yaml` path; user sources live
//! in sibling `keyboards/<id>.yaml`.

use crate::{
    mobile_layout::{
        mobile_layout_preview_json, resolve_mobile_layout_from_yaml, MobileLayout,
        DEFAULT_KEYBOARD_ID, KEYBOARD_SOURCE_VERSION,
    },
    share::THEME_SHARE_FILE_THRESHOLD,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{write::ZlibEncoder, Compression, Decompress, FlushDecompress, Status};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use unicode_normalization::UnicodeNormalization;

pub const KEYBOARD_SHARE_PREFIX: &str = "keytao-keyboard:";
pub const KEYBOARD_SHARE_FILE_THRESHOLD: usize = THEME_SHARE_FILE_THRESHOLD;
pub const MAX_KEYBOARD_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_KEYBOARD_SHARE_TEXT_BYTES: usize = 384 * 1024;
const MAX_KEYBOARD_JSON_DEPTH: usize = 32;

const BUILT_IN_KEYBOARDS: &[(&str, &str)] = &[
    ("keytao.qwerty26", include_str!("../keyboards/qwerty26.yaml")),
    ("keytao.keytao30", include_str!("../keyboards/keytao30.yaml")),
    ("keytao.keytao26", include_str!("../keyboards/keytao26.yaml")),
    ("keytao.keydo30", include_str!("../keyboards/keydo30.yaml")),
    ("keytao.lssp_3x5", include_str!("../keyboards/lssp_3x5.yaml")),
    ("keytao.lssp_4x4", include_str!("../keyboards/lssp_4x4.yaml")),
];

static STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock_store() -> MutexGuard<'static, ()> {
    STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub built_in: bool,
    pub current: bool,
    pub has_mnemonics: bool,
    pub recommended_schemas: Vec<String>,
    pub preview_layout_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardExport {
    pub id: String,
    pub file_name: String,
    pub json: String,
    pub share_string: String,
    pub use_file: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardImportPreview {
    pub id: String,
    pub name: String,
    pub suggested_id: String,
    pub suggested_name: String,
    pub conflict: bool,
    pub built_in_conflict: bool,
    pub has_mnemonics: bool,
    pub warnings: Vec<String>,
    pub preview_layout_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardImportResult {
    pub id: String,
    pub name: String,
    pub disposition: String,
    pub warnings: Vec<String>,
}

struct StoredKeyboard {
    id: String,
    built_in: bool,
    raw_yaml: String,
    layout: MobileLayout,
}

fn keyboards_dir(keyboard_path: &Path) -> PathBuf {
    keyboard_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("keyboards")
}

fn is_built_in_id(id: &str) -> bool {
    BUILT_IN_KEYBOARDS.iter().any(|(built_in, _)| *built_in == id)
}

fn validate_keyboard_id(id: &str) -> Result<String, String> {
    let trimmed = id.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err("键盘 ID 长度必须为 1–64 字符".into());
    }
    if trimmed.contains("..") || trimmed.starts_with('.') || trimmed.ends_with('.') {
        return Err("键盘 ID 格式无效".into());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("键盘 ID 仅支持字母、数字、点、下划线和连字符".into());
    }
    Ok(trimmed.to_string())
}

fn canonical_keyboard_json(value: &Value) -> Result<String, String> {
    fn write(value: &Value, out: &mut String, depth: usize) -> Result<(), String> {
        if depth > MAX_KEYBOARD_JSON_DEPTH {
            return Err("键盘配置嵌套层数超过限制".into());
        }
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Value::Number(n) => {
                let f = n.as_f64().ok_or("键盘配置数值超出范围")?;
                if !f.is_finite() {
                    return Err("键盘配置数值必须有限".into());
                }
                out.push_str(ryu_js::Buffer::new().format_finite(f));
            }
            Value::String(s) => {
                out.push_str(
                    &serde_json::to_string(&s.nfc().collect::<String>())
                        .map_err(|e| e.to_string())?,
                );
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    write(item, out, depth + 1)?;
                }
                out.push(']');
            }
            Value::Object(map) => {
                let mut entries: Vec<_> = map
                    .iter()
                    .map(|(k, v)| (k.nfc().collect::<String>(), v))
                    .collect();
                entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
                if entries.windows(2).any(|w| w[0].0 == w[1].0) {
                    return Err("键盘配置存在 NFC 归一化冲突键名".into());
                }
                out.push('{');
                for (i, (k, v)) in entries.into_iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(&k).map_err(|e| e.to_string())?);
                    out.push(':');
                    write(v, out, depth + 1)?;
                }
                out.push('}');
            }
        }
        if out.len() > MAX_KEYBOARD_SOURCE_BYTES {
            return Err("键盘配置超过 256 KiB 上限".into());
        }
        Ok(())
    }
    let mut out = String::new();
    write(value, &mut out, 0)?;
    Ok(out)
}

fn active_keyboard_id(keyboard_path: &Path) -> String {
    let Ok(content) = fs::read_to_string(keyboard_path) else {
        return DEFAULT_KEYBOARD_ID.into();
    };
    let layout = resolve_mobile_layout_from_yaml(&content);
    layout.id
}

fn load_all_keyboards(keyboard_path: &Path) -> Vec<StoredKeyboard> {
    let mut items = Vec::new();
    for (id, yaml) in BUILT_IN_KEYBOARDS {
        let mut layout = resolve_mobile_layout_from_yaml(yaml);
        layout.id = (*id).to_string();
        items.push(StoredKeyboard {
            id: (*id).to_string(),
            built_in: true,
            raw_yaml: (*yaml).to_string(),
            layout,
        });
    }
    let dir = keyboards_dir(keyboard_path);
    if let Ok(entries) = fs::read_dir(&dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("yaml" | "yml" | "json")
                    )
            })
            .collect();
        files.sort();
        for path in files {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_built_in_id(stem) {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let mut layout = resolve_mobile_layout_from_yaml(&raw);
            if layout.rows.is_empty() {
                continue;
            }
            if layout.id == DEFAULT_KEYBOARD_ID || layout.id.is_empty() {
                layout.id = stem.to_string();
            }
            items.push(StoredKeyboard {
                id: layout.id.clone(),
                built_in: false,
                raw_yaml: raw,
                layout,
            });
        }
    }
    items
}

fn to_summary(item: &StoredKeyboard, current_id: &str) -> Result<KeyboardSummary, String> {
    let preview_layout_json =
        mobile_layout_preview_json(&item.layout).map_err(|e| format!("序列化预览布局失败: {e}"))?;
    Ok(KeyboardSummary {
        id: item.id.clone(),
        name: item.layout.name.clone(),
        description: item.layout.description.clone(),
        built_in: item.built_in,
        current: item.id == current_id,
        has_mnemonics: item.layout.has_mnemonics,
        recommended_schemas: item.layout.recommended_schemas.clone(),
        preview_layout_json,
    })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp).map_err(|e| format!("写入临时文件失败: {e}"))?;
        file.write_all(bytes)
            .map_err(|e| format!("写入文件失败: {e}"))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("替换文件失败: {e}")
    })
}

/// Compile a keyboard source document into `<userRoot>/keyboard.yaml` with
/// `keyboardId` and `keyboardHash` metadata preserved.
fn compile_keyboard_yaml(id: &str, raw_yaml: &str) -> Result<String, String> {
    let mut value: Value =
        serde_yaml::from_str(raw_yaml).map_err(|e| format!("解析键盘源失败: {e}"))?;
    let object = value
        .as_object_mut()
        .ok_or("键盘配置根节点必须是对象")?;
    let hash_source = canonical_keyboard_json(&Value::Object(object.clone()))?;
    let hash = format!("{:x}", Sha256::digest(hash_source.as_bytes()));
    object.insert("keyboardId".into(), Value::String(id.to_string()));
    object.insert("keyboardHash".into(), Value::String(hash[..16].to_string()));
    if !object.contains_key("version") {
        object.insert(
            "version".into(),
            Value::Number(KEYBOARD_SOURCE_VERSION.into()),
        );
    }
    serde_yaml::to_string(&value).map_err(|e| format!("序列化 keyboard.yaml 失败: {e}"))
}

pub fn list_keyboards(keyboard_path: &Path) -> Result<Vec<KeyboardSummary>, String> {
    let _guard = lock_store();
    let current_id = active_keyboard_id(keyboard_path);
    let items = load_all_keyboards(keyboard_path);
    items
        .iter()
        .map(|item| to_summary(item, &current_id))
        .collect()
}

pub fn apply_keyboard(keyboard_path: &Path, id: &str) -> Result<KeyboardSummary, String> {
    let _guard = lock_store();
    let items = load_all_keyboards(keyboard_path);
    let target = items
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("未找到键盘方案：{id}"))?;
    let compiled = compile_keyboard_yaml(&target.id, &target.raw_yaml)?;
    write_atomic(keyboard_path, compiled.as_bytes())?;
    to_summary(target, &target.id)
}

pub fn delete_keyboard(keyboard_path: &Path, id: &str) -> Result<(), String> {
    let _guard = lock_store();
    if is_built_in_id(id) {
        return Err("内置键盘方案不可删除".into());
    }
    let id = validate_keyboard_id(id)?;
    let dir = keyboards_dir(keyboard_path);
    let mut removed = false;
    for ext in ["yaml", "yml", "json"] {
        let candidate = dir.join(format!("{id}.{ext}"));
        if candidate.is_file() {
            fs::remove_file(&candidate).map_err(|e| format!("删除键盘配置失败: {e}"))?;
            removed = true;
        }
    }
    if !removed {
        return Err(format!("未找到自定义键盘：{id}"));
    }
    if active_keyboard_id(keyboard_path) == id {
        let items = load_all_keyboards(keyboard_path);
        if let Some(default_kb) = items.iter().find(|item| item.id == DEFAULT_KEYBOARD_ID) {
            let compiled = compile_keyboard_yaml(&default_kb.id, &default_kb.raw_yaml)?;
            write_atomic(keyboard_path, compiled.as_bytes())?;
        }
    }
    Ok(())
}

pub fn recommended_keyboard_for_schema(
    keyboard_path: &Path,
    schema_id: &str,
) -> Result<Option<KeyboardSummary>, String> {
    let _guard = lock_store();
    let normalized = schema_id.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }
    let current_id = active_keyboard_id(keyboard_path);
    let items = load_all_keyboards(keyboard_path);
    // Prefer explicit exact match (user custom keyboards first, then built-ins).
    if let Some(found) = items
        .iter()
        .filter(|item| !item.built_in)
        .chain(items.iter().filter(|item| item.built_in))
        .find(|item| {
            item.layout
                .recommended_schemas
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&normalized))
        })
    {
        return Ok(Some(to_summary(found, &current_id)?));
    }
    // Heuristic fallback: pinyin / shuangpin / wubi / generic schemas -> standard 26-key.
    if normalized.contains("pinyin")
        || normalized.contains("shuangpin")
        || normalized.contains("wanxiang")
        || normalized.contains("rime_ice")
        || normalized.contains("rime_mint")
        || normalized.contains("rime_frost")
    {
        if let Some(default_kb) = items.iter().find(|item| item.id == DEFAULT_KEYBOARD_ID) {
            return Ok(Some(to_summary(default_kb, &current_id)?));
        }
    }
    Ok(None)
}

fn portable_keyboard_value(raw_yaml: &str) -> Result<Value, String> {
    let mut value: Value =
        serde_yaml::from_str(raw_yaml).map_err(|e| format!("解析键盘配置失败: {e}"))?;
    let object = value
        .as_object_mut()
        .ok_or("键盘配置根节点必须是对象")?;
    object.remove("keyboardId");
    object.remove("keyboardHash");
    object.insert(
        "version".into(),
        Value::Number(KEYBOARD_SOURCE_VERSION.into()),
    );
    Ok(value)
}

pub fn export_keyboard(keyboard_path: &Path, id: &str) -> Result<KeyboardExport, String> {
    let _guard = lock_store();
    let items = load_all_keyboards(keyboard_path);
    let target = items
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("未找到键盘方案：{id}"))?;
    let portable = portable_keyboard_value(&target.raw_yaml)?;
    let json = canonical_keyboard_json(&portable)?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(json.as_bytes())
        .map_err(|e| e.to_string())?;
    let compressed = encoder.finish().map_err(|e| e.to_string())?;
    let share_string = format!(
        "{KEYBOARD_SHARE_PREFIX}{KEYBOARD_SOURCE_VERSION}:{}",
        URL_SAFE_NO_PAD.encode(compressed)
    );
    let use_file = share_string.len() > KEYBOARD_SHARE_FILE_THRESHOLD;
    Ok(KeyboardExport {
        id: target.id.clone(),
        file_name: format!("{}.keytao-keyboard.json", target.id),
        json,
        share_string,
        use_file,
    })
}

fn pasted_whitespace(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{200b}'..='\u{200d}' | '\u{feff}')
}

fn decode_keyboard_source(text: &str) -> Result<(Value, MobileLayout, Vec<String>), String> {
    let trimmed = text.trim_matches(pasted_whitespace);
    let raw_json = if trimmed.starts_with('{') {
        let mut cleaned = String::new();
        let (mut quoted, mut escaped) = (false, false);
        for ch in trimmed.chars() {
            if matches!(ch, '\u{200b}'..='\u{200d}' | '\u{feff}')
                || (!quoted && ch.is_whitespace())
            {
                continue;
            }
            cleaned.push(ch);
            if cleaned.len() > MAX_KEYBOARD_SHARE_TEXT_BYTES {
                return Err("键盘配置文本超过 384 KiB 上限".into());
            }
            if escaped {
                escaped = false;
            } else if quoted && ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = !quoted;
            }
        }
        cleaned
    } else {
        let mut cleaned = String::with_capacity(text.len().min(MAX_KEYBOARD_SHARE_TEXT_BYTES));
        for ch in text.chars().filter(|ch| !pasted_whitespace(*ch)) {
            cleaned.push(ch);
            if cleaned.len() > MAX_KEYBOARD_SHARE_TEXT_BYTES {
                return Err("键盘分享串超过 384 KiB 上限".into());
            }
        }
        let rest = cleaned
            .strip_prefix(KEYBOARD_SHARE_PREFIX)
            .ok_or("请粘贴有效的键盘分享串（keytao-keyboard:2:...）或 JSON")?;
        let (version_str, encoded) = rest.split_once(':').ok_or("键盘分享串缺少版本号")?;
        let version: u32 = version_str
            .parse()
            .map_err(|_| "键盘分享串版本号无效")?;
        if version > KEYBOARD_SOURCE_VERSION {
            return Err("该键盘配置需要更新版 KeyTao 才能导入".into());
        }
        if version == 0 {
            return Err("键盘分享串版本号无效".into());
        }
        let compressed = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| format!("分享串 Base64 解码失败: {e}"))?;
        let mut output = vec![0u8; MAX_KEYBOARD_SOURCE_BYTES + 1];
        let mut decoder = Decompress::new(true);
        let status = decoder
            .decompress(&compressed, &mut output, FlushDecompress::Finish)
            .map_err(|e| format!("分享串解压失败: {e}"))?;
        if decoder.total_out() > MAX_KEYBOARD_SOURCE_BYTES as u64 {
            return Err("键盘配置解压后超过 256 KiB 上限".into());
        }
        if status != Status::StreamEnd || decoder.total_in() != compressed.len() as u64 {
            return Err("键盘分享串数据不完整或已损坏".into());
        }
        output.truncate(decoder.total_out() as usize);
        String::from_utf8(output).map_err(|_| "键盘分享串不是有效的 UTF-8 文本".to_string())?
    };

    let mut value: Value =
        serde_json::from_str(&raw_json).map_err(|e| format!("键盘 JSON 解析失败: {e}"))?;
    let mut warnings = Vec::new();
    let (version, raw_id_opt) = {
        let object = value
            .as_object_mut()
            .ok_or("键盘配置根节点必须是 JSON 对象")?;
        object.remove("keyboardId");
        object.remove("keyboardHash");
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(KEYBOARD_SOURCE_VERSION as u64);
        let raw_id_opt = object
            .get("meta")
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        (version, raw_id_opt)
    };
    if version > KEYBOARD_SOURCE_VERSION as u64 {
        return Err("该键盘配置版本高于当前支持版本（v2）".into());
    }
    let yaml_text = serde_yaml::to_string(&value).map_err(|e| e.to_string())?;
    let layout = resolve_mobile_layout_from_yaml(&yaml_text);
    if layout.rows.is_empty() {
        return Err("键盘配置缺少有效的按键行（rows）".into());
    }
    let raw_id = raw_id_opt.as_deref().unwrap_or(&layout.id);
    if validate_keyboard_id(raw_id).is_err() {
        warnings.push("原键盘 ID 不规范，导入时将自动生成新 ID".into());
    }
    Ok((value, layout, warnings))
}

fn suggest_non_conflicting_id(base_id: &str, existing: &BTreeSet<String>) -> String {
    let cleaned: String = base_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = cleaned
        .strip_prefix("keytao.")
        .or_else(|| cleaned.strip_prefix("my."))
        .unwrap_or(&cleaned)
        .trim_matches(|c| c == '.' || c == '-');
    let slug = if slug.is_empty() { "custom" } else { slug };
    let candidate = format!("my.{slug}");
    if !existing.contains(&candidate) {
        return candidate;
    }
    for index in 2..1000 {
        let next = format!("my.{slug}-{index}");
        if !existing.contains(&next) {
            return next;
        }
    }
    format!("my.{slug}-copy")
}

pub fn preview_keyboard_import(
    keyboard_path: &Path,
    text: &str,
) -> Result<KeyboardImportPreview, String> {
    let _guard = lock_store();
    let (_value, layout, warnings) = decode_keyboard_source(text)?;
    let existing: BTreeSet<String> = load_all_keyboards(keyboard_path)
        .into_iter()
        .map(|k| k.id)
        .collect();
    let valid_id = validate_keyboard_id(&layout.id).ok();
    let id = valid_id.clone().unwrap_or_else(|| "my.imported".into());
    let built_in_conflict = is_built_in_id(&id);
    let conflict = built_in_conflict || existing.contains(&id) || valid_id.is_none();
    let suggested_id = if conflict {
        suggest_non_conflicting_id(&id, &existing)
    } else {
        id.clone()
    };
    let suggested_name = if conflict {
        format!("{}（副本）", layout.name)
    } else {
        layout.name.clone()
    };
    let preview_layout_json =
        mobile_layout_preview_json(&layout).map_err(|e| format!("序列化预览失败: {e}"))?;
    Ok(KeyboardImportPreview {
        id,
        name: layout.name,
        suggested_id,
        suggested_name,
        conflict,
        built_in_conflict,
        has_mnemonics: layout.has_mnemonics,
        warnings,
        preview_layout_json,
    })
}

pub fn import_keyboard(
    keyboard_path: &Path,
    text: &str,
    overwrite: bool,
) -> Result<KeyboardImportResult, String> {
    let _guard = lock_store();
    let (mut value, layout, warnings) = decode_keyboard_source(text)?;
    let existing: BTreeSet<String> = load_all_keyboards(keyboard_path)
        .into_iter()
        .map(|k| k.id)
        .collect();
    let valid_id = validate_keyboard_id(&layout.id).ok();
    let source_id = valid_id.clone().unwrap_or_else(|| "my.imported".into());
    let built_in_conflict = is_built_in_id(&source_id);
    let exists = existing.contains(&source_id);

    let (final_id, final_name, disposition) = if !exists && !built_in_conflict && valid_id.is_some()
    {
        (source_id, layout.name.clone(), "imported".to_string())
    } else if overwrite && !built_in_conflict && valid_id.is_some() {
        (source_id, layout.name.clone(), "overwritten".to_string())
    } else {
        let next_id = suggest_non_conflicting_id(&source_id, &existing);
        let next_name = format!("{}（副本）", layout.name);
        (next_id, next_name, "copied".to_string())
    };

    let object = value
        .as_object_mut()
        .ok_or("键盘配置根节点必须是对象")?;
    object.insert(
        "version".into(),
        Value::Number(KEYBOARD_SOURCE_VERSION.into()),
    );
    let meta = object
        .entry("meta")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(meta_obj) = meta.as_object_mut() {
        meta_obj.insert("id".into(), Value::String(final_id.clone()));
        meta_obj.insert("name".into(), Value::String(final_name.clone()));
    }
    let yaml = serde_yaml::to_string(&value).map_err(|e| format!("序列化键盘源失败: {e}"))?;
    let target_file = keyboards_dir(keyboard_path).join(format!("{final_id}.yaml"));
    write_atomic(&target_file, yaml.as_bytes())?;

    if disposition == "overwritten" && active_keyboard_id(keyboard_path) == final_id {
        let compiled = compile_keyboard_yaml(&final_id, &yaml)?;
        write_atomic(keyboard_path, compiled.as_bytes())?;
    }

    Ok(KeyboardImportResult {
        id: final_id,
        name: final_name,
        disposition,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_keyboard_path(name: &str) -> (PathBuf, PathBuf) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-kb-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyboard.yaml");
        (dir, path)
    }

    #[test]
    fn all_six_built_in_keyboards_resolve_with_expected_mnemonics_and_recommendations() {
        let (dir, path) = temp_keyboard_path("builtins");
        let list = list_keyboards(&path).unwrap();
        assert_eq!(list.len(), 6);
        let ids: Vec<_> = list.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "keytao.qwerty26",
                "keytao.keytao30",
                "keytao.keytao26",
                "keytao.keydo30",
                "keytao.lssp_3x5",
                "keytao.lssp_4x4",
            ]
        );
        let by_id = |id: &str| list.iter().find(|k| k.id == id).unwrap();
        assert!(!by_id("keytao.qwerty26").has_mnemonics);
        assert!(by_id("keytao.keytao30").has_mnemonics);
        assert!(by_id("keytao.keytao26").has_mnemonics);
        assert!(by_id("keytao.keydo30").has_mnemonics);
        assert!(!by_id("keytao.lssp_3x5").has_mnemonics);
        assert!(!by_id("keytao.lssp_4x4").has_mnemonics);

        // Verify schema recommendations.
        assert_eq!(
            recommended_keyboard_for_schema(&path, "keytao")
                .unwrap()
                .unwrap()
                .id,
            "keytao.keytao30"
        );
        assert_eq!(
            recommended_keyboard_for_schema(&path, "keydo")
                .unwrap()
                .unwrap()
                .id,
            "keytao.keydo30"
        );
        assert_eq!(
            recommended_keyboard_for_schema(&path, "triple_pinyin_lssp")
                .unwrap()
                .unwrap()
                .id,
            "keytao.lssp_3x5"
        );
        assert_eq!(
            recommended_keyboard_for_schema(&path, "rime_mint")
                .unwrap()
                .unwrap()
                .id,
            "keytao.qwerty26"
        );
        assert_eq!(
            recommended_keyboard_for_schema(&path, "custom_pinyin_xyz")
                .unwrap()
                .unwrap()
                .id,
            "keytao.qwerty26"
        );

        // Verify keytao30 bottom row has no "键" key and starts with "数".
        let k30 = resolve_mobile_layout_from_yaml(
            BUILT_IN_KEYBOARDS
                .iter()
                .find(|(id, _)| *id == "keytao.keytao30")
                .unwrap()
                .1,
        );
        assert_eq!(k30.rows.len(), 4);
        assert_eq!(k30.rows[3][0].label, "数");
        assert!(k30.rows[3].iter().all(|k| k.label != "键"));

        // Verify lssp_3x5 has embedded numberRow and rowSpan=3 stacked left symbol column.
        let l35 = resolve_mobile_layout_from_yaml(
            BUILT_IN_KEYBOARDS
                .iter()
                .find(|(id, _)| *id == "keytao.lssp_3x5")
                .unwrap()
                .1,
        );
        assert_eq!(l35.number_row, crate::mobile_layout::MobileNumberRowMode::Embedded);
        assert_eq!(l35.rows[1][0].row_span, Some(3));
        assert_eq!(l35.rows[1][0].stack.len(), 4);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn apply_export_import_and_delete_round_trip() {
        let (dir, path) = temp_keyboard_path("roundtrip");
        let applied = apply_keyboard(&path, "keytao.keydo30").unwrap();
        assert_eq!(applied.id, "keytao.keydo30");
        assert!(applied.current);
        assert!(path.is_file());

        let exported = export_keyboard(&path, "keytao.keydo30").unwrap();
        assert!(exported.share_string.starts_with("keytao-keyboard:2:"));
        assert_eq!(exported.file_name, "keytao.keydo30.keytao-keyboard.json");

        // Built-in conflict imports as a copy.
        let preview = preview_keyboard_import(&path, &exported.share_string).unwrap();
        assert!(preview.conflict);
        assert!(preview.built_in_conflict);
        assert!(preview.has_mnemonics);
        assert_eq!(preview.suggested_id, "my.keydo30");

        let imported = import_keyboard(&path, &exported.share_string, false).unwrap();
        assert_eq!(imported.id, "my.keydo30");
        assert_eq!(imported.disposition, "copied");

        let list = list_keyboards(&path).unwrap();
        assert_eq!(list.len(), 7);
        assert!(list.iter().any(|k| k.id == "my.keydo30" && !k.built_in));

        // Apply imported then delete it -> falls back to default.
        apply_keyboard(&path, "my.keydo30").unwrap();
        delete_keyboard(&path, "my.keydo30").unwrap();
        let after = list_keyboards(&path).unwrap();
        assert_eq!(after.len(), 6);
        assert!(after
            .iter()
            .find(|k| k.id == DEFAULT_KEYBOARD_ID)
            .unwrap()
            .current);

        let _ = fs::remove_dir_all(dir);
    }
}
