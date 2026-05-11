fn main() {
    #[cfg(target_os = "linux")]
    prepare_linux_ime_sidecar();

    tauri_build::build()
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
