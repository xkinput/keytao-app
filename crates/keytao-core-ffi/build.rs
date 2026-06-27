extern crate cbindgen;

use std::{env, path::PathBuf};

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = PathBuf::from(&crate_dir).join("include");
    std::fs::create_dir_all(&out_dir).unwrap();

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_language(cbindgen::Language::C)
        .with_include_guard("KEYTAO_CORE_H")
        .with_documentation(true)
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(out_dir.join("keytao_core.h"));

    let target = env::var("TARGET").unwrap_or_default();
    if target.contains("apple-ios") {
        link_ios_runtime(&crate_dir, &target);
    }
}

fn link_ios_runtime(crate_dir: &str, target: &str) {
    println!("cargo:rerun-if-env-changed=RIME_LIB_DIR");
    println!("cargo:rerun-if-env-changed=KEYTAO_IOS_RIME_ROOT");

    let Some(lib_dir) = ios_lib_dir(crate_dir, target) else {
        return;
    };
    println!("cargo:rustc-link-search={}", lib_dir.to_string_lossy());
    for lib in [
        "rime",
        "boost_filesystem",
        "boost_regex",
        "boost_system",
        "boost_atomic",
        "glog",
        "leveldb",
        "marisa",
        "opencc",
        "yaml-cpp",
    ] {
        let archive = lib_dir.join(format!("lib{lib}.a"));
        if archive.is_file() {
            println!(
                "cargo:rustc-link-arg=-Wl,-force_load,{}",
                archive.to_string_lossy()
            );
        }
    }
    println!("cargo:rustc-link-lib=c++");
}

fn ios_lib_dir(crate_dir: &str, target: &str) -> Option<PathBuf> {
    if let Ok(lib_dir) = env::var("RIME_LIB_DIR") {
        let lib_dir = PathBuf::from(lib_dir);
        if lib_dir.join("librime.a").is_file() {
            return Some(lib_dir);
        }
    }
    if let Ok(root) = env::var("KEYTAO_IOS_RIME_ROOT") {
        let root = PathBuf::from(root);
        if root.join("lib/librime.a").is_file() {
            return Some(root.join("lib"));
        }
        let runtime_root = root.join(ios_runtime_name(target)?);
        if runtime_root.join("lib/librime.a").is_file() {
            return Some(runtime_root.join("lib"));
        }
    }

    let workspace_root = PathBuf::from(crate_dir).parent()?.parent()?.to_path_buf();
    let lib_dir = workspace_root
        .join("vendor/librime/ios")
        .join(ios_runtime_name(target)?)
        .join("lib");
    lib_dir.join("librime.a").is_file().then_some(lib_dir)
}

fn ios_runtime_name(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-ios" => Some("iphoneos-arm64"),
        "aarch64-apple-ios-sim" => Some("iphonesimulator-arm64"),
        "x86_64-apple-ios" => Some("iphonesimulator-x86_64"),
        _ => None,
    }
}
