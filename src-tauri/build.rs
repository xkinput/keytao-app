fn main() {
    emit_rime_version();

    #[cfg(target_os = "linux")]
    prepare_linux_ime_sidecar();

    tauri_build::build()
}

fn emit_rime_version() {
    println!("cargo:rerun-if-env-changed=RIME_VERSION");
    println!("cargo:rerun-if-env-changed=RIME_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    if let Some(version) = detect_rime_version() {
        println!("cargo:rustc-env=RIME_VERSION={version}");
    }
}

fn detect_rime_version() -> Option<String> {
    non_empty_env("RIME_VERSION").or_else(|| {
        non_empty_env("RIME_LIB_DIR")
            .and_then(|path| {
                let lib_dir = std::path::PathBuf::from(path);
                version_from_pkg_config_dir(&lib_dir)
                    .or_else(|| version_from_rime_lib_dir(&lib_dir))
            })
            .or_else(|| command_version("pkg-config", &["--modversion", "rime"]))
            .or_else(|| command_version("pkg-config", &["--modversion", "librime"]))
    })
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

fn version_from_pkg_config_dir(lib_dir: &std::path::Path) -> Option<String> {
    ["rime.pc", "librime.pc"].iter().find_map(|name| {
        let path = lib_dir.join("pkgconfig").join(name);
        let content = std::fs::read_to_string(path).ok()?;
        content.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.trim() == "Version" {
                let value = value.trim();
                if value.is_empty() {
                    None
                } else {
                    Some(value.to_owned())
                }
            } else {
                None
            }
        })
    })
}

fn version_from_rime_lib_dir(lib_dir: &std::path::Path) -> Option<String> {
    if lib_dir.is_file() {
        return lib_dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(version_from_rime_filename);
    }

    std::fs::read_dir(lib_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| version_from_rime_filename(&name))
        .max()
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
