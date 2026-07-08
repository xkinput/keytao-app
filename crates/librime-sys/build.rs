use std::env;
use std::path::PathBuf;

const DEFAULT_INCLUDE_DIR: &str = "/usr/include";
const DEFAULT_LIB_DIR: &str = "/usr/lib";

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let mut include_dir = None;
    let mut lib_dir = None;
    let mut bindgen_args = Vec::new();

    if let Ok(e) = env::var("RIME_INCLUDE_DIR") {
        include_dir = Some(e);
    }
    if let Ok(e) = env::var("RIME_LIB_DIR") {
        lib_dir = Some(e);
    }
    if let Ok(extra_args) = env::var("BINDGEN_EXTRA_CLANG_ARGS") {
        bindgen_args.extend(extra_args.split_whitespace().map(str::to_owned));
    }

    if target.contains("android") {
        let Some(root) = android_rime_root(&target) else {
            panic!(
                "Android target {target} requires an imported librime runtime. \
                 Run scripts/android-librime-runtime.sh import-sdk/import-fcitx5-rime for the matching ABI, \
                 or set KEYTAO_ANDROID_RIME_ROOT/RIME_INCLUDE_DIR/RIME_LIB_DIR."
            );
        };
        include_dir.get_or_insert_with(|| root.join("include").to_string_lossy().into_owned());
        lib_dir.get_or_insert_with(|| root.join("lib").to_string_lossy().into_owned());
        bindgen_args.extend(android_bindgen_args(&target));
    }
    if target.contains("apple-ios") {
        let Some(root) = ios_rime_root(&target) else {
            panic!(
                "iOS target {target} requires an imported librime runtime. \
                 Run scripts/ios-librime-runtime.sh import-sdk for the matching target, \
                 or set KEYTAO_IOS_RIME_ROOT/RIME_INCLUDE_DIR/RIME_LIB_DIR."
            );
        };
        include_dir.get_or_insert_with(|| root.join("include").to_string_lossy().into_owned());
        lib_dir.get_or_insert_with(|| root.join("lib").to_string_lossy().into_owned());
        bindgen_args.extend(ios_bindgen_args(&target));
    }

    let include_dir = include_dir.unwrap_or_else(|| DEFAULT_INCLUDE_DIR.to_owned());
    let lib_dir = lib_dir.unwrap_or_else(|| DEFAULT_LIB_DIR.to_owned());

    println!("cargo:rerun-if-env-changed=RIME_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=RIME_LIB_DIR");
    println!("cargo:rerun-if-env-changed=BINDGEN_EXTRA_CLANG_ARGS");
    println!("cargo:rerun-if-env-changed=KEYTAO_ANDROID_RIME_ROOT");
    println!("cargo:rerun-if-env-changed=KEYTAO_IOS_RIME_ROOT");
    println!("cargo:rerun-if-env-changed=KEYTAO_RIME_LINK_KIND");
    println!("cargo:rerun-if-env-changed=SDKROOT");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_ROOT");
    println!("cargo:rerun-if-env-changed=NDK_HOME");
    println!("cargo:rustc-link-search={}", lib_dir);

    let link_kind = env::var("KEYTAO_RIME_LINK_KIND").unwrap_or_else(|_| {
        if target.contains("apple-ios") && PathBuf::from(&lib_dir).join("librime.a").is_file() {
            "static".to_owned()
        } else {
            "dylib".to_owned()
        }
    });
    match link_kind.as_str() {
        "static" => println!("cargo:rustc-link-lib=static=rime"),
        _ => println!("cargo:rustc-link-lib=rime"),
    }
    if target.contains("apple-ios") && link_kind == "static" {
        link_ios_static_dependencies(&lib_dir);
    }
    if target.contains("apple-ios") {
        println!("cargo:rustc-link-lib=c++");
    }

    let mut builder = bindgen::Builder::default()
        .header(
            PathBuf::from(include_dir)
                .join("rime_api.h")
                .to_string_lossy(),
        )
        .header("./include/keycodes.h")
        .header("./include/modifiers.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks));
    for arg in bindgen_args {
        builder = builder.clang_arg(arg);
    }

    let bindings = builder.generate().expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn link_ios_static_dependencies(lib_dir: &str) {
    let lib_dir = PathBuf::from(lib_dir);
    for lib in [
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
        if lib_dir.join(format!("lib{lib}.a")).is_file() {
            println!("cargo:rustc-link-lib=static={lib}");
        }
    }
}

fn android_rime_root(target: &str) -> Option<PathBuf> {
    if let Ok(root) = env::var("KEYTAO_ANDROID_RIME_ROOT") {
        let root = PathBuf::from(root);
        if root.join("include/rime_api.h").is_file() {
            return Some(root);
        }
        let abi_root = root.join(android_abi(target)?);
        if abi_root.join("include/rime_api.h").is_file() {
            return Some(abi_root);
        }
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    let workspace_root = manifest_dir.parent()?.parent()?;
    let root = workspace_root
        .join("vendor/librime/android")
        .join(android_abi(target)?);
    root.join("include/rime_api.h").is_file().then_some(root)
}

fn android_abi(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-linux-android" => Some("arm64-v8a"),
        "armv7-linux-androideabi" => Some("armeabi-v7a"),
        "i686-linux-android" => Some("x86"),
        "x86_64-linux-android" => Some("x86_64"),
        _ => None,
    }
}

fn android_bindgen_args(target: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(clang_target) = android_clang_target(target) {
        args.push(format!("--target={clang_target}"));
    }
    let Some(sysroot) = android_ndk_sysroot() else {
        panic!(
            "Android target {target} requires an Android NDK sysroot for bindgen. \
             Set ANDROID_NDK_HOME, ANDROID_NDK_ROOT, or NDK_HOME."
        );
    };
    args.push(format!("--sysroot={}", sysroot.to_string_lossy()));
    args
}

fn android_clang_target(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-linux-android" => Some("aarch64-linux-android24"),
        "armv7-linux-androideabi" => Some("armv7a-linux-androideabi24"),
        "i686-linux-android" => Some("i686-linux-android24"),
        "x86_64-linux-android" => Some("x86_64-linux-android24"),
        _ => None,
    }
}

