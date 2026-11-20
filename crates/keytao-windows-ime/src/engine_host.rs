pub(crate) fn allow_in_process_fallback(host_exe_name: &str) -> bool {
    !host_exe_name.is_empty()
        && ![
            "keytao-app.exe",
            "keytao.exe",
            "keytao-windows-ime-server.exe",
            "keytao-setup-helper.exe",
            "keytao-setup-helper-x64.exe",
            "keytao-setup-helper-arm64.exe",
        ]
        .iter()
        .any(|name| host_exe_name.eq_ignore_ascii_case(name))
}
