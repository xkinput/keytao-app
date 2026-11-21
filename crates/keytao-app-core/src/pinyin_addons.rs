//! Mint and Frost source extraction into isolated Rime scheme directories.

use crate::{
    downloaded_addon::Addon,
    generic_rime::{self, DEFAULT_MAX_EXPANDED_BYTES},
    wanxiang::{InstalledFile, RemotePackage},
};
use std::{fs::File, path::Path};

pub(crate) use crate::shape_addons::{LSSP, SNOW};

#[derive(Clone, Copy)]
pub(crate) enum Package {
    Mint,
    Frost,
}

const MINT_REQUIRED: &[&str] = &["rime_mint.schema.yaml"];
const FROST_REQUIRED: &[&str] = &["rime_frost.schema.yaml"];

pub(crate) static MINT: Addon = Addon {
    id: "rime_mint",
    dictionary: "rime_mint",
    name: "薄荷拼音",
    manifest: ".keytao-rime-mint.json",
    required: MINT_REQUIRED,
    extra_archives: &[],
    installable: |name| Package::Mint.installable(name),
    extract: |archive, stage, package| Package::Mint.extract(archive, stage, package),
};
pub(crate) static FROST: Addon = Addon {
    id: "rime_frost",
    dictionary: "rime_frost",
    name: "白霜拼音",
    manifest: ".keytao-rime-frost.json",
    required: FROST_REQUIRED,
    extra_archives: &[],
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

    fn installable(self, name: &str) -> bool {
        let _ = self;
        generic_rime::is_installable_path(name)
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
        let mut files = generic_rime::extract_archive(
            archive,
            stage,
            Some(self.addon().id),
            DEFAULT_MAX_EXPANDED_BYTES,
        )?;
        let notice = format!(
            "{} / {} {}\nLicense: GPL-3.0-only\nSource: {}\nUpstream: {}\n\
            Copyright: upstream authors and contributors\n\
            Downloaded at runtime into its own isolated Rime directory; not bundled with KeyTao.\n\
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
        generic_rime::append_notice(stage, &mut files, &name, &notice)?;
        Ok(files)
    }
}

#[cfg(test)]
#[path = "pinyin_addons_tests.rs"]
mod tests;
