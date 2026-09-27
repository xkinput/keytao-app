#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::ime_paths::path_string;
use crate::{Core, ime_host::ImeHost};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
pub struct ComponentVersions {
    pub app_version: String,
    pub tauri_version: String,
    pub librime_version: Option<String>,
    pub opencc_version: Option<String>,
    pub data_dir: Option<String>,
}

fn command_version(command: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let value = text.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn non_empty_version(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn env_version(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty_version)
}

fn nix_store_package_version(package: &str) -> Option<String> {
    let entries = std::fs::read_dir("/nix/store").ok()?;
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.ends_with(".drv"))
        .filter_map(|name| {
            let marker = format!("-{package}-");
            let version = name.split_once(&marker)?.1.to_string();
            Some(version)
        })
        .max()
}

fn version_from_pkg_config_content(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() == "Version" {
            non_empty_version(value)
        } else {
            None
        }
    })
}

fn pkg_config_file_version(lib_dir: &Path, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let path = lib_dir.join("pkgconfig").join(name);
        let content = std::fs::read_to_string(path).ok()?;
        version_from_pkg_config_content(&content)
    })
}

fn metadata_file_version(path: &Path, keys: &[&str]) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        keys.iter()
            .any(|candidate| key.trim() == *candidate)
            .then(|| non_empty_version(value))
            .flatten()
    })
}

fn rime_filename_version(name: &str) -> Option<String> {
    let version = name
        .strip_prefix("librime.")
        .and_then(|value| value.strip_suffix(".dylib"))
        .or_else(|| name.strip_prefix("librime.so."))?;
    version
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_digit())
        .and_then(|_| non_empty_version(version))
}

fn rime_lib_dir_version(path: &Path) -> Option<String> {
    if path.is_file() {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(rime_filename_version);
    }

    metadata_file_version(
        &path.join("librime-release.txt"),
        &["version", "librime_version"],
    )
    .or_else(|| pkg_config_file_version(path, &["rime.pc", "librime.pc"]))
    .or_else(|| {
        std::fs::read_dir(path)
            .ok()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| rime_filename_version(&name))
            .max()
    })
}

fn opencc_dir_version(path: &Path) -> Option<String> {
    if path.is_file() {
        return std::fs::read_to_string(path)
            .ok()
            .and_then(|content| version_from_pkg_config_content(&content));
    }

    metadata_file_version(
        &path.join("opencc-release.txt"),
        &["version", "opencc_version"],
    )
    .or_else(|| pkg_config_file_version(path, &["opencc.pc", "libopencc.pc", "OpenCC.pc"]))
    .or_else(|| {
        pkg_config_file_version(
            &path.join("lib"),
            &["opencc.pc", "libopencc.pc", "OpenCC.pc"],
        )
    })
}

fn bundled_runtime_candidates(core: &Core) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    {
        let resource_dir = core.env().resource_dir.clone();
        candidates.extend([
            resource_dir.clone(),
            resource_dir.join("Frameworks"),
            resource_dir.join("rime-data"),
            resource_dir.join("runtime"),
            resource_dir.join("runtime/lib"),
            resource_dir.join("runtime/rime-data"),
            resource_dir.join("keytao-windows-ime-runtime/current"),
            resource_dir.join("keytao-windows-ime-runtime/current/bin"),
            resource_dir.join("keytao-windows-ime-runtime/current/lib"),
            resource_dir.join("keytao-windows-ime-runtime/current/rime-data"),
        ]);
        if let Some(contents) = resource_dir.parent() {
            candidates.push(contents.join("Frameworks"));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.extend([
                exe_dir.to_path_buf(),
                exe_dir.join("Frameworks"),
                exe_dir.join("runtime"),
                exe_dir.join("runtime/lib"),
                exe_dir.join("runtime/rime-data"),
            ]);
            if let Some(contents) = exe_dir.parent() {
                candidates.push(contents.join("Frameworks"));
            }
        }
    }

    candidates
}

fn bundled_librime_version(core: &Core) -> Option<String> {
    bundled_runtime_candidates(core)
        .into_iter()
        .find_map(|dir| rime_lib_dir_version(&dir))
}

fn runtime_librime_version() -> Option<String> {
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        keytao_core::librime_runtime_version().and_then(non_empty_version)
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        None
    }
}

