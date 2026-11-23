use crate::key_policy::{self, XK_F4};
use crate::{
    Candidate, DEFAULT_SELECT_KEYS, ImeState, RIME_MOD_CONTROL, RIME_RELEASE_MASK, RuntimeDirs,
    SchemaInfo, SchemaShortcut, SchemaSwitchSetting, package_build_ready, rime_modifier_mask,
    runtime_dirs::read_regular_file,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

pub const SCHEMA_ID: &str = ".keytao_packages";
pub const OPEN_PACKAGE_MENU: u32 = 0x0200_0004;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub const MOBILE_PACKAGE_CATALOG: &str = "scheme-packages/.keytao-switcher-packages.json";
const DEFAULT_PAGE_SIZE: usize = 5;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub display_name: String,
    #[serde(default)]
    pub schemas: Vec<SchemaInfo>,
}

pub fn is_hotkey(key: u32, mask: u32) -> bool {
    matches!(
        (key, rime_modifier_mask(mask)),
        (XK_F4 | OPEN_PACKAGE_MENU, 0) | (0x60, RIME_MOD_CONTROL)
    )
}

#[derive(Clone, Debug, Serialize)]
pub struct Package {
    pub package_key: String,
    pub display_name: String,
    pub active: bool,
    pub ready: bool,
    pub schemas: Vec<SchemaInfo>,
}

pub fn packages(
    root: &Path,
    shared: &Path,
    installed: impl IntoIterator<Item = (String, String)>,
) -> Vec<Package> {
    packages_in_store(root, shared, None, installed)
}

pub fn packages_in_store(
    root: &Path,
    shared: &Path,
    data_root: Option<&Path>,
    installed: impl IntoIterator<Item = (String, String)>,
) -> Vec<Package> {
    collect_packages(root, shared, data_root, installed, |dirs| {
        package_build_ready(root, dirs)
    })
}

pub(crate) fn mobile_packages(root: &Path, shared: &Path) -> Vec<Package> {
    // Reading the last build is enough for display. Selection validates current inputs.
    collect_packages(
        root,
        shared,
        None,
        installed_packages(root),
        crate::isolated_deploy::has_published_package_build,
    )
}

fn collect_packages(
    root: &Path,
    shared: &Path,
    data_root: Option<&Path>,
    installed: impl IntoIterator<Item = (String, String)>,
    build_ready: impl Fn(&RuntimeDirs) -> bool,
) -> Vec<Package> {
    if RuntimeDirs::needs_migration(root) {
        return Vec::new();
    }
    let active_key = RuntimeDirs::active_package_key(root);
    let catalog_schemas = catalog_schema_fallbacks(root);
    installed
        .into_iter()
        .filter(|(key, _)| key != "easy_en")
        .map(|(package_key, display_name)| {
            let active = active_key.as_ref() == Some(&package_key);
            let dirs = data_root
                .and_then(|data| RuntimeDirs::for_package_in_store(root, shared, data, &package_key))
                .or_else(|| RuntimeDirs::for_package(root, shared, &package_key));
            let ready = crate::runtime_libjd::package_libjd_ready(root, Some(&package_key))
                || dirs.as_ref().is_some_and(&build_ready);
            let mut schemas =
                crate::package_switchable_schemas(root, shared, data_root, &package_key);
            if schemas.is_empty() {
                if let Some(fallback) = catalog_schemas.get(&package_key) {
                    schemas = fallback.clone();
                }
            }
            if !active {
                schemas.retain(|schema| schema.id != "easy_en");
            }
            Package {
                active,
                package_key,
                display_name,
                ready,
                schemas,
            }
        })
        .collect()
}

fn catalog_schema_fallbacks(root: &Path) -> BTreeMap<String, Vec<SchemaInfo>> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    if let Some(bytes) = read_regular_file(&root.join(MOBILE_PACKAGE_CATALOG)) {
        if let Ok(catalog) = serde_json::from_slice::<BTreeMap<String, CatalogEntry>>(&bytes) {
            return catalog
                .into_iter()
                .map(|(key, entry)| (key, entry.schemas))
                .collect();
        }
    }
    let _ = root;
    BTreeMap::new()
}

