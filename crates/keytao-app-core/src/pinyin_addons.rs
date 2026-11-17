//! Mint and Frost source selection and package-local dependency rewriting.

use crate::{
    downloaded_addon::Addon,
    wanxiang::{self, InstalledFile, RemotePackage},
};
use serde::Deserialize;
use std::{
    fs::{self, File},
    path::Path,
};

#[derive(Clone, Copy)]
pub(crate) enum Package {
    Mint,
    Frost,
}

const MINT_REQUIRED: &[&str] = &[
    "rime_mint.schema.yaml",
    "rime_mint.dict.yaml",
    "wubi98_mint.schema.yaml",
    "wubi98_mint.dict.yaml",
    "rime_mint_melt_eng.schema.yaml",
    "rime_mint_melt_eng.dict.yaml",
    "rime_mint_radical_pinyin.schema.yaml",
    "rime_mint_radical_pinyin.dict.yaml",
    "rime_mint_stroke.schema.yaml",
    "rime_mint_stroke.dict.yaml",
    "rime_mint_symbols.yaml",
    "rime_mint-dicts/rime_ice.cn_en.txt",
    "rime_mint-LICENSE.txt",
    "opencc/rime_mint_emoji.json",
    "opencc/rime_mint_emoji.txt",
    "opencc/rime_mint_others.txt",
    "opencc/rime_mint_spoken.txt",
];
const FROST_REQUIRED: &[&str] = &[
    "rime_frost.schema.yaml",
    "rime_frost.dict.yaml",
    "rime_frost_aux.schema.yaml",
    "rime_frost_aux.dict.yaml",
    "rime_frost_melt_eng.schema.yaml",
    "rime_frost_melt_eng.dict.yaml",
    "rime_frost_radical_pinyin.schema.yaml",
    "rime_frost_radical_pinyin.dict.yaml",
    "rime_frost_symbols_v.yaml",
    "rime_frost-dicts/cn_dicts/corrections.dict.yaml",
    "rime_frost-dicts/en_dicts/cn_en.txt",
    "lua/rime_frost/aux_code/moqi_aux_code.txt",
    "zh-moqi.gram",
    "rime_frost-LICENSE.txt",
    "opencc/rime_frost_emoji.json",
    "opencc/rime_frost_emoji.txt",
    "opencc/rime_frost_others.txt",
    "opencc/rime_frost_moqi_chaifen.json",
    "opencc/rime_frost_moqi_chaifen.txt",
    "opencc/rime_frost_moqi_chaifen_all.json",
    "opencc/rime_frost_moqi_chaifen_all.txt",
    "opencc/rime_frost_chinese_english.json",
    "opencc/rime_frost_chinese_english.txt",
    "opencc/rime_frost_english_chinese.txt",
    "opencc/rime_frost_martian.json",
    "opencc/rime_frost_martian.txt",
];
const MINT_LUA: &[&str] = &[
    "kp_number_processor",
    "select_character",
    "codeLengthLimit_processor",
    "unicode_translator",
    "shijian",
    "number_translator",
    "chineseLunarCalendar_translator",
    "mint_calculator_translator",
    "force_gc",
    "corrector_filter",
    "super_preedit",
    "autocap_filter",
    "reduce_english_filter",
];
const FROST_LUA: &[&str] = &[
    "select_character",
    "date_translator",
    "lunar",
    "unicode",
    "number_translator",
    "force_gc",
    "calculator",
    "is_in_user_dict",
    "corrector",
    "autocap_filter",
    "v_filter",
    "pin_cand_filter",
    "reduce_english_filter",
    "aux_lookup_filter",
];

pub(crate) static MINT: Addon = Addon {
    id: "rime_mint",
    name: "薄荷拼音",
    manifest: ".keytao-rime-mint.json",
    required: MINT_REQUIRED,
    installable: |name| Package::Mint.installable(name),
    extract: |archive, stage, package| Package::Mint.extract(archive, stage, package),
};
pub(crate) static FROST: Addon = Addon {
    id: "rime_frost",
    name: "白霜拼音",
    manifest: ".keytao-rime-frost.json",
    required: FROST_REQUIRED,
    installable: |name| Package::Frost.installable(name),
    extract: |archive, stage, package| Package::Frost.extract(archive, stage, package),
};