fn librime_version(core: &Core, embedded_version: Option<&str>) -> Option<String> {
    runtime_librime_version()
        .or_else(|| embedded_version.and_then(non_empty_version))
        .or_else(|| env_version("RIME_VERSION"))
        .or_else(|| {
            env_version("RIME_LIB_DIR").and_then(|path| rime_lib_dir_version(Path::new(&path)))
        })
        .or_else(|| bundled_librime_version(core))
        .or_else(|| command_version("pkg-config", &["--modversion", "rime"]))
        .or_else(|| command_version("pkg-config", &["--modversion", "librime"]))
        .or_else(|| nix_store_package_version("librime"))
}

fn opencc_version(core: &Core, embedded_version: Option<&str>) -> Option<String> {
    embedded_version
        .and_then(non_empty_version)
        .or_else(|| env_version("OPENCC_VERSION"))
        .or_else(|| {
            env_version("OPENCC_LIB_DIR").and_then(|path| opencc_dir_version(Path::new(&path)))
        })
        .or_else(|| {
            bundled_runtime_candidates(core)
                .into_iter()
                .find_map(|dir| opencc_dir_version(&dir))
        })
        .or_else(|| {
            ["opencc", "libopencc", "OpenCC"]
                .iter()
                .find_map(|name| command_version("pkg-config", &["--modversion", name]))
        })
        .or_else(|| command_version("opencc", &["--version"]))
        .or_else(|| nix_store_package_version("opencc"))
}

#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    allow(unused_variables)
)]
pub fn get_component_versions(
    core: &Core,
    host: &impl ImeHost,
    tauri_version: &str,
    rime_version: Option<&str>,
    opencc_version_hint: Option<&str>,
) -> ComponentVersions {
    #[cfg(target_os = "android")]
    let data_dir = host.user_root().ok().map(path_string);
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let data_dir = keytao_core::default_user_data_dir().map(|p| p.to_string_lossy().into_owned());
    #[cfg(target_os = "ios")]
    let data_dir = host.user_root().ok().map(path_string);

    ComponentVersions {
        app_version: core.env().app_version.clone(),
        tauri_version: tauri_version.to_string(),
        librime_version: librime_version(&core, rime_version),
        opencc_version: opencc_version(&core, opencc_version_hint),
        data_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_rime_filename_version() {
        assert_eq!(
            rime_filename_version("librime.1.17.0.dylib"),
            Some("1.17.0".to_owned())
        );
        assert_eq!(
            rime_filename_version("librime.so.1.17.0"),
            Some("1.17.0".to_owned())
        );
        assert_eq!(rime_filename_version("librime-lua.dylib"), None);
    }

    #[test]
    fn test_rime_lib_dir_version_prefers_pkg_config() {
        let dir =
            std::env::temp_dir().join(format!("keytao-rime-version-test-{}", std::process::id()));
        let pkgconfig_dir = dir.join("pkgconfig");
        std::fs::create_dir_all(&pkgconfig_dir).expect("create pkgconfig dir");
        std::fs::write(
            pkgconfig_dir.join("rime.pc"),
            "Name: Rime\nVersion: 9.8.7\n",
        )
        .expect("write rime.pc");
        std::fs::write(dir.join("librime.1.dylib"), "").expect("write dylib marker");

        assert_eq!(rime_lib_dir_version(&dir), Some("9.8.7".to_owned()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_rime_lib_dir_version_reads_release_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-rime-release-version-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create version dir");
        std::fs::write(
            dir.join("librime-release.txt"),
            "platform=android\nversion=1.17.0\n",
        )
        .expect("write release metadata");
        std::fs::write(dir.join("librime.1.dylib"), "").expect("write dylib marker");

        assert_eq!(rime_lib_dir_version(&dir), Some("1.17.0".to_owned()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_opencc_dir_version_reads_metadata_and_pkg_config() {
        let dir =
            std::env::temp_dir().join(format!("keytao-opencc-version-test-{}", std::process::id()));
        let pkgconfig_dir = dir.join("lib/pkgconfig");
        std::fs::create_dir_all(&pkgconfig_dir).expect("create pkgconfig dir");
        std::fs::write(dir.join("opencc-release.txt"), "version=1.1.9\n")
            .expect("write opencc metadata");
        std::fs::write(
            pkgconfig_dir.join("opencc.pc"),
            "Name: opencc\nVersion: 7.6.5\n",
        )
        .expect("write opencc.pc");

        assert_eq!(opencc_dir_version(&dir), Some("1.1.9".to_owned()));
        std::fs::remove_file(dir.join("opencc-release.txt")).ok();
        assert_eq!(opencc_dir_version(&dir), Some("7.6.5".to_owned()));
        std::fs::remove_dir_all(dir).ok();
    }
}