pub fn default_package_name(key: &str) -> &str {
    match key.strip_prefix("builtin/").unwrap_or(key) {
        "keytao" => "键道6",
        "xmjd" => "星猫键道",
        "xmjd-feiding" => "星猫键道（非顶功）",
        "txjx" => "天行键",
        "keydo" => "键道·我流",
        "snow_jiandao" => "冰雪键道",
        "easy_en" => "Easy English",
        "wanxiang" => "万象拼音",
        "rime_ice" => "雾凇拼音",
        "rime_mint" => "薄荷拼音",
        "rime_frost" => "白霜拼音",
        "triple_pinyin_lssp" => "李氏三拼",
        _ => key,
    }
}

pub fn package_display_name(root: &Path, key: &str) -> String {
    let normalized = key.strip_prefix("builtin/").unwrap_or(key);
    installed_packages(root)
        .remove(normalized)
        .unwrap_or_else(|| default_package_name(normalized).into())
}

fn installed_packages(root: &Path) -> BTreeMap<String, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    if let Some(bytes) = read_regular_file(&root.join(MOBILE_PACKAGE_CATALOG)) {
        if let Ok(mut catalog) = serde_json::from_slice::<BTreeMap<String, CatalogEntry>>(&bytes) {
            catalog.retain(|key, _| RuntimeDirs::isolated_user_data_dir(root, key).is_some());
            return catalog
                .into_iter()
                .map(|(key, entry)| (key, entry.display_name))
                .collect();
        }
        if let Ok(mut catalog) = serde_json::from_slice::<BTreeMap<String, String>>(&bytes) {
            catalog.retain(|key, _| RuntimeDirs::isolated_user_data_dir(root, key).is_some());
            return catalog;
        }
    }
    let mut installed = BTreeMap::new();
    for store in RuntimeDirs::package_stores(root) {
        for namespace in ["cloud", "local", "addons"] {
            let directory = store.join(namespace);
            if !fs::symlink_metadata(&directory).is_ok_and(|meta| meta.is_dir()) {
                continue;
            }
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(key) = entry.file_name().into_string() else {
                    continue;
                };
                if !entry.file_type().is_ok_and(|kind| kind.is_dir())
                    || RuntimeDirs::isolated_user_data_dir(root, &key).is_none()
                {
                    continue;
                }
                let metadata = read_regular_file(&entry.path().join("package.json"));
                if metadata.is_none()
                    && !fs::symlink_metadata(entry.path().join("files"))
                        .is_ok_and(|meta| meta.is_dir())
                {
                    continue;
                }
                let name = metadata
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .and_then(|value| value["display_name"].as_str().map(str::to_owned))
                    .filter(|name| !name.trim().is_empty());
                if let Some(name) = name {
                    installed.insert(key, name);
                } else {
                    installed
                        .entry(key.clone())
                        .or_insert_with(|| default_package_name(&key).into());
                }
            }
        }
    }
    if let Some(active) =
        RuntimeDirs::active_package_key(root).filter(|key| key.starts_with("builtin/"))
    {
        installed
            .entry(active.clone())
            .or_insert_with(|| default_package_name(&active).into());
    }
    installed
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Stay,
    Close,
    SelectActiveSchema(String),
    SwitchPackage {
        package_key: String,
        schema_id: Option<String>,
    },
    ToggleSwitch {
        name: String,
        options: Vec<String>,
        next_value: usize,
    },
    Pass,
}

#[derive(Clone, Debug)]
enum Entry {
    ActiveSchema {
        schema_id: String,
        text: String,
    },
    InactiveSchema {
        package_key: String,
        schema_id: Option<String>,
        text: String,
        ready: bool,
    },
    Switch {
        name: String,
        options: Vec<String>,
        next_value: usize,
        text: String,
    },
}

impl Entry {
    fn candidate(&self) -> Candidate {
        let text = match self {
            Self::ActiveSchema { text, .. } | Self::Switch { text, .. } => text.clone(),
            Self::InactiveSchema { text, ready, .. } => {
                if *ready {
                    text.clone()
                } else {
                    format!("{text} 未部署")
                }
            }
        };
        Candidate {
            text,
            comment: None,
        }
    }
}

