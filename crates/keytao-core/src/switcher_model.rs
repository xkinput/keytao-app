use crate::key_policy::{self, XK_F4};
use crate::{
    Candidate, DEFAULT_SELECT_KEYS, ImeState, RIME_MOD_CONTROL, RIME_RELEASE_MASK, RuntimeDirs,
    package_build_ready, rime_modifier_mask, runtime_dirs::read_regular_file,
};
use serde::Serialize;
use std::{collections::BTreeMap, fs, path::Path};

pub const SCHEMA_ID: &str = ".keytao_packages";
pub const OPEN_PACKAGE_MENU: u32 = 0x0200_0004;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub const MOBILE_PACKAGE_CATALOG: &str = "scheme-packages/.keytao-switcher-packages.json";
const PAGE_SIZE: usize = 5;

pub fn is_hotkey(key: u32, mask: u32) -> bool {
    matches!(
        (key, rime_modifier_mask(mask)),
        (XK_F4 | OPEN_PACKAGE_MENU, 0) | (0x60, RIME_MOD_CONTROL)
    )
}

#[derive(Serialize)]
pub struct Package {
    pub package_key: String,
    pub display_name: String,
    pub active: bool,
    pub ready: bool,
}

pub fn packages(
    root: &Path,
    shared: &Path,
    installed: impl IntoIterator<Item = (String, String)>,
) -> Vec<Package> {
    if RuntimeDirs::needs_migration(root) {
        return Vec::new();
    }
    let active = RuntimeDirs::active_package_key(root);
    installed
        .into_iter()
        .filter(|(key, _)| key != "easy_en")
        .map(|(package_key, display_name)| {
            let ready = RuntimeDirs::for_package(root, shared, &package_key)
                .is_some_and(|dirs| package_build_ready(root, &dirs));
            Package {
                active: active.as_ref() == Some(&package_key),
                package_key,
                display_name,
                ready,
            }
        })
        .collect()
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
    if let Some(mut catalog) = read_regular_file(&root.join(MOBILE_PACKAGE_CATALOG))
        .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, String>>(&bytes).ok())
    {
        catalog.retain(|key, _| RuntimeDirs::isolated_user_data_dir(root, key).is_some());
        return catalog;
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
    Native,
    Switch(String),
    Pass,
}

pub(crate) struct Menu {
    packages: Vec<Package>,
    highlighted: usize,
    ascii_mode: bool,
    schema_name: String,
}

impl Menu {
    pub(crate) fn load(root: &Path, shared: &Path) -> Option<Self> {
        Self::open(!RuntimeDirs::needs_migration(root), || {
            packages(root, shared, installed_packages(root))
        })
    }

    fn open(enabled: bool, load: impl FnOnce() -> Vec<Package>) -> Option<Self> {
        if !enabled {
            return None;
        }
        let packages = load();
        let highlighted = packages
            .iter()
            .position(|package| package.active)
            .unwrap_or(0);
        (!packages.is_empty()).then_some(Self {
            packages,
            highlighted,
            ascii_mode: false,
            schema_name: String::new(),
        })
    }

    pub(crate) fn keep_input_mode(&mut self, state: ImeState) {
        self.ascii_mode = state.ascii_mode;
        self.schema_name = state.schema_name;
    }

    pub(crate) fn state(&self) -> ImeState {
        let page = self.highlighted / PAGE_SIZE;
        let candidates = self.packages[page * PAGE_SIZE..]
            .iter()
            .take(PAGE_SIZE)
            .map(Self::candidate)
            .collect();
        ImeState {
            candidates,
            highlighted_candidate_index: self.highlighted % PAGE_SIZE,
            page_size: PAGE_SIZE,
            page,
            is_last_page: (page + 1) * PAGE_SIZE >= self.packages.len(),
            select_keys: Some(DEFAULT_SELECT_KEYS[..PAGE_SIZE].into()),
            schema_id: SCHEMA_ID.into(),
            schema_name: self.schema_name.clone(),
            ascii_mode: self.ascii_mode,
            ..ImeState::empty()
        }
    }

    pub(crate) fn candidates(&self, limit: usize) -> Vec<Candidate> {
        self.packages
            .iter()
            .take(limit)
            .map(Self::candidate)
            .collect()
    }

