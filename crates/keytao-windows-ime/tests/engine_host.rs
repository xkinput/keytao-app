#[path = "../src/engine_host.rs"]
mod engine_host;

use engine_host::allow_in_process_fallback;

#[test]
fn keytao_hosts_must_not_initialize_a_second_librime() {
    for name in [
        "keytao-app.exe",
        "KEYTAO-APP.EXE",
        "keytao.exe",
        "keytao-windows-ime-server.exe",
        "keytao-setup-helper.exe",
        "keytao-setup-helper-x64.exe",
        "keytao-setup-helper-arm64.exe",
    ] {
        assert!(!allow_in_process_fallback(name), "host: {name}");
    }
}

#[test]
fn other_hosts_keep_the_existing_fallback() {
    for name in [
        "notepad.exe",
        "chrome.exe",
        "编辑器.exe",
        "keytao-app-other.exe",
        "keytao-app.exe.bak",
    ] {
        assert!(allow_in_process_fallback(name), "host: {name}");
    }
    assert!(!allow_in_process_fallback(""));
}
