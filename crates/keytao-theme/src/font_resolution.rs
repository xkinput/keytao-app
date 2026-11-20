//! Resolve portable theme family choices against fonts installed on this device.
use crate::FontTheme;
use std::{collections::BTreeSet, sync::OnceLock};

const EQUIVALENT_FAMILIES: &[&[&str]] = &[
    &[
        "PingFang SC",
        "Hiragino Sans GB",
        "Microsoft YaHei",
        "Microsoft YaHei UI",
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "Source Han Sans SC",
        "HarmonyOS Sans SC",
        "Droid Sans Fallback",
        "苹方-简",
        "微软雅黑",
        "思源黑体",
    ],
    &[
        "Songti SC",
        "SimSun",
        "NSimSun",
        "Noto Serif CJK SC",
        "Noto Serif SC",
        "Source Han Serif SC",
        "宋体-简",
        "宋体",
        "新宋体",
        "思源宋体",
    ],
    &[
        "Kaiti SC",
        "STKaiti",
        "KaiTi",
        "AR PL UKai",
        "AR PL UKai CN",
        "楷体-简",
        "华文楷体",
        "楷体",
    ],
    &[
        "Heiti SC",
        "SimHei",
        "WenQuanYi Zen Hei",
        "黑体-简",
        "黑体",
        "文泉驿正黑",
    ],
    &[
        "Yuanti SC",
        "YouYuan",
        "STYuanti",
        "HarmonyOS Sans SC Round",
        "圆体-简",
        "幼圆",
        "华文圆体",
    ],
];

/// Explicit fallback choices precede equivalent families; no match uses the
/// supplied platform default (`None` means the platform's native system font).
pub fn resolve_font_family(
    family: Option<&str>,
    installed: &[String],
    platform_default: Option<&str>,
) -> Option<String> {
    let choices: Vec<_> = family
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|family| !family.is_empty())
        .collect();
    let find = |choice: &str| {
        installed
            .iter()
            .find(|family| family.eq_ignore_ascii_case(choice))
            .cloned()
    };
    for choice in &choices {
        if let Some(family) = find(choice) {
            return Some(family);
        }
    }
    if let Some(class) = choices.first().and_then(|preferred| {
        EQUIVALENT_FAMILIES.iter().find(|class| {
            class
                .iter()
                .any(|family| family.eq_ignore_ascii_case(preferred))
        })
    }) {
        for equivalent in *class {
            if let Some(family) = find(equivalent) {
                return Some(family);
            }
        }
    }
    platform_default.map(str::to_owned)
}

struct InstalledFonts {
    families: Vec<String>,
    #[cfg(target_os = "android")]
    database: fontdb::Database,
}

fn installed_fonts() -> &'static InstalledFonts {
    static FONTS: OnceLock<InstalledFonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        #[cfg(target_os = "windows")]
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            database.load_fonts_dir(
                std::path::PathBuf::from(local_app_data).join("Microsoft/Windows/Fonts"),
            );
        }
        #[cfg(target_os = "android")]
        {
            database.load_fonts_dir("/system/fonts");
            database.load_fonts_dir("/product/fonts");
        }
        #[cfg(target_os = "ios")]
        database.load_fonts_dir("/System/Library/Fonts");
        let families = database
            .faces()
            .flat_map(|face| face.families.iter())
            .map(|(family, _)| family.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        InstalledFonts {
            families,
            #[cfg(target_os = "android")]
            database,
        }
    })
}

impl FontTheme {
    pub(crate) fn resolve_family(&mut self) {
        if self
            .family
            .as_deref()
            .is_none_or(|family| family.trim().is_empty())
        {
            self.effective_family = None;
            #[cfg(target_os = "android")]
            {
                self.effective_file = None;
                self.effective_index = 0;
            }
            return;
        }
        let fonts = installed_fonts();
        self.effective_family = resolve_font_family(self.family.as_deref(), &fonts.families, None);
        #[cfg(target_os = "android")]
        {
            self.effective_file = None;
            self.effective_index = 0;
            if let Some(face) = self.effective_family.as_deref().and_then(|family| {
                fonts
                    .database
                    .faces()
                    .filter(|face| {
                        face.style == fontdb::Style::Normal
                            && face.families.iter().any(|(name, _)| name == family)
                    })
                    .min_by_key(|face| face.weight.0.abs_diff(400))
            }) {
                self.effective_file = match &face.source {
                    fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
                        Some(path.to_string_lossy().into_owned())
                    }
                    fontdb::Source::Binary(_) => None,
                };
                self.effective_index = face.index;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_families_use_installed_equivalents_and_platform_default() {
        let installed =
            ["Microsoft YaHei", "SimSun", "KaiTi", "SimHei", "YouYuan"].map(str::to_owned);
        for (choice, expected) in [
            ("PingFang SC", "Microsoft YaHei"),
            ("Songti SC", "SimSun"),
            ("STKaiti", "KaiTi"),
            ("Heiti SC", "SimHei"),
            ("Yuanti SC", "YouYuan"),
            ("MICROSOFT YAHEI", "Microsoft YaHei"),
            ("Unknown Family", "System"),
        ] {
            assert_eq!(
                resolve_font_family(Some(choice), &installed, Some("System")).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(resolve_font_family(None, &installed, None), None);
        assert_eq!(resolve_font_family(Some("PingFang SC"), &[], None), None);
    }

    #[test]
    fn explicit_fallback_order_precedes_equivalence_mapping() {
        let installed = ["Microsoft YaHei", "KaiTi", "Custom Font"].map(str::to_owned);
        for (choice, expected) in [
            ("PingFang SC, Custom Font, KaiTi", "Custom Font"),
            ("PingFang SC, KaiTi, Custom Font", "KaiTi"),
            ("Microsoft YaHei, KaiTi", "Microsoft YaHei"),
        ] {
            assert_eq!(
                resolve_font_family(Some(choice), &installed, None).as_deref(),
                Some(expected)
            );
        }
    }
}
