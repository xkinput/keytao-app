fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let host = std::env::var("HOST").unwrap_or_default();

    println!("cargo:rerun-if-changed=resource.rc");
    println!("cargo:rerun-if-changed=ime-brand.ico");

    if target_os == "windows" && host.contains("windows") {
        embed_resource::compile("resource.rc", embed_resource::NONE)
            .manifest_required()
            .expect("compile Windows IME resources");
    } else if target_os == "windows" {
        println!(
            "cargo:warning=skipping Windows resource compilation on non-Windows host; release builds must run on Windows"
        );
    }

    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg=/DELAYLOAD:rime.dll");
        println!("cargo:rustc-link-lib=dylib=delayimp");
    }
}