fn android_ndk_sysroot() -> Option<PathBuf> {
    for key in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"] {
        if let Ok(root) = env::var(key) {
            let sysroot = PathBuf::from(root)
                .join("toolchains")
                .join("llvm")
                .join("prebuilt")
                .join(android_ndk_host_tag()?)
                .join("sysroot");
            if sysroot.is_dir() {
                return Some(sysroot);
            }
        }
    }
    None
}

fn android_ndk_host_tag() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("darwin-x86_64")
    } else if cfg!(target_os = "linux") {
        Some("linux-x86_64")
    } else if cfg!(target_os = "windows") {
        Some("windows-x86_64")
    } else {
        None
    }
}

fn ios_rime_root(target: &str) -> Option<PathBuf> {
    if let Ok(root) = env::var("KEYTAO_IOS_RIME_ROOT") {
        let root = PathBuf::from(root);
        if root.join("include/rime_api.h").is_file() {
            return Some(root);
        }
        let runtime_root = root.join(ios_runtime_name(target)?);
        if runtime_root.join("include/rime_api.h").is_file() {
            return Some(runtime_root);
        }
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    let workspace_root = manifest_dir.parent()?.parent()?;
    let root = workspace_root
        .join("vendor/librime/ios")
        .join(ios_runtime_name(target)?);
    root.join("include/rime_api.h").is_file().then_some(root)
}

fn ios_runtime_name(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-ios" => Some("iphoneos-arm64"),
        "aarch64-apple-ios-sim" => Some("iphonesimulator-arm64"),
        "x86_64-apple-ios" => Some("iphonesimulator-x86_64"),
        _ => None,
    }
}

fn ios_bindgen_args(target: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(clang_target) = ios_clang_target(target) {
        args.push(format!("--target={clang_target}"));
    }
    let sdk = if target == "aarch64-apple-ios" {
        "iphoneos"
    } else {
        "iphonesimulator"
    };
    let sdkroot = env::var("SDKROOT")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| ios_sdkroot(sdk))
        .unwrap_or_else(|| panic!("iOS target {target} requires an {sdk} SDKROOT"));
    args.push(format!("-isysroot"));
    args.push(sdkroot.to_string_lossy().into_owned());
    args
}

fn ios_clang_target(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-ios" => Some("arm64-apple-ios15.0"),
        "aarch64-apple-ios-sim" => Some("arm64-apple-ios15.0-simulator"),
        "x86_64-apple-ios" => Some("x86_64-apple-ios15.0-simulator"),
        _ => None,
    }
}

fn ios_sdkroot(sdk: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(value.trim());
    path.is_dir().then_some(path)
}