pub(crate) struct Menu {
    entries: Vec<Entry>,
    active_schema_id: String,
    highlighted: usize,
    page_size: usize,
    ascii_mode: bool,
    schema_name: String,
    hotkeys: Vec<SchemaShortcut>,
}

impl Menu {
    pub(crate) fn load(
        root: &Path,
        shared: &Path,
        active_schema_id: &str,
        active_schemas: Vec<SchemaInfo>,
        active_switches: Vec<SchemaSwitchSetting>,
        force_open: bool,
    ) -> Option<Self> {
        let page_size = configured_page_size(root);
        let libjd_active = crate::runtime_libjd::package_libjd_ready(
            root,
            RuntimeDirs::active_package_key(root).as_deref(),
        );
        Self::open(
            !RuntimeDirs::needs_migration(root),
            active_schema_id,
            active_schemas,
            active_switches,
            page_size,
            force_open,
            || {
                if libjd_active {
                    Vec::new()
                } else {
                    packages(root, shared, installed_packages(root))
                }
            },
        )
    }

    fn open(
        enabled: bool,
        active_schema_id: &str,
        active_schemas: Vec<SchemaInfo>,
        active_switches: Vec<SchemaSwitchSetting>,
        page_size: usize,
        force_open: bool,
        load_packages: impl FnOnce() -> Vec<Package>,
    ) -> Option<Self> {
        if !enabled {
            return None;
        }
        let packages = load_packages();
        if packages.is_empty() && !force_open {
            return None;
        }

        let active_package_name = packages
            .iter()
            .find(|pkg| pkg.active)
            .map(|pkg| pkg.display_name.clone())
            .unwrap_or_default();
        let effective_active_schemas = if !active_schemas.is_empty() {
            active_schemas
        } else {
            packages
                .iter()
                .find(|pkg| pkg.active)
                .map(|pkg| pkg.schemas.clone())
                .unwrap_or_default()
        };

        #[derive(Clone)]
        struct RawSchema {
            active_pkg: bool,
            package_key: String,
            package_name: String,
            schema_id: Option<String>,
            name: String,
            ready: bool,
        }

        let mut raw_schemas = Vec::new();
        for schema in effective_active_schemas {
            raw_schemas.push(RawSchema {
                active_pkg: true,
                package_key: String::new(),
                package_name: active_package_name.clone(),
                schema_id: Some(schema.id),
                name: schema.name,
                ready: true,
            });
        }
        for pkg in packages.into_iter().filter(|pkg| !pkg.active) {
            if pkg.schemas.is_empty() {
                raw_schemas.push(RawSchema {
                    active_pkg: false,
                    package_key: pkg.package_key,
                    package_name: pkg.display_name.clone(),
                    schema_id: None,
                    name: pkg.display_name,
                    ready: pkg.ready,
                });
            } else {
                for schema in pkg.schemas {
                    raw_schemas.push(RawSchema {
                        active_pkg: false,
                        package_key: pkg.package_key.clone(),
                        package_name: pkg.display_name.clone(),
                        schema_id: Some(schema.id),
                        name: schema.name,
                        ready: pkg.ready,
                    });
                }
            }
        }

        let mut name_counts = BTreeMap::<String, usize>::new();
        for item in &raw_schemas {
            *name_counts.entry(item.name.clone()).or_default() += 1;
        }

        let mut entries = Vec::new();
        for item in raw_schemas {
            let duplicate = name_counts.get(&item.name).copied().unwrap_or(0) > 1;
            let text = if duplicate
                && !item.package_name.is_empty()
                && item.package_name != item.name
            {
                format!("{}（{}）", item.name, item.package_name)
            } else {
                item.name
            };
            if item.active_pkg {
                entries.push(Entry::ActiveSchema {
                    schema_id: item.schema_id.unwrap_or_default(),
                    text,
                });
            } else {
                entries.push(Entry::InactiveSchema {
                    package_key: item.package_key,
                    schema_id: item.schema_id,
                    text,
                    ready: item.ready,
                });
            }
        }

        for switch in active_switches {
            if switch.states.len() < 2 || switch.value >= switch.states.len() {
                continue;
            }
            let next_value = (switch.value + 1) % switch.states.len();
            let text = format!(
                "{} → {}",
                switch.states[switch.value], switch.states[next_value]
            );
            entries.push(Entry::Switch {
                name: switch.name,
                options: switch.options,
                next_value,
                text,
            });
        }

        if entries.is_empty() {
            return None;
        }
        let highlighted = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    Entry::ActiveSchema { schema_id, .. } if schema_id == active_schema_id
                )
            })
            .unwrap_or(0);
        let page_size = page_size.clamp(1, DEFAULT_SELECT_KEYS.len());
        Some(Self {
            entries,
            active_schema_id: active_schema_id.to_owned(),
            highlighted,
            page_size,
            ascii_mode: false,
            schema_name: String::new(),
            hotkeys: Vec::new(),
        })
    }

    pub(crate) fn keep_input_mode(&mut self, state: ImeState) {
        self.ascii_mode = state.ascii_mode;
        self.schema_name = state.schema_name;
        self.hotkeys = state
            .schema_shortcuts
            .into_iter()
            .filter(|key| key.is_switcher())
            .collect();
    }

    pub(crate) fn state(&self) -> ImeState {
        let page = self.highlighted / self.page_size;
        let candidates = self.entries[page * self.page_size..]
            .iter()
            .take(self.page_size)
            .map(Entry::candidate)
            .collect();
        ImeState {
            candidates,
            highlighted_candidate_index: self.highlighted % self.page_size,
            page_size: self.page_size,
            page,
            is_last_page: (page + 1) * self.page_size >= self.entries.len(),
            select_keys: Some(DEFAULT_SELECT_KEYS[..self.page_size].into()),
            schema_id: SCHEMA_ID.into(),
            schema_name: self.schema_name.clone(),
            ascii_mode: self.ascii_mode,
            schema_shortcuts: self.hotkeys.clone(),
            ..ImeState::empty()
        }
    }

    pub(crate) fn candidates(&self, limit: usize) -> Vec<Candidate> {
        self.entries
            .iter()
            .take(limit)
            .map(Entry::candidate)
            .collect()
    }

    pub(crate) fn page_index(&self, index: usize) -> Option<usize> {
        (index < self.page_size)
            .then_some(self.highlighted / self.page_size * self.page_size + index)
            .filter(|&index| index < self.entries.len())
    }

    pub(crate) fn highlight(&mut self, index: usize) {
        if let Some(index) = self.page_index(index) {
            self.highlighted = index;
        }
    }

    pub(crate) fn change_page(&mut self, backward: bool) {
        let page = self.highlighted / self.page_size;
        if backward && page > 0 {
            self.highlighted = (page - 1) * self.page_size;
        } else if !backward && (page + 1) * self.page_size < self.entries.len() {
            self.highlighted = (page + 1) * self.page_size;
        }
    }

    pub(crate) fn select(&self, index: usize) -> Decision {
        match self.entries.get(index) {
            Some(Entry::ActiveSchema { schema_id, .. }) => {
                if schema_id.is_empty() || schema_id == &self.active_schema_id {
                    Decision::Close
                } else {
                    Decision::SelectActiveSchema(schema_id.clone())
                }
            }
            Some(Entry::InactiveSchema {
                package_key,
                schema_id,
                ready: true,
                ..
            }) => Decision::SwitchPackage {
                package_key: package_key.clone(),
                schema_id: schema_id.clone().filter(|id| !id.is_empty()),
            },
            Some(Entry::Switch {
                name,
                options,
                next_value,
                ..
            }) => Decision::ToggleSwitch {
                name: name.clone(),
                options: options.clone(),
                next_value: *next_value,
            },
            _ => Decision::Stay,
        }
    }

    pub(crate) fn key(&mut self, key: u32, mask: u32) -> Decision {
        use CandidateArrowAction::*;
        use key_policy::*;
        if is_hotkey(key, mask)
            || self.hotkeys.iter().any(|hotkey| hotkey.matches_switcher(key, mask))
        {
            self.highlighted = (self.highlighted + 1) % self.entries.len();
            return Decision::Stay;
        }
        if matches!(key, 0xffe1..=0xffe4 | 0xffe9..=0xffec) {
            return Decision::Stay;
        }
        if key == 0xffe5 {
            return Decision::Pass;
        }
        if mask & RIME_RELEASE_MASK != 0 {
            return Decision::Stay;
        }
        if rime_modifier_mask(mask) != 0 {
            return Decision::Pass;
        }
        let arrow = CandidateArrowAction::from_keycode(key).or(match key {
            XK_UP | XK_LEFT => Some(PreviousCandidate),
            XK_DOWN | XK_RIGHT => Some(NextCandidate),
            XK_PAGE_UP => Some(PreviousPage),
            XK_PAGE_DOWN => Some(NextPage),
            _ => None,
        });
        if let Some(arrow) = arrow {
            match arrow {
                PreviousCandidate => self.highlighted = self.highlighted.saturating_sub(1),
                NextCandidate => {
                    self.highlighted = (self.highlighted + 1).min(self.entries.len() - 1)
                }
                PreviousPage => self.change_page(true),
                NextPage => self.change_page(false),
            }
            return Decision::Stay;
        }
        let max_digit = 0x30 + self.page_size.min(9) as u32;
        match key {
            XK_ESCAPE | XK_BACK_SPACE => Decision::Close,
            XK_RETURN | XK_KP_ENTER | XK_SPACE => self.select(self.highlighted),
            digit if (0x31..=max_digit).contains(&digit) => self
                .page_index((digit - 0x31) as usize)
                .map_or(Decision::Stay, |index| self.select(index)),
            0x30..=0x39 => Decision::Stay,
            _ => Decision::Pass,
        }
    }
}