    fn candidate(package: &Package) -> Candidate {
        let folder = package.package_key.rsplit('/').next().unwrap_or_default();
        let suffix = if package.ready { "" } else { " 未部署" };
        Candidate {
            text: format!("{}（{folder}）{suffix}", package.display_name),
            comment: None,
        }
    }

    pub(crate) fn page_index(&self, index: usize) -> Option<usize> {
        (index < PAGE_SIZE)
            .then_some(self.highlighted / PAGE_SIZE * PAGE_SIZE + index)
            .filter(|&index| index < self.packages.len())
    }

    pub(crate) fn highlight(&mut self, index: usize) {
        if let Some(index) = self.page_index(index) {
            self.highlighted = index;
        }
    }

    pub(crate) fn change_page(&mut self, backward: bool) {
        let page = self.highlighted / PAGE_SIZE;
        if backward && page > 0 {
            self.highlighted = (page - 1) * PAGE_SIZE;
        } else if !backward && (page + 1) * PAGE_SIZE < self.packages.len() {
            self.highlighted = (page + 1) * PAGE_SIZE;
        }
    }

    pub(crate) fn select(&self, index: usize) -> Decision {
        match self.packages.get(index) {
            Some(package) if package.ready && package.active => Decision::Native,
            Some(package) if package.ready => Decision::Switch(package.package_key.clone()),
            _ => Decision::Stay,
        }
    }

    pub(crate) fn key(&mut self, key: u32, mask: u32) -> Decision {
        use CandidateArrowAction::*;
        use key_policy::*;
        if is_hotkey(key, mask) {
            return Decision::Native;
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
                    self.highlighted = (self.highlighted + 1).min(self.packages.len() - 1)
                }
                PreviousPage => self.change_page(true),
                NextPage => self.change_page(false),
            }
            return Decision::Stay;
        }
        match key {
            XK_ESCAPE | XK_BACK_SPACE => Decision::Close,
            XK_RETURN | XK_KP_ENTER | XK_SPACE => self.select(self.highlighted),
            0x31..=0x35 => self
                .page_index((key - 0x31) as usize)
                .map_or(Decision::Stay, |index| self.select(index)),
            _ => Decision::Pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use key_policy::{XK_DOWN, XK_ESCAPE, XK_PAGE_DOWN, XK_RETURN};

    #[test]
    fn package_menu_routes_navigation_selection_and_escape() {
        let mut menu = Menu::open(true, || {
            (0..7)
                .map(|index| Package {
                    package_key: format!("package-{index}"),
                    display_name: format!("Package {index}"),
                    active: index == 0,
                    ready: index != 1,
                })
                .collect()
        })
        .unwrap();
        let first = menu.state();
        assert_eq!(first.schema_id, SCHEMA_ID);
        assert_eq!(first.candidates[1].text, "Package 1（package-1） 未部署");
        assert_eq!(first.candidates.len(), PAGE_SIZE);
        assert_eq!(first.page, 0);
        assert!(!first.is_last_page);
        assert_eq!(menu.key(XK_RETURN, 0), Decision::Native);
        assert_eq!(menu.key(XK_F4, 0), Decision::Native);
        assert_eq!(menu.key(XK_DOWN, 0), Decision::Stay);
        assert_eq!(menu.state().highlighted_candidate_index, 1);
        assert_eq!(menu.key(XK_RETURN, 0), Decision::Stay);
        assert_eq!(menu.key(XK_PAGE_DOWN, 0), Decision::Stay);
        assert_eq!(menu.state().page, 1);
        assert!(menu.state().is_last_page);
        assert_eq!(
            menu.key(b'2'.into(), 0),
            Decision::Switch("package-6".into())
        );
        assert_eq!(menu.key(XK_ESCAPE, 0), Decision::Close);
    }

    #[test]
    fn disabled_does_not_load_or_intercept_the_package_menu() {
        assert!(is_hotkey(XK_F4, 0));
        assert!(is_hotkey(0x60, RIME_MOD_CONTROL));
        assert!(!is_hotkey(XK_F4, RIME_RELEASE_MASK));
        assert!(Menu::open(false, || panic!("loaded while disabled")).is_none());
    }

    #[test]
    fn modifier_keys_keep_the_package_menu_open() {
        use crate::{RIME_MOD_ALT, RIME_MOD_SHIFT, RIME_MOD_SUPER};

        let mut menu = Menu::open(true, || {
            vec![Package {
                package_key: "package".into(),
                display_name: "Package".into(),
                active: true,
                ready: true,
            }]
        })
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
