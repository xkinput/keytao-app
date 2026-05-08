fn main() {
    let rime_ver = std::process::Command::new("pkg-config")
        .args(["--modversion", "librime"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RIME_VERSION={rime_ver}");
    println!("cargo:rerun-if-changed=build.rs");
}
