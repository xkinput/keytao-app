//! File selection and isolation for downloaded Snow Jiandao and LSSP packages.

use crate::{
    downloaded_addon::Addon,
    wanxiang::{self, InstalledFile, RemotePackage},
};
use serde::Deserialize;
use std::{
    fs::{self, File},
    path::Path,
};

const SNOW_REQUIRED: &[&str] = &[
    "snow_jiandao.schema.yaml",
    "snow_pinyin.schema.yaml",
    "snow_bihua.schema.yaml",
    "snow_jiandao_bihua.schema.yaml",
    "snow_jiandao_jianpin.schema.yaml",
    "snow_jiandao_chaifen.schema.yaml",
    "snow_sanpin.schema.yaml",
    "snow_radical_pinyin.schema.yaml",
    "snow_radical_pinyin.dict.yaml",
    "snow_pinyin.dict.yaml",
    "snow_pinyin.base.dict.yaml",
    "snow_pinyin.ext.dict.yaml",
    "snow_pinyin.tencent.dict.yaml",
    "snow_pinyin.user_pack.dict.yaml",
    "snow_bihua.dict.yaml",
    "snow_jiandao_chaifen.dict.yaml",
    "snow_jiandao.fixed.txt",
    "lua/snow/snow.lua",
    "lua/snow/radical_jiandao.txt",
];
const LSSP_REQUIRED: &[&str] = &[
    "triple_pinyin_lssp.schema.yaml",
    "triple_pinyin.yaml",
    "triple_pinyin.dict.yaml",
    "triple_pinyin.additional.dict.yaml",
    "triple_pinyin.ddpy.dict.yaml",
    "lssp_stroke.schema.yaml",
    "lssp_stroke.dict.yaml",
    "triple_pinyin_lssp-LICENSE.txt",
    "lssp-stroke-LICENSE.txt",
];
const LSSP_LUA: &[&str] = &[
    "limited",
    "lssp_processor",
    "triple_pinyin_lssp",
    "aux_code_stroke",
];
const LSSP_TABLES: &[&str] = &[
    "zi",
    "jichu",
    "lianxiang",
    "duoyin",
    "shici",
    "diming",
    "yixue",
    "huaxue",
    "yaopin",
    "mingren",
    "yiren",
    "wuzhong",
    "renming",
    "fangyan",
];

pub(crate) static SNOW: Addon = Addon {
    id: "snow_jiandao",
    dictionary: "snow_pinyin",
    name: "冰雪键道",
    manifest: ".keytao-rime-snow.json",
    required: SNOW_REQUIRED,
    extra_archives: &[],
    installable: |name| Package::Snow.installable(name),
    extract: |archive, stage, package| Package::Snow.extract(archive, stage, package),
};
pub(crate) static LSSP: Addon = Addon {
    id: "triple_pinyin_lssp",
    dictionary: "triple_pinyin",
    name: "李氏三拼",
    manifest: ".keytao-rime-lssp.json",
    required: LSSP_REQUIRED,
    extra_archives: &[
        (
            "wanxiang.zip",
            "https://github.com/amzxyz/rime-wanxiang/releases/download/v17.0.0/rime-wanxiang-base.zip",
        ),
        (
            "stroke.zip",
            "https://codeload.github.com/rime/rime-stroke/zip/refs/heads/master",
        ),
    ],
    installable: |name| Package::Lssp.installable(name),
    extract: |archive, stage, package| Package::Lssp.extract(archive, stage, package),
};

#[derive(Clone, Copy)]
pub(crate) enum Package {
    Snow,
    Lssp,
}

// Only these sections are decoded: upstream LSSP uses repeated __patch keys elsewhere.
#[derive(Deserialize, Default)]
struct Schema {
    #[serde(default)]
    schema: SchemaInfo,
    #[serde(default)]
    engine: Engine,
}
#[derive(Deserialize, Default)]
struct SchemaInfo {
    #[serde(default)]
    dependencies: Vec<String>,
}
#[derive(Deserialize, Default)]
struct Engine {
    #[serde(default)]
    processors: Vec<String>,
    #[serde(default)]
    translators: Vec<String>,
    #[serde(default)]
    filters: Vec<String>,
}

