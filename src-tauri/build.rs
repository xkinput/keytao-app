fn main() {
    emit_component_versions();

    #[cfg(target_os = "linux")]
    prepare_linux_ime_sidecar();

    tauri_build::build()
}

fn emit_component_versions() {
    println!("cargo:rerun-if-env-changed=RIME_VERSION");
    println!("cargo:rerun-if-env-changed=RIME_LIB_DIR");
    println!("cargo:rerun-if-env-changed=RIME_PREFIX");
    println!("cargo:rerun-if-env-changed=OPENCC_VERSION");
    println!("cargo:rerun-if-env-changed=OPENCC_LIB_DIR");
    println!("cargo:rerun-if-env-changed=KEYTAO_ANDROID_RIME_ROOT");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=TARGET");

    if let Some(version) = detect_rime_version() {
        println!("cargo:rustc-env=RIME_VERSION={version}");
    }
    if let Some(version) = detect_opencc_version() {
        println!("cargo:rustc-env=OPENCC_VERSION={version}");
    }
}

fn detect_rime_version() -> Option<String> {
    non_empty_env("RIME_VERSION")
        .or_else(|| {
            non_empty_env("RIME_LIB_DIR").and_then(|path| {
                let lib_dir = std::path::PathBuf::from(path);
                version_from_rime_dir(&lib_dir)
            })
        })
        .or_else(|| {
            non_empty_env("RIME_PREFIX").and_then(|path| {
                let prefix = std::path::PathBuf::from(path);
                version_from_rime_dir(&prefix)
                    .or_else(|| version_from_rime_dir(&prefix.join("lib")))
            })
        })
        .or_else(|| android_runtime_version("librime-release.txt", &["version", "librime_version"]))
        .or_else(|| {
            workspace_runtime_version("librime-release.txt", &["version", "librime_version"])
        })
        .or_else(|| workspace_rime_lib_version())
        .or_else(|| command_version("pkg-config", &["--modversion", "rime"]))
        .or_else(|| command_version("pkg-config", &["--modversion", "librime"]))
}

fn detect_opencc_version() -> Option<String> {
    non_empty_env("OPENCC_VERSION")
        .or_else(|| {
            non_empty_env("OPENCC_LIB_DIR").and_then(|path| {
                let lib_dir = std::path::PathBuf::from(path);
                version_from_opencc_dir(&lib_dir)
            })
        })
        .or_else(|| android_runtime_version("opencc-release.txt", &["version", "opencc_version"]))
        .or_else(|| workspace_runtime_version("opencc-release.txt", &["version", "opencc_version"]))
        .or_else(|| workspace_opencc_pkg_config_version())
        .or_else(|| command_version("pkg-config", &["--modversion", "opencc"]))
        .or_else(|| command_version("pkg-config", &["--modversion", "libopencc"]))
        .or_else(|| command_version("opencc", &["--version"]).and_then(first_version_token))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    })
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
        Some(value.to_owned())
    }
}

fn first_version_token(value: String) -> Option<String> {
    value
        .split_whitespace()
        .map(|part| {
            part.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-')
        })
        .find_map(|part| {
            let version = part
                .strip_prefix("ver.")
                .or_else(|| part.strip_prefix('v'))
                .unwrap_or(part);
            version
                .chars()
                .next()
                .filter(|ch| ch.is_ascii_digit())
                .and_then(|_| non_empty_value(version))
        })
}

fn version_from_pkg_config_content(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() == "Version" {
            non_empty_value(value)
        } else {
            None
        }
    })
}

fn pkg_config_version(lib_dir: &std::path::Path, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let path = lib_dir.join("pkgconfig").join(name);
        let content = std::fs::read_to_string(path).ok()?;
        version_from_pkg_config_content(&content)
    })
}

fn metadata_file_version(path: &std::path::Path, keys: &[&str]) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        keys.iter()
            .any(|candidate| key.trim() == *candidate)
            .then(|| non_empty_value(value))
            .flatten()
    })
}

fn non_empty_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn version_from_rime_dir(lib_dir: &std::path::Path) -> Option<String> {
    if lib_dir.is_file() {
        return lib_dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(version_from_rime_filename);
    }

    metadata_file_version(
        &lib_dir.join("librime-release.txt"),
        &["version", "librime_version"],
    )
    .or_else(|| pkg_config_version(lib_dir, &["rime.pc", "librime.pc"]))
    .or_else(|| {
        std::fs::read_dir(lib_dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| version_from_rime_filename(&name))
            .max()
    })
}

fn version_from_rime_filename(name: &str) -> Option<String> {
    let version = name
        .strip_prefix("librime.")
        .and_then(|value| value.strip_suffix(".dylib"))
        .or_else(|| name.strip_prefix("librime.so."))?;
    if version.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        Some(version.to_owned())
    } else {
        None
    }
}

