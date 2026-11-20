use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LibjdPlatform {
    pub target: &'static str,
    pub library: &'static str,
    pub archive_entry: &'static str,
    pub archive_suffix: &'static str,
}

pub fn platform() -> Option<LibjdPlatform> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn platform_for(os: &str, arch: &str) -> Option<LibjdPlatform> {
    let (target, library, archive_entry, archive_suffix) = match (os, arch) {
        ("macos", "aarch64") => ("macos-arm64", "libjd.dylib", "libjd.dylib", "dylib.tar.xz"),
        ("macos", "x86_64") => ("macos-x86_64", "libjd.dylib", "libjd.dylib", "dylib.tar.xz"),
        ("linux", "aarch64") => ("linux-arm64", "libjd.so", "libjd.so", "so.tar.xz"),
        ("linux", "x86_64") => ("linux-x86_64", "libjd.so", "libjd.so", "so.tar.xz"),
        ("android", "aarch64") => ("android-arm64", "libjd.so", "libjd.so", "so.tar.xz"),
        ("android", "x86_64") => ("android-x86_64", "libjd.so", "libjd.so", "so.tar.xz"),
        ("windows", "aarch64") => ("windows-arm64", "libjd.dll", "jd.dll", "zip"),
        ("windows", "x86_64") => ("windows-x86_64", "libjd.dll", "jd.dll", "zip"),
        _ => return None,
    };
    Some(LibjdPlatform {
        target,
        library,
        archive_entry,
        archive_suffix,
    })
}

#[derive(Deserialize, Serialize)]
pub struct LibjdManifest {
    pub tag: String,
    pub asset: String,
    pub sha256: String,
    pub abi_layout: serde_json::Value,
    pub installed_at: u64,
}

pub fn valid_tag(tag: &str) -> bool {
    tag.len() <= 80
        && tag.strip_prefix('v').is_some_and(|version| {
            version.starts_with(|c: char| c.is_ascii_digit())
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
                && !version.ends_with('.')
        })
}

pub fn valid_sha256(sha256: &str) -> bool {
    sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl LibjdPlatform {
    pub fn asset_name(self, tag: &str) -> String {
        format!("libjd-{tag}-{}.{}", self.target, self.archive_suffix)
    }

    pub fn current_path(self, root: &Path) -> PathBuf {
        root.join(format!("current-{}", self.target))
    }

    pub fn library_path(self, data: &Path, tag: &str) -> Option<PathBuf> {
        if !valid_tag(tag) {
            return None;
        }
        let version = engine_root(data)?.join(tag);
        let directory = version.join(self.target);
        (dir_or_missing(&version) && dir_or_missing(&directory))
            .then(|| directory.join(self.library))
    }

    pub fn manifest_at(self, library: &Path, tag: &str) -> Option<LibjdManifest> {
        if !fs::symlink_metadata(library).is_ok_and(|metadata| metadata.is_file()) {
            return None;
        }
        let text = small_text(&library.with_file_name("manifest.json"), 8192)?;
        let manifest: LibjdManifest = serde_json::from_str(&text).ok()?;
        (manifest.tag == tag
            && manifest.asset == self.asset_name(tag)
            && valid_sha256(&manifest.sha256)
            && manifest.abi_layout.is_object())
        .then_some(manifest)
    }

    fn installed(self, data: &Path) -> Option<(LibjdManifest, PathBuf)> {
        let root = engine_root(data)?;
        let tag = small_text(&self.current_path(&root), 128)?;
        let library = self.library_path(data, &tag)?;
        if let Some(manifest) = self.manifest_at(&library, &tag) {
            return Some((manifest, library));
        }
        // A same-tag replacement can be interrupted between the two directory renames.
        let previous = root.join(&tag).join(format!(".old-{}", self.target));
        if !fs::symlink_metadata(&previous).is_ok_and(|metadata| metadata.is_dir()) {
            return None;
        }
        let library = previous.join(self.library);
        Some((self.manifest_at(&library, &tag)?, library))
    }

    pub fn installed_tag(self, data: &Path) -> Option<String> {
        self.installed(data).map(|(manifest, _)| manifest.tag)
    }
}

fn dir_or_missing(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

pub fn engine_root(data: &Path) -> Option<PathBuf> {
    let engines = data.join("engines");
    let root = engines.join("libjd");
    (dir_or_missing(&engines) && dir_or_missing(&root)).then_some(root)
}

pub fn library_path(data: &Path, tag: &str) -> Option<PathBuf> {
    platform()?.library_path(data, tag)
}

fn small_text(path: &Path, limit: u64) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    fs::read_to_string(path).ok()
}

pub fn manifest_at(library: &Path, tag: &str) -> Option<LibjdManifest> {
    platform()?.manifest_at(library, tag)
}

pub fn installed_library(data: &Path) -> Option<PathBuf> {
    platform()?.installed(data).map(|(_, library)| library)
}