impl Package {
    pub(crate) fn addon(self) -> &'static Addon {
        match self {
            Self::Snow => &SNOW,
            Self::Lssp => &LSSP,
        }
    }
    fn source(self) -> &'static str {
        match self {
            Self::Snow => "https://github.com/rimeinn/rime-snow-pinyin",
            Self::Lssp => "https://github.com/arsenali/rime-triple-pinyin-lssp",
        }
    }
    fn destination(self, name: &str) -> Option<String> {
        match self {
            Self::Snow => {
                let renamed = name
                    .replace("radical_pinyin", "snow_radical_pinyin")
                    .replace("snow_pinyin.user", "snow_pinyin.user_pack");
                if SNOW_REQUIRED.contains(&renamed.as_str())
                    || name.starts_with("lua/snow/")
                        && (name.ends_with(".lua") || name.ends_with(".txt"))
                {
                    Some(renamed)
                } else {
                    None
                }
            }
            Self::Lssp => {
                let name = name.strip_prefix("rime-triple-pinyin-lssp-main/")?;
                if name == "LICENSE" {
                    return Some("triple_pinyin_lssp-LICENSE.txt".into());
                }
                if matches!(
                    name,
                    "triple_pinyin_lssp.schema.yaml"
                        | "triple_pinyin.yaml"
                        | "triple_pinyin.dict.yaml"
                        | "triple_pinyin.additional.dict.yaml"
                        | "triple_pinyin.ddpy.dict.yaml"
                ) {
                    return Some(name.into());
                }
                let module = name.strip_prefix("lua/")?.strip_suffix(".lua")?;
                LSSP_LUA
                    .contains(&module)
                    .then(|| format!("lua/lssp/{module}.lua"))
            }
        }
    }
    fn installable(self, name: &str) -> bool {
        if name == format!("{}-NOTICE.txt", self.addon().id)
            || self.addon().required.contains(&name)
        {
            return true;
        }
        match self {
            Self::Snow => self.destination(name).as_deref() == Some(name),
            Self::Lssp => {
                if let Some(table) = name
                    .strip_prefix("lssp-dicts/")
                    .and_then(|n| n.strip_suffix(".dict.yaml"))
                {
                    return LSSP_TABLES.contains(&table);
                }
                name.strip_prefix("lua/lssp/")
                    .and_then(|n| n.strip_suffix(".lua"))
                    .is_some_and(|m| LSSP_LUA.contains(&m))
            }
        }
    }
    fn adapt(self, name: &str, path: &Path) -> Result<(), String> {
        if !name.ends_with(".yaml") && !name.ends_with(".lua") {
            return Ok(());
        }
        let mut text = fs::read_to_string(path)
            .map_err(|e| e.to_string())?
            .replace("\r\n", "\n");
        match self {
            Self::Snow => {
                text = text
                    .replace("radical_pinyin", "snow_radical_pinyin")
                    .replace("/rime_snow_radical_pinyin", "/rime_radical_pinyin")
                    // The upstream seed is package data, not a user's protected dictionary.
                    .replace("snow_pinyin.user", "snow_pinyin.user_pack");
            }
            Self::Lssp => {
                if name.ends_with(".schema.yaml") {
                    for module in LSSP_LUA {
                        text = text.replace(&format!("@*{module}"), &format!("@*lssp.{module}"));
                    }
                    text = text
                        .replace("    - pinyin_simp\n", "")
                        .replace("    - reverse_lookup_translator@pinyin_lookup\n", "")
                        .replace("    - stroke\n", "    - lssp_stroke\n")
                        .replace("dictionary: stroke", "dictionary: lssp_stroke")
                        // Keep the unused lookup block because stroke shares its YAML anchor.
                        .replace("dictionary: pinyin_simp", "dictionary: lssp_stroke")
                        .replace("schema_id: stroke", "schema_id: lssp_stroke")
                        .replace("  import_preset: symbols", "  import_preset: default");
                    text = text
                        .lines()
                        .filter(|line| !line.starts_with("    pinyin_lookup:"))
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n";
                    if name == "triple_pinyin_lssp.schema.yaml" {
                        if let Some((without_grammar, _)) = text.rsplit_once("\n__patch:") {
                            text = without_grammar.to_owned() + "\n";
                        }
                    }
                } else if name == "triple_pinyin.dict.yaml" {
                    text = text.replace("vocabulary: essay-zh-hans", "vocabulary: essay");
                    text = text
                        .lines()
                        .filter(|line| !line.trim_start().starts_with("- triple_pinyin.zianyin"))
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n";
                    text = text.replace("- dicts/", "- lssp-dicts/");
                } else if name == "lssp_stroke.dict.yaml" {
                    text = text.replace("name: stroke", "name: lssp_stroke");
                } else if name.ends_with("aux_code_stroke.lua") {
                    text = text.replace("/stroke.dict.yaml", "/lssp_stroke.dict.yaml");
                }
                if name == "lssp_stroke.schema.yaml" {
                    let mut schema: serde_yaml::Value =
                        serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
                    schema["schema"]["dependencies"] = serde_yaml::Value::Sequence(vec![]);
                    if let Some(translators) = schema
                        .get_mut("engine")
                        .and_then(|e| e.get_mut("translators"))
                        .and_then(serde_yaml::Value::as_sequence_mut)
                    {
                        translators.retain(|v| v.as_str() != Some("reverse_lookup_translator"));
                    }
                    schema.as_mapping_mut().unwrap().remove("reverse_lookup");
                    text = serde_yaml::to_string(&schema).map_err(|e| e.to_string())?;
                }
            }
        }
        fs::write(path, text).map_err(|e| e.to_string())
    }
    pub(crate) fn extract(
        self,
        archive: &Path,
        stage: &Path,
        package: &RemotePackage,
    ) -> Result<Vec<InstalledFile>, String> {
        if matches!(self, Self::Lssp) {
            let sha = package
                .source
                .strip_prefix(&format!("{}/commit/", self.source()))
                .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or("李氏三拼安装来源缺少有效提交号")?;
            let zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if zip.comment() != sha.as_bytes() {
                return Err("李氏三拼 main 分支在下载期间发生变化，请重新安装".into());
            }
        }
        let mut files = wanxiang::extract_package_with_limit(
            archive,
            stage,
            |name| self.destination(name),
            |name, path| self.adapt(name, path),
            128 * 1024 * 1024,
        )?;
        if matches!(self, Self::Lssp) {
            let work = archive.parent().ok_or("附加方案归档目录缺失")?;
            files.extend(wanxiang::extract_package_with_limit(
                &work.join("wanxiang.zip"),
                stage,
                |name| {
                    let table = name.strip_prefix("dicts/")?.strip_suffix(".dict.yaml")?;
                    LSSP_TABLES
                        .contains(&table)
                        .then(|| format!("lssp-dicts/{table}.dict.yaml"))
                },
                |_, _| Ok(()),
                256 * 1024 * 1024,
            )?);
            files.extend(wanxiang::extract_package_with_limit(
                &work.join("stroke.zip"),
                stage,
                |name| match name.strip_prefix("rime-stroke-master/")? {
                    "stroke.schema.yaml" => Some("lssp_stroke.schema.yaml".into()),
                    "stroke.dict.yaml" => Some("lssp_stroke.dict.yaml".into()),
                    "LICENSE" => Some("lssp-stroke-LICENSE.txt".into()),
                    _ => None,
                },
                |name, path| self.adapt(name, path),
                16 * 1024 * 1024,
            )?);
            for table in LSSP_TABLES {
                if !stage
                    .join(format!("lssp-dicts/{table}.dict.yaml"))
                    .is_file()
                {
                    return Err(format!("李氏三拼依赖包缺少词库：{table}"));
                }
            }
            for module in LSSP_LUA {
                if !stage.join(format!("lua/lssp/{module}.lua")).is_file() {
                    return Err(format!("李氏三拼缺少 Lua 模块：{module}"));
                }
            }
        }
        for name in self.addon().required {
            if !wanxiang::safe_target(stage, name)?.is_file() {
                return Err(format!("{}发布包缺少必要文件：{name}", self.addon().name));
            }
        }
        for file in &files {
            if !self.installable(&file.path) {
                return Err(format!("附加方案文件不在安装范围：{}", file.path));
            }
            if file.path.ends_with(".schema.yaml") {
                let schema: Schema = serde_yaml::from_reader(
                    File::open(stage.join(&file.path)).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                for dependency in schema.schema.dependencies {
                    if !wanxiang::safe_target(stage, &format!("{dependency}.schema.yaml"))?
                        .is_file()
                    {
                        return Err(format!(
                            "{}发布包缺少依赖方案：{dependency}",
                            self.addon().name
                        ));
                    }
                }
                for component in schema
                    .engine
                    .processors
                    .iter()
                    .chain(&schema.engine.translators)
                    .chain(&schema.engine.filters)
                {
                    if let Some((kind, module)) = component.split_once("@*") {
                        if kind.starts_with("lua_") {
                            let module = module.split(['*', '@']).next().unwrap().replace('.', "/");
                            let namespace = match self {
                                Self::Snow => "snow/",
                                Self::Lssp => "lssp/",
                            };
                            if !module.starts_with(namespace)
                                || !wanxiang::safe_target(stage, &format!("lua/{module}.lua"))?
                                    .is_file()
                            {
                                return Err(format!(
                                    "{}缺少 Lua 模块：{module}",
                                    self.addon().name
                                ));
                            }
                        }
                    }
                }
            }
            if file.path.ends_with(".dict.yaml") && !file.path.contains('/') {
                let text = fs::read_to_string(stage.join(&file.path)).map_err(|e| e.to_string())?;
                let header = text
                    .split_once("\n...")
                    .map(|(h, _)| h)
                    .ok_or("附加方案词库缺少 YAML 结束标记")?;
                let value: serde_yaml::Value =
                    serde_yaml::from_str(header).map_err(|e| e.to_string())?;
                for table in value
                    .get("import_tables")
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                {
                    let table = table.as_str().ok_or("附加方案词库引用无效")?;
                    if !wanxiang::safe_target(stage, &format!("{table}.dict.yaml"))?.is_file() {
                        return Err(format!("{}发布包缺少词库：{table}", self.addon().name));
                    }
                }
            }
        }
        let name = format!("{}-NOTICE.txt", self.addon().id);
        let license = match self {
            Self::Snow => "GPL-3.0",
            Self::Lssp => "LGPL-3.0",
        };
        let mut notice = format!(
            "{} {}\nLicense: {license}\nSource: {}\nUpstream: {}\nCopyright: upstream authors and contributors\nKeyTao integration: package-owned dependencies; host settings and alternative layouts omitted.\nDownloaded at runtime; not bundled with KeyTao.\nUser customizations and learned dictionaries are preserved during uninstall.\n",
            self.addon().name,
            package.version,
            package.source,
            self.source()
        );
        if matches!(self, Self::Lssp) {
            notice.push_str("Dictionary source: https://github.com/amzxyz/rime-wanxiang/releases/tag/v17.0.0 (CC-BY-4.0)\nDictionary license: https://github.com/amzxyz/rime-wanxiang/blob/v17.0.0/LICENSE\nStroke source: https://github.com/rime/rime-stroke (LGPL-3.0)\nKeyTao integration: dictionaries copied into lssp-dicts; Lua modules into lua/lssp; stroke renamed lssp_stroke; optional zianyin, language model and pinyin reverse lookup omitted; punctuation uses default.\n");
        }
        fs::write(stage.join(&name), notice).map_err(|e| e.to_string())?;
        let (sha256, size) = wanxiang::fingerprint(&stage.join(&name))?;
        files.push(InstalledFile {
            path: name,
            sha256,
            size,
        });
        Ok(files)
    }
}

#[cfg(test)]
#[path = "shape_addons_tests.rs"]
mod tests;