fn version_from_opencc_dir(path: &std::path::Path) -> Option<String> {
    if path.is_file() {
        return std::fs::read_to_string(path)
            .ok()
            .and_then(|content| version_from_pkg_config_content(&content));
    }

    metadata_file_version(
        &path.join("opencc-release.txt"),
        &["version", "opencc_version"],
    )
    .or_else(|| pkg_config_version(path, &["opencc.pc", "libopencc.pc", "OpenCC.pc"]))
    .or_else(|| {
        pkg_config_version(
            &path.join("lib"),
            &["opencc.pc", "libopencc.pc", "OpenCC.pc"],
        )
    })
}

fn workspace_root() -> Option<std::path::PathBuf> {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
    manifest_dir.parent().map(|path| path.to_path_buf())
}

fn android_abi_from_target(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-linux-android" => Some("arm64-v8a"),
        "armv7-linux-androideabi" => Some("armeabi-v7a"),
        "i686-linux-android" => Some("x86"),
        "x86_64-linux-android" => Some("x86_64"),
        _ => None,
    }
}

fn android_runtime_root() -> Option<std::path::PathBuf> {
    let target = std::env::var("TARGET").ok()?;
    let abi = android_abi_from_target(&target)?;

    if let Ok(root) = std::env::var("KEYTAO_ANDROID_RIME_ROOT") {
        let root = std::path::PathBuf::from(root);
        if root.join("lib/librime.so").is_file() {
            return Some(root);
        }
        let abi_root = root.join(abi);
        if abi_root.join("lib/librime.so").is_file() {
            return Some(abi_root);
        }
    }

    workspace_root()
        .map(|root| root.join("vendor/librime/android").join(abi))
        .filter(|root| root.join("lib/librime.so").is_file())
}

fn android_runtime_version(file_name: &str, keys: &[&str]) -> Option<String> {
    let root = android_runtime_root()?;
    metadata_file_version(&root.join(file_name), keys).or_else(|| {
        if file_name == "librime-release.txt" {
            version_from_rime_dir(&root).or_else(|| version_from_rime_dir(&root.join("lib")))
        } else {
            version_from_opencc_dir(&root)
                .or_else(|| version_from_opencc_dir(&root.join("rime-data")))
        }
    })
}

fn workspace_runtime_dirs() -> Vec<std::path::PathBuf> {
    let Some(root) = workspace_root() else {
        return Vec::new();
    };
    let mut dirs = vec![
        root.join("vendor/librime/macos-universal"),
        root.join("vendor/librime/macos-universal/lib"),
        root.join("vendor/librime/windows-x64"),
        root.join("vendor/librime/windows-x64/lib"),
        root.join("vendor/librime/windows-x86"),
        root.join("vendor/librime/windows-x86/lib"),
        root.join("target/keytao-linux-runtime"),
        root.join("target/keytao-linux-runtime/lib"),
        root.join("target/keytao-linux-runtime/rime-data"),
        root.join("target/keytao-macos-app-runtime"),
        root.join("target/keytao-macos-app-runtime/Frameworks"),
        root.join("target/keytao-macos-app-runtime/rime-data"),
        root.join("src-tauri/gen/android/app/src/main/assets"),
        root.join("src-tauri/gen/android/app/src/main/assets/keytao-rime-data"),
    ];
    for abi in ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"] {
        dirs.push(root.join("vendor/librime/android").join(abi));
        dirs.push(
            root.join("vendor/librime/android")
                .join(abi)
                .join("rime-data"),
        );
    }
    dirs
}

fn workspace_runtime_version(file_name: &str, keys: &[&str]) -> Option<String> {
    workspace_runtime_dirs()
        .into_iter()
        .find_map(|dir| metadata_file_version(&dir.join(file_name), keys))
}

fn workspace_rime_lib_version() -> Option<String> {
    workspace_runtime_dirs()
        .into_iter()
        .find_map(|dir| version_from_rime_dir(&dir))
}

fn workspace_opencc_pkg_config_version() -> Option<String> {
    let Some(root) = workspace_root() else {
        return None;
    };
    workspace_runtime_dirs()
        .into_iter()
        .chain([
            root.join(".cache/librime"),
            root.join(".cache/librime-inspect"),
        ])
        .find_map(|dir| find_pkg_config_file_version(&dir, "opencc.pc"))
}

fn find_pkg_config_file_version(root: &std::path::Path, file_name: &str) -> Option<String> {
    if !root.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            let content = std::fs::read_to_string(path).ok()?;
            if let Some(version) = version_from_pkg_config_content(&content) {
                return Some(version);
            }
        } else if path.is_dir() {
            if let Some(version) = find_pkg_config_file_version(&path, file_name) {
                return Some(version);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn prepare_linux_ime_sidecar() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=KEYTAO_IME_PATH");

    let Ok(source) = std::env::var("KEYTAO_IME_PATH") else {
        return;
    };
    let Ok(target) = std::env::var("TARGET") else {
        return;
    };

    let metadata = std::fs::metadata(&source).expect("stat keytao-ime sidecar");
    assert!(metadata.len() > 0, "keytao-ime sidecar is empty");

    let destination = PathBuf::from("binaries").join(format!("keytao-ime-{target}"));
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).expect("create sidecar directory");
    }
    std::fs::copy(&source, &destination).expect("copy keytao-ime sidecar");
}
