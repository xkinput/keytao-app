use crate::{
    Core, CoreError, CoreEvent, events::InstallProgress, scheme_files::*, scheme_host::SchemeHost,
    scheme_types::AddonSchemaStatus,
};
use std::path::{Path, PathBuf};

pub const EASY_EN_ADDON_ID: &str = "easy_en";
const EASY_EN_ADDON_VERSION: &str = "0.10.1";

pub fn validate_addon_schema_id(id: &str) -> Result<&str, String> {
    match id.trim() {
        EASY_EN_ADDON_ID => Ok(EASY_EN_ADDON_ID),
        _ => Err(format!("不支持的附加方案：{id}")),
    }
}

pub fn addon_schema_source_files(id: &str) -> [(&'static str, String); 4] {
    [
        ("easy_en.schema.yaml", format!("{id}.schema.yaml")),
        ("easy_en.dict.yaml", format!("{id}.dict.yaml")),
        ("easy_en.custom.yaml", format!("{id}.custom.yaml")),
        ("lua/easy_en.lua", format!("lua/{id}.lua")),
    ]
}

pub fn deployed_english_schema_available(root: &Path) -> bool {
    let schemas = [
        "default.custom.yaml",
        "default-custom.yaml",
        "default.yaml",
        "build/default.yaml",
    ]
    .into_iter()
    .filter_map(|name| std::fs::read_to_string(root.join(name)).ok())
    .map(|text| parse_schema_list(&text))
    .find(|schemas| !schemas.is_empty())
    .unwrap_or_default();
    schemas
        .iter()
        .filter(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        })
        .any(|id| {
            let path = root.join("build").join(format!("{id}.schema.yaml"));
            let Some(schema) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_yaml::from_slice::<serde_yaml::Value>(&bytes).ok())
            else {
                return false;
            };
            let name = schema
                .get("schema")
                .and_then(|v| v.get("name"))
                .and_then(serde_yaml::Value::as_str)
                .unwrap_or("");
            keytao_core::english_mode::is_english_schema(id, name)
        })
}

pub fn addon_schema_status_at(root: &Path, id: &str) -> AddonSchemaStatus {
    let configured = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        .is_some_and(|content| {
            parse_schema_list(&content)
                .iter()
                .any(|schema| schema == id)
        });
    let installed = configured
        && addon_schema_source_files(id)
            .iter()
            .all(|(_, relative)| root.join(relative).is_file());
    let deployed = installed
        && root
            .join("build")
            .join(format!("{id}.schema.yaml"))
            .is_file();
    AddonSchemaStatus {
        installed,
        deployed,
        version: EASY_EN_ADDON_VERSION.into(),
        english_available: deployed_english_schema_available(root),
    }
}

pub fn update_addon_schema_list_content(
    existing: &str,
    id: &str,
    installed: bool,
) -> Result<String, String> {
    let mut value = serde_yaml::from_str::<serde_yaml::Value>(&existing)
        .map_err(|error| format!("解析 default custom 配置失败: {error}"))?;
    let mapping = value
        .as_mapping_mut()
        .ok_or("default custom 配置根节点必须是 YAML mapping")?;
    let patch = yaml_child_mapping(mapping, "patch", "patch 必须是 YAML mapping")?;

    let mut schemas = parse_schema_list(&existing);
    schemas.retain(|schema| schema != id);
    if installed {
        schemas.push(id.to_string());
    }
    let schema_list = serde_yaml::Value::Sequence(
        schemas
            .into_iter()
            .map(|schema| {
                let mut entry = serde_yaml::Mapping::new();
                entry.insert(
                    serde_yaml::Value::String("schema".into()),
                    serde_yaml::Value::String(schema),
                );
                serde_yaml::Value::Mapping(entry)
            })
            .collect(),
    );
    patch.insert(serde_yaml::Value::String("schema_list".into()), schema_list);

    let mut content = serde_yaml::to_string(&value)
        .map_err(|error| format!("序列化 default custom 配置失败: {error}"))?;
    if let Some(stripped) = content.strip_prefix("---\n") {
        content = stripped.to_string();
    }
    Ok(content)
}

pub fn write_addon_schema_list(root: &Path, id: &str, installed: bool) -> Result<(), String> {
    let path = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
        .ok_or("请先安装键道方案")?;
    let existing = std::fs::read_to_string(&path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    let content = update_addon_schema_list_content(&existing, id, installed)?;
    write_file_atomic(&path, content.as_bytes())
        .map_err(|error| format!("写入附加方案列表失败: {error}"))
}

pub fn addon_schema_status(
    _core: &Core,
    root: impl FnOnce() -> Result<PathBuf, String>,
    id: String,
) -> Result<AddonSchemaStatus, CoreError> {
    let result: Result<_, String> = (|| {
        let id = validate_addon_schema_id(&id)?;
        let root = root()?;
        Ok(addon_schema_status_at(&root, id))
    })();
    result.map_err(CoreError::Other)
}

#[cfg(not(target_os = "android"))]
pub fn bundled_addon_schema_dir(core: &Core, id: &str) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    {
        let resource_dir = &core.env().resource_dir;
        candidates.extend([
            resource_dir.join("addon-schemas").join(id),
            resource_dir
                .join("resources")
                .join("addon-schemas")
                .join(id),
        ]);
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/addon-schemas")
            .join(id),
    );
    candidates
        .into_iter()
        .find(|candidate| candidate.join("easy_en.schema.yaml").is_file())
        .ok_or_else(|| format!("应用包中缺少附加方案资源：{id}"))
}

