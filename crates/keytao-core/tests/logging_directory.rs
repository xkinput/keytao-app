//! glog outlives librime's runtime and keeps the first setup's log directory.
//! Exercise the real deployment path after that directory disappears, including
//! libraries that do not catch filesystem exceptions in clean_old_log_files.

#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::ImeRuntime;

#[test]
fn redeploy_restores_the_process_wide_log_directory() {
    let first_dir = smoke_fixture::scratch_dir("first-log-directory");
    let second_dir = smoke_fixture::scratch_dir("second-log-directory");
    smoke_fixture::write(&first_dir).unwrap();
    smoke_fixture::write(&second_dir).unwrap();
    let original_log = first_dir.join("log");
    #[cfg(windows)]
    prepare_removable_windows_log_directory(&original_log);
    let first = ImeRuntime::with_dirs(&first_dir, first_dir.to_string_lossy().into_owned());
    let second = ImeRuntime::with_dirs(&second_dir, second_dir.to_string_lossy().into_owned());
    first.init().expect("deploy the first runtime");

    #[cfg(windows)]
    std::fs::remove_dir(&original_log).unwrap();
    #[cfg(not(windows))]
    std::fs::remove_dir_all(&original_log).unwrap();
    assert!(!original_log.exists());
    second
        .init()
        .expect("deploy after the first log directory disappeared");
    assert!(
        original_log.is_dir(),
        "deployment must restore the first setup's directory, not only the new runtime's"
    );
    let session = second.create_session().unwrap();
    for character in smoke_fixture::CODE.chars() {
        session.process_key_result(character as u32, 0).unwrap();
    }
    assert!(!session.state().candidates.is_empty());
    drop(session);

    // An obstruction must become a Rust error before entering old librime's
    // synchronous directory iterator, and a later retry must remain usable.
    std::fs::remove_dir(&original_log).unwrap();
    std::fs::write(&original_log, b"not a directory").unwrap();
    let error = second
        .reload()
        .expect_err("a file blocks the log directory");
    assert!(error.contains("cannot prepare process-wide librime log directory"));
    assert!(error.contains(original_log.to_str().unwrap()));
    std::fs::remove_file(&original_log).unwrap();
    second
        .reload()
        .expect("retry after removing the obstruction");
    assert!(original_log.is_dir());
    let session = second.create_session().unwrap();
    assert!(session.process_key_result(b'a' as u32, 0).unwrap().accepted);

    // Process-global glog handles stay open until this test binary exits.
    // The host's temporary-directory cleanup can remove both roots afterward.
}

#[cfg(windows)]
fn prepare_removable_windows_log_directory(path: &std::path::Path) {
    // glog opens files without FILE_SHARE_DELETE, preventing a directory that
    // contains those files from being removed or renamed. A directory junction
    // lets the test remove the configured path while keeping those handles in
    // its target. Junction creation does not require symlink privileges.
    let target = smoke_fixture::scratch_dir("open-log-handles");
    std::fs::create_dir_all(&target).unwrap();
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:KEYTAO_TEST_LOG_LINK -Target $env:KEYTAO_TEST_LOG_TARGET | Out-Null",
        ])
        .env("KEYTAO_TEST_LOG_LINK", path)
        .env("KEYTAO_TEST_LOG_TARGET", target)
        .output()
        .expect("create the test's Windows log directory junction");
    assert!(
        output.status.success(),
        "cannot create the test's Windows log directory junction: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
