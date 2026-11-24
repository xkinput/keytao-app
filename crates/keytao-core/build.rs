fn main() {
    println!("cargo:rustc-check-cfg=cfg(keytao_ios_libjd)");
    println!("cargo:rerun-if-env-changed=KEYTAO_IOS_LIBJD_DIR");
    if std::env::var("TARGET").as_deref() != Ok("aarch64-apple-ios") {
        return;
    }
    let Some(directory) = std::env::var_os("KEYTAO_IOS_LIBJD_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let archive = directory.join("libjd-engine.a");
    assert!(archive.is_file(), "missing {}", archive.display());
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo:rustc-link-search=native={}", directory.display());
    println!("cargo:rustc-link-lib=static=jd-engine");
    println!("cargo:rustc-cfg=keytao_ios_libjd");
}