#[cfg(not(target_os = "android"))]
fn install_addon_schema_files(core: &Core, root: &Path, id: &str) -> Result<(), String> {
    let source = bundled_addon_schema_dir(core, id)?;
    for (source_relative, destination_relative) in addon_schema_source_files(id) {
        let bytes = std::fs::read(source.join(source_relative))
            .map_err(|error| format!("读取附加方案资源 {source_relative} 失败: {error}"))?;
        let destination = root.join(destination_relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
        }
        write_file_atomic(&destination, &bytes)
            .map_err(|error| format!("写入 {} 失败: {error}", destination.display()))?;
    }
    Ok(())
}

fn remove_path_if_present(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|error| format!("删除 {} 失败: {error}", path.display()))
}

fn remove_addon_schema_files(root: &Path, id: &str) -> Result<(), String> {
    for (_, relative) in addon_schema_source_files(id) {
        remove_path_if_present(&root.join(relative))?;
    }
    for dir in [root.to_path_buf(), root.join("build")] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("读取 {} 失败: {error}", dir.display())),
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let remove = if dir == root {
                name.starts_with(&format!("{id}.userdb"))
            } else {
                name.starts_with(&format!("{id}."))
            };
            if remove {
                remove_path_if_present(&entry.path())?;
            }
        }
    }
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn reset_mobile_english_mode(root: &Path) -> Result<bool, String> {
    let path = android_ime_config_path(root);
    let mut config = read_android_ime_config(&path);
    if config
        .get("englishMode")
        .and_then(serde_json::Value::as_str)
        != Some("schema")
    {
        return Ok(false);
    }
    config.insert(
        "englishMode".into(),
        serde_json::Value::String("ascii".into()),
    );
    let content = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("序列化移动端输入设置失败: {error}"))?;
    write_file_atomic(&path, &content)
        .map_err(|error| format!("写入 {} 失败: {error}", path.display()))?;
    Ok(true)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn reset_mobile_english_mode(_root: &Path) -> Result<bool, String> {
    use keytao_core::english_mode::{EnglishMode, read_english_mode, write_english_mode};
    let changed = read_english_mode(_root) != EnglishMode::Ascii;
    if changed {
        write_english_mode(_root, EnglishMode::Ascii)?;
    }
    Ok(changed)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn android_ime_config_path(root: &Path) -> PathBuf {
    #[cfg(target_os = "ios")]
    {
        root.join("ios_ime.json")
    }
    #[cfg(target_os = "android")]
    {
        root.join("android_ime.json")
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn read_android_ime_config(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

pub async fn addon_schema_install(
    core: &Core,
    id: String,
    host: &impl SchemeHost,
) -> Result<AddonSchemaStatus, CoreError> {
    let result: Result<_, String> = async {
        let id = validate_addon_schema_id(&id)?;
        let root = host.user_root()?;
        let base = keytao_core::schema_install_state(&root);
        if !base.installed {
            return Err("请先安装键道方案".into());
        }
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-copy".into(),
            percent: 20,
            message: "正在复制附加方案 English...".into(),
        }));
        #[cfg(not(target_os = "android"))]
        install_addon_schema_files(core, &root, id)?;
        #[cfg(target_os = "android")]
        host.copy_addon_assets(id)?;
        write_addon_schema_list(&root, id, true)?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-deploy".into(),
            percent: 55,
            message: "正在部署键道方案与 English...".into(),
        }));
        host.deploy().await?;
        let status = addon_schema_status_at(&root, id);
        if !status.deployed {
            return Err("附加方案文件已安装，但部署产物缺失".into());
        }
        #[cfg(target_os = "windows")]
        host.publish_english_addon()?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: "附加方案 English 已安装并部署".into(),
        }));
        Ok(status)
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn addon_schema_uninstall(
    core: &Core,
    id: String,
    host: &impl SchemeHost,
) -> Result<AddonSchemaStatus, CoreError> {
    let result: Result<_, String> = async {
        let id = validate_addon_schema_id(&id)?;
        let root = host.user_root()?;
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "addon-uninstall".into(),
            percent: 20,
            message: "正在卸载附加方案 English...".into(),
        }));
        write_addon_schema_list(&root, id, false)?;
        remove_addon_schema_files(&root, id)?;
        let reset_english_mode = reset_mobile_english_mode(&root)?;
        #[cfg(target_os = "windows")]
        {
            let files = addon_schema_source_files(id)
                .into_iter()
                .map(|(_, relative)| PathBuf::from(relative))
                .collect::<Vec<_>>();
            host.remove_public_schema(id, &files)?;
            host.publish_english_settings(false)?;
        }
        if keytao_core::schema_install_state(&root).installed {
            host.deploy().await?;
        } else {
            let _ = host.write_reload_stamp(&root)?;
        }
        if reset_english_mode {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: "addon-reset-mode".into(),
                percent: 90,
                message: "英文模式已切回 ASCII 模式".into(),
            }));
        }
        core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: "附加方案 English 已卸载".into(),
        }));
        Ok(addon_schema_status_at(&root, id))
    }
    .await;
    result.map_err(CoreError::Other)
}