impl Package {
    pub(crate) fn addon(self) -> &'static Addon {
        match self {
            Self::Mint => &MINT,
            Self::Frost => &FROST,
        }
    }
    fn source(self) -> &'static str {
        match self {
            Self::Mint => "https://github.com/Mintimate/oh-my-rime",
            Self::Frost => "https://github.com/gaboolic/rime-frost",
        }
    }
    fn lua(self) -> &'static [&'static str] {
        match self {
            Self::Mint => MINT_LUA,
            Self::Frost => FROST_LUA,
        }
    }
    fn dependency(self, name: &str) -> String {
        if matches!(name, "melt_eng" | "radical_pinyin")
            || matches!(self, Self::Mint) && name == "stroke"
        {
            format!("{}_{name}", self.addon().id)
        } else {
            name.into()
        }
    }
    fn opencc(self, name: &str) -> bool {
        match self {
            Self::Mint => matches!(
                name,
                "emoji.json" | "emoji.txt" | "others.txt" | "spoken.txt"
            ),
            Self::Frost => matches!(
                name,
                "emoji.json"
                    | "emoji.txt"
                    | "others.txt"
                    | "moqi_chaifen.json"
                    | "moqi_chaifen.txt"
                    | "moqi_chaifen_all.json"
                    | "moqi_chaifen_all.txt"
                    | "chinese_english.json"
                    | "chinese_english.txt"
                    | "english_chinese.txt"
                    | "martian.json"
                    | "martian.txt"
            ),
        }
    }
    fn destination(self, name: &str) -> Option<String> {
        let name = match self {
            Self::Mint => name.strip_prefix("oh-my-rime-main/")?,
            Self::Frost => name,
        };
        let id = self.addon().id;
        if name == "LICENSE" {
            return Some(format!("{id}-LICENSE.txt"));
        }
        if let Some((stem, extension)) = name.split_once('.') {
            let selected = matches!(self, Self::Mint)
                && matches!(stem, "rime_mint" | "wubi98_mint" | "stroke")
                || matches!(self, Self::Frost) && matches!(stem, "rime_frost" | "rime_frost_aux")
                || matches!(stem, "melt_eng" | "radical_pinyin");
            if selected && matches!(extension, "schema.yaml" | "dict.yaml") {
                return Some(format!("{}.{extension}", self.dependency(stem)));
            }
        }
        match (self, name) {
            (Self::Mint, "symbols.yaml") => return Some("rime_mint_symbols.yaml".into()),
            (Self::Frost, "symbols_v.yaml") => return Some("rime_frost_symbols_v.yaml".into()),
            (Self::Frost, "zh-moqi.gram") => return Some(name.into()),
            (Self::Frost, "lua/aux_code/moqi_aux_code.txt") => {
                return Some(format!("lua/{id}/aux_code/moqi_aux_code.txt"));
            }
            _ => {}
        }
        if let Some(name) = name.strip_prefix("opencc/") {
            return self.opencc(name).then(|| format!("opencc/{id}_{name}"));
        }
        if let Some(module) = name
            .strip_prefix("lua/")
            .and_then(|name| name.strip_suffix(".lua"))
        {
            return self
                .lua()
                .contains(&module)
                .then(|| format!("lua/{id}/{module}.lua"));
        }
        match self {
            Self::Mint => {
                let table = name.strip_prefix("dicts/")?;
                (matches!(
                    table,
                    "custom_simple.dict.yaml"
                        | "other_kaomoji.dict.yaml"
                        | "wubi98_base.dict.yaml"
                        | "rime_ice.others.dict.yaml"
                        | "rime_ice.en_ext.dict.yaml"
                        | "rime_ice.en.dict.yaml"
                        | "rime_ice.cn_en.txt"
                ) || matches!(
                    table
                        .strip_prefix("rime_mint.")
                        .and_then(|n| n.strip_suffix(".dict.yaml")),
                    Some(
                        "chars"
                            | "base"
                            | "correlation"
                            | "compatible"
                            | "places"
                            | "artists"
                            | "famous"
                    )
                ))
                .then(|| format!("{id}-dicts/{table}"))
            }
            Self::Frost => {
                let (directory, table) = name.split_once('/')?;
                (!table.contains('/')
                    && match directory {
                        "cn_dicts" => matches!(
                            table,
                            "8105.dict.yaml"
                                | "base.dict.yaml"
                                | "ext.dict.yaml"
                                | "others.dict.yaml"
                                | "corrections.dict.yaml"
                        ),
                        "en_dicts" => {
                            matches!(table, "en_ext.dict.yaml" | "en.dict.yaml" | "cn_en.txt")
                        }
                        _ => false,
                    })
                .then(|| format!("{id}-dicts/{name}"))
            }
        }
    }

    fn installable(self, name: &str) -> bool {
        let id = self.addon().id;
        if self.addon().required.contains(&name) {
            return true;
        }
        if name == format!("{id}-NOTICE.txt") {
            return true;
        }
        let source = if name == format!("{id}-LICENSE.txt") {
            "LICENSE".into()
        } else if let Some(table) = name.strip_prefix(&format!("{id}-dicts/")) {
            match self {
                Self::Mint => format!("dicts/{table}"),
                Self::Frost => table.into(),
            }
        } else if let Some(module) = name.strip_prefix(&format!("lua/{id}/")) {
            format!("lua/{module}")
        } else if let Some(file) = name.strip_prefix(&format!("opencc/{id}_")) {
            format!("opencc/{file}")
        } else if let Some(dependency) = name.strip_prefix(&format!("{id}_")) {
            dependency.into()
        } else {
            name.into()
        };
        let source = match self {
            Self::Mint => format!("oh-my-rime-main/{source}"),
            Self::Frost => source,
        };
        self.destination(&source).as_deref() == Some(name)
    }

    fn adapt_yaml(self, value: &mut serde_yaml::Value) {
        match value {
            serde_yaml::Value::Mapping(map) => {
                for (key, value) in map {
                    if let (Some(key), Some(text)) = (key.as_str(), value.as_str()) {
                        let replacement = match key {
                            "schema_id" | "dictionary" | "name" => self.dependency(text),
                            "user_dict" => match self {
                                Self::Mint => text
                                    .strip_prefix("dicts/")
                                    .map(|table| format!("rime_mint-dicts/{table}"))
                                    .unwrap_or_else(|| text.into()),
                                Self::Frost if text == "custom_phrase" => {
                                    "rime_frost_custom_phrase".into()
                                }
                                Self::Frost => text
                                    .strip_prefix("en_dicts/")
                                    .map(|table| format!("rime_frost-dicts/en_dicts/{table}"))
                                    .unwrap_or_else(|| text.into()),
                            },
                            "opencc_config" if self.opencc(text) => {
                                format!("{}_{text}", self.addon().id)
                            }
                            "import_preset" if matches!(self, Self::Mint) && text == "symbols" => {
                                "rime_mint_symbols".into()
                            }
                            "__include" if matches!(self, Self::Frost) => {
                                text.replace("symbols_v:/", "rime_frost_symbols_v:/")
                            }
                            _ => text.into(),
                        };
                        *value = replacement.into();
                    } else {
                        self.adapt_yaml(value);
                    }
                    if key.as_str() == Some("dependencies") {
                        if let Some(dependencies) = value.as_sequence_mut() {
                            for dependency in dependencies {
                                if let Some(name) = dependency.as_str() {
                                    *dependency = self.dependency(name).into();
                                }
                            }
                        }
                    }
                }
            }
            serde_yaml::Value::Sequence(values) => {
                for value in values {
                    self.adapt_yaml(value);
                }
            }
            _ => {}
        }
    }

    fn adapt(self, name: &str, path: &Path) -> Result<(), String> {
        if name.ends_with(".schema.yaml") {
            let mut schema: serde_yaml::Value =
                serde_yaml::from_reader(File::open(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if name == "rime_mint_stroke.schema.yaml" {
                // Windows shared data has no Luna Pinyin; stroke is only a reverse-lookup dependency here.
                if let Some(dependencies) = schema
                    .get_mut("schema")
                    .and_then(|s| s.get_mut("dependencies"))
                    .and_then(serde_yaml::Value::as_sequence_mut)
                {
                    dependencies.retain(|value| value.as_str() != Some("luna_pinyin"));
                }
                if let Some(translators) = schema
                    .get_mut("engine")
                    .and_then(|e| e.get_mut("translators"))
                    .and_then(serde_yaml::Value::as_sequence_mut)
                {
                    translators.retain(|value| value.as_str() != Some("reverse_lookup_translator"));
                }
                schema.as_mapping_mut().unwrap().remove("reverse_lookup");
                if let Some(tags) = schema
                    .get_mut("abc_segmentor")
                    .and_then(|s| s.get_mut("extra_tags"))
                    .and_then(serde_yaml::Value::as_sequence_mut)
                {
                    tags.retain(|value| value.as_str() != Some("reverse_lookup"));
                }
                if let Some(patterns) = schema
                    .get_mut("recognizer")
                    .and_then(|r| r.get_mut("patterns"))
                    .and_then(serde_yaml::Value::as_mapping_mut)
                {
                    patterns.remove("reverse_lookup");
                }
            }
            self.adapt_yaml(&mut schema);
            for group in ["processors", "translators", "filters"] {
                if let Some(components) = schema
                    .get_mut("engine")
                    .and_then(|engine| engine.get_mut(group))
                    .and_then(serde_yaml::Value::as_sequence_mut)
                {
                    for component in components {
                        if let Some((kind, module)) =
                            component.as_str().and_then(|c| c.split_once("@*"))
                        {
                            if kind.starts_with("lua_") {
                                let (module, namespace) =
                                    module.split_once('@').unwrap_or((module, module));
                                *component =
                                    format!("{kind}@*{}/{module}@{namespace}", self.addon().id)
                                        .into();
                            }
                        }
                    }
                }
            }
            fs::write(
                path,
                serde_yaml::to_string(&schema).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        } else if name.ends_with(".dict.yaml") && !name.contains('/') {
            let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
            let content = content.replace("\r\n", "\n");
            let (header, entries) = content
                .split_once("\n...\n")
                .ok_or("附加方案词库缺少 YAML 结束标记")?;
            let mut header: serde_yaml::Value =
                serde_yaml::from_str(header).map_err(|e| e.to_string())?;
            self.adapt_yaml(&mut header);
            if let Some(tables) = header
                .get_mut("import_tables")
                .and_then(serde_yaml::Value::as_sequence_mut)
            {
                tables.retain(|table| {
                    !matches!(self, Self::Frost)
                        || table.as_str().is_none_or(|table| {
                            !table.starts_with("cn_dicts_cell/")
                                && !matches!(table, "cn_dicts/41448" | "cn_dicts/tencent")
                        })
                });
                for table in tables {
                    let table_name = table.as_str().ok_or("附加方案词库引用无效")?;
                    *table = match self {
                        Self::Mint => table_name
                            .strip_prefix("dicts/")
                            .map(|name| format!("rime_mint-dicts/{name}")),
                        Self::Frost => Some(format!("rime_frost-dicts/{table_name}")),
                    }
                    .ok_or("附加方案词库引用不在包内")?
                    .into();
                }
            }
            fs::write(
                path,
                format!(
                    "---\n{}...\n{entries}",
                    serde_yaml::to_string(&header).map_err(|e| e.to_string())?
                ),
            )
            .map_err(|e| e.to_string())?;
        } else if matches!(self, Self::Frost) && name.ends_with(".lua") {
            let content = fs::read_to_string(path)
                .map_err(|e| e.to_string())?
                .replace(
                    "/cn_dicts/corrections.dict.yaml",
                    "/rime_frost-dicts/cn_dicts/corrections.dict.yaml",
                )
                .replace(
                    "/lua/aux_code/moqi_aux_code.txt",
                    "/lua/rime_frost/aux_code/moqi_aux_code.txt",
                );
            fs::write(path, content).map_err(|e| e.to_string())?;
        } else if name.starts_with("opencc/") && name.ends_with(".json") {
            let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
            let mut json: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| e.to_string())?;
            self.adapt_opencc(&mut json);
            fs::write(
                path,
                serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn adapt_opencc(self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if key == "file" {
                        if let Some(name) = value.as_str().filter(|name| self.opencc(name)) {
                            *value = format!("{}_{name}", self.addon().id).into();
                        }
                    } else {
                        self.adapt_opencc(value);
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    self.adapt_opencc(value);
                }
            }
            _ => {}
        }
    }

    fn validate_opencc(self, value: &serde_json::Value, stage: &Path) -> Result<(), String> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if key == "file" {
                        let name = value.as_str().ok_or("附加方案 OpenCC 文件引用无效")?;
                        if !name.starts_with(&format!("{}_", self.addon().id))
                            || !wanxiang::safe_target(stage, &format!("opencc/{name}"))?.is_file()
                        {
                            return Err(format!(
                                "{}发布包缺少 OpenCC 文件：{name}",
                                self.addon().name
                            ));
                        }
                    } else {
                        self.validate_opencc(value, stage)?;
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    self.validate_opencc(value, stage)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_opencc_configs(
        self,
        value: &serde_yaml::Value,
        stage: &Path,
    ) -> Result<(), String> {
        match value {
            serde_yaml::Value::Mapping(map) => {
                for (key, value) in map {
                    if key.as_str() == Some("opencc_config") {
                        let name = value.as_str().ok_or("附加方案 OpenCC 配置引用无效")?;
                        if name != "s2t.json"
                            && (!name.starts_with(&format!("{}_", self.addon().id))
                                || !name.ends_with(".json")
                                || !wanxiang::safe_target(stage, &format!("opencc/{name}"))?
                                    .is_file())
                        {
                            return Err(format!(
                                "{}发布包缺少 OpenCC 配置：{name}",
                                self.addon().name
                            ));
                        }
                    } else {
                        self.validate_opencc_configs(value, stage)?;
                    }
                }
            }
            serde_yaml::Value::Sequence(values) => {
                for value in values {
                    self.validate_opencc_configs(value, stage)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn extract(
        self,
        archive: &Path,
        stage: &Path,
        package: &RemotePackage,
    ) -> Result<Vec<InstalledFile>, String> {
        if matches!(self, Self::Mint) {
            let commit = package
                .source
                .strip_prefix("https://github.com/Mintimate/oh-my-rime/commit/")
                .filter(|commit| {
                    commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .ok_or("薄荷拼音安装来源缺少有效提交号")?;
            let zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if zip.comment() != commit.as_bytes() {
                return Err(
                    "薄荷拼音 main 分支在下载期间发生变化，请重新安装以获取一致版本".into(),
                );
            }
        }
        let mut files = wanxiang::extract_package_with_limit(
            archive,
            stage,
            |name| self.destination(name),
            |name, path| self.adapt(name, path),
            128 * 1024 * 1024,
        )?;
        for name in self.addon().required {
            if !wanxiang::safe_target(stage, name)?.is_file() {
                return Err(format!("{}发布包缺少必要文件：{name}", self.addon().name));
            }
        }
        for module in self.lua() {
            let name = format!("lua/{}/{module}.lua", self.addon().id);
            if !wanxiang::safe_target(stage, &name)?.is_file() {
                return Err(format!(
                    "{}发布包缺少 Lua 模块：{module}",
                    self.addon().name
                ));
            }
        }
        for file in &files {
            if !self.installable(&file.path) {
                return Err(format!("附加方案文件不在安装范围：{}", file.path));
            }
            if file.path.ends_with(".schema.yaml") {
                let schema: serde_yaml::Value = serde_yaml::from_reader(
                    File::open(stage.join(&file.path)).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                self.validate_opencc_configs(&schema, stage)?;
                for dependency in schema
                    .get("schema")
                    .and_then(|s| s.get("dependencies"))
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                {
                    let dependency = dependency.as_str().ok_or("附加方案依赖无效")?;
                    if !wanxiang::safe_target(stage, &format!("{dependency}.schema.yaml"))?
                        .is_file()
                    {
                        return Err(format!(
                            "{}发布包缺少依赖方案：{dependency}",
                            self.addon().name
                        ));
                    }
                }
                for group in ["processors", "translators", "filters"] {
                    for component in schema
                        .get("engine")
                        .and_then(|engine| engine.get(group))
                        .and_then(serde_yaml::Value::as_sequence)
                        .into_iter()
                        .flatten()
                        .filter_map(serde_yaml::Value::as_str)
                    {
                        if let Some((kind, module)) = component.split_once("@*") {
                            if kind.starts_with("lua_") {
                                let module = module.split('@').next().unwrap();
                                if !module.starts_with(&format!("{}/", self.addon().id))
                                    || !wanxiang::safe_target(stage, &format!("lua/{module}.lua"))?
                                        .is_file()
                                {
                                    return Err(format!(
                                        "{}发布包缺少引用的 Lua 模块：{module}",
                                        self.addon().name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if file.path.starts_with("opencc/") && file.path.ends_with(".json") {
                let json = serde_json::from_reader(
                    File::open(stage.join(&file.path)).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                self.validate_opencc(&json, stage)?;
            }
            if file.path.ends_with(".dict.yaml") && !file.path.contains('/') {
                let header = serde_yaml::Deserializer::from_reader(
                    File::open(stage.join(&file.path)).map_err(|e| e.to_string())?,
                )
                .next()
                .ok_or("词库为空")?;
                let header = serde_yaml::Value::deserialize(header).map_err(|e| e.to_string())?;
                for table in header
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
        let notice = format!(
            "{} / {} {}\nLicense: GPL-3.0-only\nSource: {}\nUpstream: {}\n\
            Copyright: upstream authors and contributors\n\
            KeyTao integration changes: dependencies, dictionary tables, Lua modules and OpenCC data are namespaced.\n\
            Host settings and alternative layouts are omitted; Frost cell, 41448 and Tencent tables are omitted.\n\
            Mint stroke's optional Luna Pinyin reverse lookup is omitted for Windows compatibility.\n\
            Downloaded at runtime; not bundled with KeyTao.\n\
            User customizations and learned dictionaries are preserved during uninstall.\n",
            match self {
                Self::Mint => "Mint Pinyin",
                Self::Frost => "Frost Pinyin",
            },
            self.addon().name,
            package.version,
            package.source,
            self.source()
        );
        let name = format!("{}-NOTICE.txt", self.addon().id);
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
#[path = "pinyin_addons_tests.rs"]
mod tests;