fn configured_page_size(root: &Path) -> usize {
    read_regular_file(&root.join(crate::english_mode::SETTINGS_FILE))
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|settings| settings["candidatePageSize"].as_u64())
        .filter(|size| (1..=10).contains(size))
        .map(|size| size as usize)
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use key_policy::{XK_DOWN, XK_ESCAPE, XK_PAGE_DOWN, XK_RETURN};

    #[test]
    fn flat_menu_lists_active_schemas_inactive_schemas_and_active_switches_only() {
        let mut menu = Menu::open(
            true,
            "xmjd6",
            vec![
                SchemaInfo {
                    id: "xmjd6".into(),
                    name: "🌟🐈".into(),
                },
                SchemaInfo {
                    id: "easy_en".into(),
                    name: "English".into(),
                },
            ],
            vec![
                SchemaSwitchSetting {
                    schema_id: "xmjd6".into(),
                    name: "ascii_mode".into(),
                    states: vec!["中文".into(), "西文".into()],
                    value: 0,
                    options: Vec::new(),
                },
                SchemaSwitchSetting {
                    schema_id: "xmjd6".into(),
                    name: "full_shape".into(),
                    states: vec!["半角".into(), "全角".into()],
                    value: 1,
                    options: Vec::new(),
                },
            ],
            DEFAULT_PAGE_SIZE,
            false,
            || {
                vec![
                    Package {
                        package_key: "xmjd".into(),
                        display_name: "星猫键道".into(),
                        active: true,
                        ready: true,
                        schemas: Vec::new(),
                    },
                    Package {
                        package_key: "rime_ice".into(),
                        display_name: "雾凇拼音".into(),
                        active: false,
                        ready: false,
                        schemas: vec![SchemaInfo {
                            id: "rime_ice".into(),
                            name: "雾凇拼音".into(),
                        }],
                    },
                    Package {
                        package_key: "keytao".into(),
                        display_name: "键道6".into(),
                        active: false,
                        ready: true,
                        schemas: vec![
                            SchemaInfo {
                                id: "keytao".into(),
                                name: "键道6".into(),
                            },
                            SchemaInfo {
                                id: "keytao-dz".into(),
                                name: "键道6单字".into(),
                            },
                        ],
                    },
                ]
            },
        )
        .unwrap();

        let first = menu.state();
        assert_eq!(first.schema_id, SCHEMA_ID);
        assert_eq!(
            first
                .candidates
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["🌟🐈", "English", "雾凇拼音 未部署", "键道6", "键道6单字"]
        );
        assert_eq!(first.page, 0);
        assert!(!first.is_last_page);
        assert_eq!(menu.key(XK_RETURN, 0), Decision::Close);
        assert_eq!(menu.key(XK_F4, 0), Decision::Stay);
        assert_eq!(menu.state().highlighted_candidate_index, 1);
        assert_eq!(
            menu.key(b'2'.into(), 0),
            Decision::SelectActiveSchema("easy_en".into())
        );
        assert_eq!(menu.key(b'3'.into(), 0), Decision::Stay);
        assert_eq!(
            menu.key(b'5'.into(), 0),
            Decision::SwitchPackage {
                package_key: "keytao".into(),
                schema_id: Some("keytao-dz".into()),
            }
        );

        assert_eq!(menu.key(XK_PAGE_DOWN, 0), Decision::Stay);
        let second = menu.state();
        assert_eq!(second.page, 1);
        assert!(second.is_last_page);
        assert_eq!(
            second
                .candidates
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["中文 → 西文", "全角 → 半角"]
        );
        assert_eq!(
            menu.key(b'1'.into(), 0),
            Decision::ToggleSwitch {
                name: "ascii_mode".into(),
                options: Vec::new(),
                next_value: 1,
            }
        );
        assert_eq!(menu.key(XK_DOWN, 0), Decision::Stay);
        assert_eq!(menu.key(b'`'.into(), RIME_MOD_CONTROL), Decision::Stay);
        assert_eq!(menu.state().page, 0);
        assert_eq!(menu.state().highlighted_candidate_index, 0);
        assert_eq!(menu.key(XK_ESCAPE, 0), Decision::Close);
    }

    #[test]
    fn disabled_does_not_load_or_intercept_the_package_menu() {
        assert!(is_hotkey(XK_F4, 0));
        assert!(is_hotkey(0x60, RIME_MOD_CONTROL));
        assert!(!is_hotkey(XK_F4, RIME_RELEASE_MASK));
        assert!(
            Menu::open(
                false,
                "keytao",
                Vec::new(),
                Vec::new(),
                DEFAULT_PAGE_SIZE,
                false,
                || panic!("loaded while disabled")
            )
            .is_none()
        );
    }

    #[test]
    fn modifier_keys_keep_the_package_menu_open() {
        use crate::{RIME_MOD_ALT, RIME_MOD_SHIFT, RIME_MOD_SUPER};

        let mut menu = Menu::open(
            true,
            "keytao",
            vec![SchemaInfo {
                id: "keytao".into(),
                name: "键道6".into(),
            }],
            Vec::new(),
            DEFAULT_PAGE_SIZE,
            false,
            || {
                vec![Package {
                    package_key: "keytao".into(),
                    display_name: "键道6".into(),
                    active: true,
                    ready: true,
                    schemas: Vec::new(),
                }]
            },
        )
        .unwrap();
        for (key, mask) in [
            (0xffe1, RIME_MOD_SHIFT),
            (0xffe2, RIME_MOD_SHIFT),
            (0xffe3, RIME_MOD_CONTROL),
            (0xffe4, RIME_MOD_CONTROL),
            (0xffe9, RIME_MOD_ALT),
            (0xffea, RIME_MOD_ALT),
            (0xffeb, RIME_MOD_SUPER),
            (0xffec, RIME_MOD_SUPER),
        ] {
            assert_eq!(menu.key(key, 0), Decision::Stay, "{key:#x} press");
            assert_eq!(menu.key(key, mask), Decision::Stay, "{key:#x} with mask");
            assert_eq!(
                menu.key(key, mask | RIME_RELEASE_MASK),
                Decision::Stay,
                "{key:#x} release"
            );
        }
    }
}
