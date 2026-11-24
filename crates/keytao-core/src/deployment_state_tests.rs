use super::deployment_state::cached_deployment_in_progress;
use super::{
    begin_deployment, complete_deployment, deployment_in_progress, schema_install_state,
    DeploymentState,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static ENGINE_LOAD_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn installed_schema() -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "keytao-deployment-state-{}-{suffix}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(dir.join("build")).unwrap();
    fs::write(
        dir.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: keytao\n",
    )
    .unwrap();
    fs::write(
        dir.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\ntranslator:\n  dictionary: keytao_dict\n",
    )
    .unwrap();
    dir
}

fn compiled_schema(dir: &Path) {
    fs::copy(
        dir.join("keytao.schema.yaml"),
        dir.join("build/keytao.schema.yaml"),
    )
    .unwrap();
    fs::write(dir.join("build/keytao_dict.table.bin"), b"table").unwrap();
    fs::write(dir.join("build/keytao_dict.prism.bin"), b"prism").unwrap();
}

#[test]
fn mobile_scheme_progress_names_target_and_clears_after_work() {
    use super::deployment_state::{clear_deploy_progress, write_deploy_progress};
    use super::{
        scheme_operation_progress_message, DeployProgress, DeployStep, SchemeConversionProgress,
    };

    let root = installed_schema();
    assert_eq!(scheme_operation_progress_message(&root), None);
    let older = SchemeConversionProgress::start(&root, "键道6".into()).unwrap();
    let current = SchemeConversionProgress::start(&root, "星猫键道".into()).unwrap();
    drop(older);
    assert!(!deployment_in_progress(&root));
    assert_eq!(
        scheme_operation_progress_message(&root).as_deref(),
        Some("星猫键道 转换中")
    );
    write_deploy_progress(
        &root,
        &DeployProgress::flat("xmjd", "星猫键道", DeployStep::Compile, "编译中"),
    )
    .unwrap();
    assert_eq!(
        scheme_operation_progress_message(&root).as_deref(),
        Some("星猫键道 部署中")
    );
    clear_deploy_progress(&root);
    drop(current);
    assert_eq!(scheme_operation_progress_message(&root), None);
    fs::write(
        root.join(".keytao-conversion-progress.json"),
        r#"{"owner":"1/0","package_name":"已退出"}"#,
    )
    .unwrap();
    assert_eq!(scheme_operation_progress_message(&root), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_transient_deployment_hints_yield_to_real_candidates() {
    for hint in ["部署成功", "部署失败"] {
        assert_eq!(DeploymentState::visible_hint(Some(hint), false), Some(hint));
        assert_eq!(DeploymentState::visible_hint(Some(hint), true), None);
    }
    for has_candidates in [false, true] {
        assert_eq!(
            DeploymentState::visible_hint(Some("正在部署中"), has_candidates),
            Some("正在部署中")
        );
    }
    assert_eq!(DeploymentState::visible_hint(None, false), None);
    assert_eq!(DeploymentState::visible_hint(None, true), None);
}

#[test]
fn schema_install_state_rejects_partial_and_interrupted_builds() {
    let dir = installed_schema();
    fs::copy(
        dir.join("keytao.schema.yaml"),
        dir.join("build/keytao.schema.yaml"),
    )
    .unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/.keytao-deploying"), "interrupted").unwrap();
    let partial = schema_install_state(&dir);
    assert!(partial.installed);
    assert!(!partial.deployed);
    assert!(!dir.join("build/.keytao-deployed").exists());
    compiled_schema(&dir);
    fs::write(dir.join("build/.keytao-deployed"), r#"["keytao"]"#).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn completion_succeeds_and_keeps_stale_file_when_prune_cannot_remove_it() {
    use std::os::unix::fs::PermissionsExt;

    let dir = installed_schema();
    compiled_schema(&dir);
    fs::write(
        dir.join("build/default.yaml"),
        "schema_list: [{schema: keytao}]\n",
    )
    .unwrap();
    let stale = dir.join("build/stale.table.bin");
    fs::write(&stale, b"stale").unwrap();
    begin_deployment(&dir).unwrap();
    super::complete_deployment_with_cleanup(&dir, || {
        assert!(deployment_in_progress(&dir));
        assert!(!dir.join("build/.keytao-deployed").exists());
        let build = dir.join("build");
        let permissions = fs::metadata(&build).unwrap().permissions();
        fs::set_permissions(&build, fs::Permissions::from_mode(0o555)).unwrap();
        let result = super::prune_rime_build_artifacts(&dir, None);
        fs::set_permissions(&build, permissions).unwrap();
        result.map(|_| ())
    })
    .unwrap();
    assert_eq!(fs::read(stale).unwrap(), b"stale");
    assert!(!deployment_in_progress(&dir));
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(dir.join("build/.keytao-deployed").is_file());
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_accepts_successful_deployment_marker() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(
        marker["fingerprint"],
        format!("keytao-core/{};bundled/", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(marker["schemas"], serde_json::json!(["keytao"]));
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_rejects_mismatched_fingerprint_without_rewriting_it() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let path = dir.join("build/.keytao-deployed");
    let mut marker: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    marker["fingerprint"] = "previous-engine-version".into();
    let content = serde_json::to_vec(&marker).unwrap();
    fs::write(&path, &content).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    assert_eq!(fs::read(&path).unwrap(), content);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_rejects_old_plain_schema_list() {
    let dir = installed_schema();
    compiled_schema(&dir);
    let marker = r#"["keytao"]"#;
    fs::write(dir.join("build/.keytao-deployed"), marker).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    assert_eq!(
        fs::read_to_string(dir.join("build/.keytao-deployed")).unwrap(),
        marker
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_sentinel_overrides_matching_fingerprint() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    fs::write(dir.join("build/.keytao-deploying"), "interrupted").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    assert!(dir.join("build/.keytao-deploying").is_file());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_rejects_bundled_apk_version_change() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::create_dir(dir.join("rime-data")).unwrap();
    let stamp = dir.join("rime-data/.bundled-version");
    fs::write(&stamp, "42/1.2.5").unwrap();
    complete_deployment(&dir).unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::write(&stamp, "43/1.2.6").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    complete_deployment(&dir).unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::remove_file(&stamp).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_requires_redeployment_for_ios_version_and_build_changes() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::create_dir(dir.join("rime-data")).unwrap();
    let stamp = dir.join("rime-data/.bundled-version");
    fs::write(&stamp, "1.2.5/42").unwrap();
    complete_deployment(&dir).unwrap();

    for bundled_version in ["1.2.6/42", "1.2.6/43"] {
        let previous_marker = fs::read(dir.join("build/.keytao-deployed")).unwrap();
        fs::write(&stamp, bundled_version).unwrap();
        let state = schema_install_state(&dir);
        assert!(state.installed);
        assert!(!state.deployed);
        assert_eq!(
            fs::read(dir.join("build/.keytao-deployed")).unwrap(),
            previous_marker
        );

        begin_deployment(&dir).unwrap();
        assert!(!schema_install_state(&dir).deployed);
        complete_deployment(&dir).unwrap();
        assert!(schema_install_state(&dir).deployed);
        assert!(!dir.join("build/.keytao-deploying").exists());
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
        assert_eq!(
            marker["fingerprint"],
            format!(
                "keytao-core/{};bundled/{bundled_version}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deploy_start_invalidates_previous_ready_build_until_completion() {
    let dir = installed_schema();
    compiled_schema(&dir);
    fs::write(dir.join("build/.keytao-deployed"), r#"["keytao"]"#).unwrap();
    begin_deployment(&dir).unwrap();
    assert!(dir.join("build/.keytao-deploying").is_file());
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!schema_install_state(&dir).deployed);
    complete_deployment(&dir).unwrap();
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_requires_redeployment_of_complete_legacy_build() {
    let dir = installed_schema();
    compiled_schema(&dir);
    assert!(!dir.join("build/.keytao-deploying").exists());
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!schema_install_state(&dir).deployed);
    assert!(!dir.join("build/.keytao-deployed").exists());
    assert!(!dir.join("build/.keytao-deploying").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_marker_requires_compiled_schema() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    assert!(schema_install_state(&dir).deployed);
    fs::remove_file(dir.join("build/keytao.schema.yaml")).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_hint_waits_for_loaded_build_and_expires() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let mut state = DeploymentState::default();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    begin_deployment(&dir).unwrap();
    assert!(deployment_in_progress(&dir));
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    state.key_pressed(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    complete_deployment(&dir).unwrap();
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("部署成功"));
    assert_eq!(
        state.hint_at(
            &dir,
            std::time::Instant::now() + std::time::Duration::from_secs(2)
        ),
        None
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_hint_failure_expires_and_new_deploy_can_retry() {
    let dir = installed_schema();
    let mut state = DeploymentState::default();
    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    state.failed();
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("部署失败"));
    assert_eq!(
        state.hint_at(
            &dir,
            std::time::Instant::now() + std::time::Duration::from_secs(2)
        ),
        None
    );
    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    fs::remove_file(dir.join("build/.keytao-deploying")).unwrap();
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("部署失败"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_sentinel_requires_live_owner_and_recent_timestamp() {
    let dir = installed_schema();
    let path = dir.join("build/.keytao-deploying");
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    for timestamp in [now.as_nanos(), now.as_millis()] {
        fs::write(&path, format!("{timestamp}/{}", std::process::id())).unwrap();
        assert!(deployment_in_progress(&dir));
    }
    for content in [
        "interrupted".to_owned(),
        format!("{}/{}", now.as_nanos(), u32::MAX),
        format!("{}/{}", now.as_nanos(), 0),
        format!(
            "{}/{}",
            now.as_nanos() - 601_000_000_000,
            std::process::id()
        ),
        format!("{}/{}", now.as_nanos() + 60_000_000_000, std::process::id()),
    ] {
        fs::write(&path, content).unwrap();
        assert!(!deployment_in_progress(&dir));
    }
    let mut state = DeploymentState::default();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    assert_eq!(
        state.hint_at(
            &dir,
            std::time::Instant::now() + std::time::Duration::from_secs(2)
        ),
        None
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_failure_hint_does_not_allow_reading_an_active_build() {
    let dir = installed_schema();
    begin_deployment(&dir).unwrap();
    let mut state = DeploymentState::default();
    state.key_pressed(&dir);
    super::mark_deployment_failed(&dir).unwrap();
    assert_eq!(
        super::deployment_state::failed_deployment_owned_by_process(&dir),
        !cfg!(target_os = "android")
    );
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("部署失败"));
    assert!(deployment_in_progress(&dir));
    assert_eq!(
        state.hint_at(
            &dir,
            std::time::Instant::now() + std::time::Duration::from_secs(2)
        ),
        None
    );
    begin_deployment(&dir).unwrap();
    assert!(!super::deployment_state::failed_deployment_owned_by_process(&dir));
    state.key_pressed(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_hint_detects_a_build_replaced_between_watcher_ticks() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let mut state = DeploymentState::default();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    let marker = dir.join("build/.keytao-deployed");
    let content = fs::read_to_string(&marker).unwrap();
    fs::write(&marker, format!("{content}\n")).unwrap();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    state.key_pressed(&dir);
    assert_eq!(state.cached_hint(), Some("正在部署中"));
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("部署成功"));
    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("正在部署中"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_hint_refreshes_at_most_every_250ms() {
    let dir = installed_schema();
    let mut state = DeploymentState::default();
    let now = Instant::now();
    assert_eq!(state.cached_hint_at(&dir, now), None);
    begin_deployment(&dir).unwrap();
    assert_eq!(
        state.cached_hint_at(&dir, now + Duration::from_millis(249)),
        None
    );
    assert_eq!(state.cached_hint(), None);
    assert!(!cached_deployment_in_progress(&dir));
    assert_eq!(
        state.cached_hint_at(&dir, now + Duration::from_millis(250)),
        None
    );
    assert!(cached_deployment_in_progress(&dir));
    state.key_pressed(&dir);
    let now = Instant::now();
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(state.cached_hint(), Some("正在部署中"));
    assert_eq!(
        state.cached_hint_at(&dir, now + Duration::from_millis(249)),
        Some("正在部署中")
    );
    assert!(cached_deployment_in_progress(&dir));
    assert_eq!(
        state.cached_hint_at(&dir, now + Duration::from_millis(251)),
        Some("部署失败")
    );
    assert!(!cached_deployment_in_progress(&dir));
}

#[test]
fn deployment_without_a_waiting_key_stays_silent_even_after_loading() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let mut state = DeploymentState::default();
    state.key_pressed(&dir);
    begin_deployment(&dir).unwrap();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    complete_deployment(&dir).unwrap();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_input_reset_hides_pending_and_success_hints() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let mut state = DeploymentState::default();
    state.hint_at(&dir, Instant::now());
    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    assert_eq!(state.cached_hint(), Some("正在部署中"));
    state.reset_input();
    assert_eq!(state.cached_hint(), None);
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    complete_deployment(&dir).unwrap();
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), None);

    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    complete_deployment(&dir).unwrap();
    state.loaded(&dir);
    assert_eq!(state.hint_at(&dir, Instant::now()), Some("部署成功"));
    state.reset_input();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);

    begin_deployment(&dir).unwrap();
    assert_eq!(state.hint_at(&dir, Instant::now()), None);
    state.key_pressed(&dir);
    assert_eq!(state.cached_hint(), Some("正在部署中"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_hint_stops_when_a_session_is_usable_during_prewarm() {
    let _engine_load = super::lock_ignore_poison(&ENGINE_LOAD_TEST);
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let mut state = DeploymentState::default();
    state.hint_at(&dir, Instant::now());
    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    complete_deployment(&dir).unwrap();
    let _usable = super::deployment_state::engine_loaded(&dir);
    let hint = state.hint_at_with_build(&dir, Instant::now(), true);
    assert_eq!(hint, None);
    assert_eq!(DeploymentState::visible_hint(hint, true), None);
    assert!(!state.blocks_input(&dir));
    let hint = state.hint_at_with_build(&dir, Instant::now(), false);
    assert_eq!(hint, Some("部署成功"));
    assert_eq!(DeploymentState::visible_hint(hint, true), None);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_live_checks_refresh_the_memory_cache() {
    let dir = installed_schema();
    begin_deployment(&dir).unwrap();
    assert!(deployment_in_progress(&dir));
    fs::remove_dir_all(&dir).unwrap();
    assert!(cached_deployment_in_progress(&dir));
    assert!(!deployment_in_progress(&dir));
    assert!(!cached_deployment_in_progress(&dir));
}

#[test]
fn deployment_only_blocks_and_hints_without_a_usable_session() {
    for usable in [false, true] {
        for failed in [false, true] {
            let dir = installed_schema();
            begin_deployment(&dir).unwrap();
            let mut state = DeploymentState::default();
            state.hint_at_with_session(&dir, Instant::now(), false, usable);
            state.key_pressed_with_session(&dir, usable);
            if failed {
                super::mark_deployment_failed(&dir).unwrap();
                state = DeploymentState::default();
                state.key_pressed_with_session(&dir, usable);
            }
            let hint = state.hint_at_with_session(&dir, Instant::now(), false, usable);
            assert_eq!(state.blocks_input_with_session(usable), !usable && !failed);
            assert_eq!(
                hint,
                if usable {
                    None
                } else if failed {
                    Some("部署失败")
                } else {
                    Some("正在部署中")
                }
            );
            assert_eq!(
                state.hint_at_with_session(&dir, Instant::now(), false, true),
                None
            );
            assert_eq!(
                state.hint_at_with_session(&dir, Instant::now(), true, true),
                None
            );
            fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[test]
fn deployment_reload_after_a_usable_session_never_blocks_or_hints() {
    let _engine_load = super::lock_ignore_poison(&ENGINE_LOAD_TEST);
    for (startup_pending, observe_usable) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let dir = installed_schema();
        let mut state = DeploymentState::default();
        assert_eq!(state.hint_at(&dir, Instant::now()), None);
        compiled_schema(&dir);
        complete_deployment(&dir).unwrap();
        if !startup_pending {
            state.loaded(&dir);
        }
        assert_eq!(state.hint_at(&dir, Instant::now()), None);
        let usable = super::deployment_state::engine_loaded(&dir);
        if observe_usable {
            state.key_pressed(&dir);
            assert_eq!(state.hint(&dir), None);
        }
        begin_deployment(&dir).unwrap();
        complete_deployment(&dir).unwrap();
        drop(usable);
        state.key_pressed(&dir);
        assert!(!state.blocks_input(&dir));
        assert_eq!(state.hint_at_with_build(&dir, Instant::now(), true), None);
        let _usable = super::deployment_state::engine_loaded(&dir);
        assert_eq!(state.hint_at_with_build(&dir, Instant::now(), false), None);
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deployment_dead_failed_owner_shows_failure_without_blocking_keys() {
    for waiting_key in [false, true] {
        let dir = installed_schema();
        let mut state = DeploymentState::default();
        begin_deployment(&dir).unwrap();
        if waiting_key {
            state.key_pressed(&dir);
            assert!(state.blocks_input(&dir));
            assert_eq!(state.cached_hint(), Some("正在部署中"));
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        fs::write(
            dir.join("build/.keytao-deploying"),
            format!("{}/{}", now.as_nanos(), u32::MAX),
        )
        .unwrap();
        assert!(!schema_install_state(&dir).deployed);
        for _ in 0..2 {
            state.key_pressed(&dir);
            assert_eq!(state.cached_hint(), Some("部署失败"));
            assert!(!state.blocks_input(&dir));
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deployment_usability_ends_with_the_last_loaded_session() {
    let dir = installed_schema();
    let usable = super::track_usable_input_session(&dir);
    let mut state = DeploymentState::default();
    begin_deployment(&dir).unwrap();
    state.key_pressed(&dir);
    assert!(!state.blocks_input(&dir));
    assert_eq!(state.hint(&dir), None);
    drop(usable);
    state.key_pressed(&dir);
    assert!(state.blocks_input(&dir));
    assert_eq!(state.hint(&dir), Some("正在部署中"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deployment_completion_keeps_sentinel_when_marker_write_fails() {
    let dir = installed_schema();
    compiled_schema(&dir);
    begin_deployment(&dir).unwrap();
    fs::create_dir(dir.join("build/.keytao-deployed")).unwrap();
    assert!(complete_deployment(&dir).is_err());
    assert!(!schema_install_state(&dir).deployed);
    assert!(dir.join("build/.keytao-deployed").is_dir());
    assert!(dir.join("build/.keytao-deploying").is_file());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_ignores_optional_dependency_without_source() {
    let dir = installed_schema();
    fs::write(
        dir.join("keytao.schema.yaml"),
        "schema:\n  schema_id: keytao\n  dependencies: [optional_missing]\ntranslator:\n  dictionary: keytao_dict\n",
    )
    .unwrap();
    compiled_schema(&dir);
    assert!(!dir.join("optional_missing.schema.yaml").exists());
    assert!(!schema_install_state(&dir).deployed);
    begin_deployment(&dir).unwrap();
    complete_deployment(&dir).unwrap();
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("build/.keytao-deployed")).unwrap()).unwrap();
    assert_eq!(marker["schemas"], serde_json::json!(["keytao"]));
    assert!(schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_install_state_requires_marker_to_list_every_managed_primary() {
    let dir = installed_schema();
    compiled_schema(&dir);
    complete_deployment(&dir).unwrap();
    let path = dir.join("build/.keytao-deployed");
    let mut marker: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    marker["schemas"] = serde_json::json!(["stale"]);
    fs::write(&path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::write(dir.join("build/.keytao-deployed"), "invalid json").unwrap();
    assert!(!schema_install_state(&dir).deployed);
    fs::remove_dir_all(dir).unwrap();
}
