#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{ImeRuntime, ReloadStamp};
use std::path::PathBuf;
use std::sync::Mutex;

static LIBRIME: Mutex<()> = Mutex::new(());

#[test]
fn deployment_reload_with_pending_schema_request_refreshes_dictionary() {
    deployment_reload(false);
}

#[test]
fn schema_selection_after_deployment_refreshes_dictionary() {
    deployment_reload(true);
}

fn deployment_reload(schema_stamp: bool) {
    let _serial = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    const CHILD_ROOT: &str = "KEYTAO_SCHEMA_DEPLOY_RELOAD_CHILD_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let dictionary = root.join(format!("{}.dict.yaml", smoke_fixture::SCHEMA_ID));
        let content = std::fs::read_to_string(&dictionary).unwrap();
        std::fs::write(
            dictionary,
            content
                .replace("version: \"1\"", "version: \"2\"")
                .replace("\taa\n", "\tjd\n"),
        )
        .unwrap();
        keytao_core::deploy(
            root.to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
        )
        .unwrap();
        ReloadStamp::write(&root).unwrap();
        return;
    }

    let root = smoke_fixture::scratch_dir("schema-deploy-reload");
    smoke_fixture::write(&root).unwrap();
    let runtime = ImeRuntime::with_dirs(&root, root.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    for key in smoke_fixture::CODE.chars() {
        session.process_key_result(key as u32, 0).unwrap();
    }
    assert!(!session.state().candidates.is_empty());
    session.reset().unwrap();

    let child_test = if schema_stamp {
        "schema_selection_after_deployment_refreshes_dictionary"
    } else {
        "deployment_reload_with_pending_schema_request_refreshes_dictionary"
    };
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", child_test, "--nocapture"])
        .env(CHILD_ROOT, &root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "deployment child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    keytao_core::request_schema(&root, smoke_fixture::SCHEMA_ID).unwrap();
    if schema_stamp {
        ReloadStamp::write_schema(&root).unwrap();
    }
    runtime.reload_without_deploy().unwrap();
    for key in "jd".chars() {
        session.process_key_result(key as u32, 0).unwrap();
    }
    let state = session.state();
    assert_eq!(state.schema_id, smoke_fixture::SCHEMA_ID);
    assert!(
        state
            .candidates
            .iter()
            .any(|candidate| candidate.text == "甲"),
        "deployed dictionary must accept jd after switching: {:?}",
        state.candidates
    );
    drop(session);
    keytao_core::runtime_log::flush_blocking(std::time::Duration::from_secs(1));
    std::fs::remove_dir_all(root).unwrap();
}
