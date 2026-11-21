//! Exercise reloads and contention with isolated native fixtures and runtime locks.

use super::*;
use std::sync::mpsc;

#[path = "../tests/support/smoke_fixture.rs"]
mod smoke_fixture;

thread_local! {
    static ENGINE_CREATION_GATE: std::cell::RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
    static ENGINE_ACCESS_BUDGET: std::cell::RefCell<Option<(usize, usize)>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn allow_engine_access() -> bool {
    ENGINE_ACCESS_BUDGET.with(|budget| {
        let mut budget = budget.borrow_mut();
        let Some((remaining, attempts)) = budget.as_mut() else {
            return true;
        };
        *attempts += 1;
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    })
}

pub(super) fn pause_engine_creation() {
    ENGINE_CREATION_GATE.with(|gate| {
        if let Some((ready, resume)) = gate.take() {
            ready.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    });
}

fn native_fixture(name: &str) -> Option<(ImeRuntime, PathBuf)> {
    const CASE: &str = "KEYTAO_RUNTIME_RELOAD_TEST";
    if std::env::var(CASE).as_deref() != Ok(name) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("runtime_access_tests::{name}"),
                "--nocapture",
            ])
            .env(CASE, name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().unwrap();
    Some((runtime, dir))
}

#[test]
fn package_stamp_reloads_live_sessions_and_preserves_each_schema() {
    let Some((runtime, root)) =
        native_fixture("package_stamp_reloads_live_sessions_and_preserves_each_schema")
    else {
        return;
    };
    runtime.shutdown().unwrap();
    std::fs::create_dir(root.join("runtime")).unwrap();
    std::fs::write(
        root.join(RuntimeDirs::MIGRATION_MARKER),
        br#"{"version":1,"runtime_package":"first"}"#,
    )
    .unwrap();
    let first = crate::package_store::root(&root).join("cloud/first/files");
    let addon = crate::package_store::root(&root).join("addons/rime_ice/files");
    for (shared, ids) in [
        (&first, &["keytao_a", "keytao_b"][..]),
        (&addon, &["keytao_c", "keytao_d"][..]),
    ] {
        smoke_fixture::write(shared).unwrap();
        let list = ids
            .iter()
            .map(|id| format!("  - schema: {id}\n"))
            .collect::<String>();
        let hotkeys = if shared == &addon { "[Control+Shift+grave]" } else { "[F4, Control+grave]" };
        std::fs::write(
            shared.join("default.yaml"),
            format!("config_version: '1'\nswitcher:\n  caption: Schemas\n  hotkeys: {hotkeys}\nschema_list:\n{list}"),
        )
        .unwrap();
        std::fs::remove_file(shared.join("default.custom.yaml")).unwrap();
        for extension in ["schema.yaml", "dict.yaml"] {
            let source = shared.join(format!("{}.{extension}", smoke_fixture::SCHEMA_ID));
            let content = std::fs::read_to_string(&source).unwrap();
            for id in ids {
                std::fs::write(
                    shared.join(format!("{id}.{extension}")),
                    content.replace(smoke_fixture::SCHEMA_ID, id),
                )
                .unwrap();
            }
            std::fs::remove_file(source).unwrap();
        }
        let key = shared.parent().unwrap().file_name().unwrap();
        let user = root.join("runtime").join(key);
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("default.custom.yaml"),
            format!("patch:\n  schema_list:\n{list}"),
        )
        .unwrap();
    }
    smoke_fixture::write(&crate::package_store::root(&root).join("addons/easy_en/files"))
        .unwrap();
    let target = |key: &str| isolated_deploy::PackageDeployTarget {
        package_key: key.into(),
        package_name: key.into(),
        materialize_error: None,
        staged_inputs: None,
    };
    isolated_deploy::deploy_packages(
        &root,
        &root,
        None,
        &[target("first"), target("rime_ice")],
        |_| {},
    )
    .unwrap();
    std::fs::write(
        root.join(RuntimeDirs::MIGRATION_MARKER),
        br#"{"version":1,"runtime_package":"first"}"#,
    )
    .unwrap();
    runtime.switch_package("first").unwrap();
    runtime.init_without_deploy().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema("keytao_b").unwrap();
    let addon_dirs = RuntimeDirs::for_package(&root, &root, "rime_ice").unwrap();
    let receipt_path = addon_dirs.staging_dir.join(".keytao-desktop-inputs.json");
    let receipt = std::fs::read(&receipt_path).unwrap();
    std::fs::remove_file(&receipt_path).unwrap();
    assert!(session
        .list_switcher_packages()
        .iter()
        .any(|package| package.package_key == "rime_ice" && !package.ready));
    let first_signature = ReloadStamp::current_signature(&root);
    assert_eq!(
        runtime.switch_package("rime_ice"),
        Err("请在键道 App 部署此方案包".into())
    );
    assert_eq!(
        RuntimeDirs::active_package_key(&root).as_deref(),
        Some("first")
    );
    assert_eq!(ReloadStamp::current_signature(&root), first_signature);
    let menu = session.process_key_result(key_policy::XK_F4, 0).unwrap();
    assert!(
        menu.state
            .candidates
            .iter()
            .any(|c| c.text.ends_with(" 未部署")),
        "candidates: {:?}",
        menu.state.candidates
    );
    let unready_idx = menu
        .state
        .candidates
        .iter()
        .position(|c| c.text.ends_with(" 未部署"))
        .unwrap();
    assert_eq!(
        session.select_candidate_on_page(unready_idx).unwrap().schema_id,
        switcher_model::SCHEMA_ID
    );
    session.process_key_result(key_policy::XK_ESCAPE, 0).unwrap();

    std::fs::write(receipt_path, receipt).unwrap();
    let mut gate = ReloadStampGate::new(&root);
    let generation = runtime.generation();
    session.process_key_result(u32::from(b'a'), 0).unwrap();
    let stale = ReloadStamp::write_package(&root, "first").unwrap();
    write_file_atomic(
        &crate::package_store::active_manifest(&root),
        br#"{"active_package":"rime_ice","runtime_migration_version":1}"#,
    )
    .unwrap();
    assert_eq!(ReloadStamp::package_key(&stale).as_deref(), Some("first"));
    let pending = gate.pending().unwrap();
    assert_eq!(
        runtime.reload_without_deploy(),
        Err(PACKAGE_SWITCH_PENDING.into())
    );
    assert_eq!(gate.pending().as_deref(), Some(pending.as_str()));
    assert_eq!(generation, runtime.generation());
    assert_eq!(session.current_schema().unwrap().id, "keytao_b");

    runtime.switch_package("rime_ice").unwrap();
    assert_eq!(
        ReloadStamp::package_key(gate.path()).as_deref(),
        Some("rime_ice")
    );
    assert_eq!(RuntimeDirs::resolve(&root, &root).unwrap(), addon_dirs);
    assert_eq!(session.current_schema().unwrap().id, "keytao_b");
    assert_eq!(generation, runtime.generation());
    let pending = gate.pending().unwrap();
    runtime.reload_without_deploy().unwrap();
    gate.commit(pending);
    assert!(gate.pending().is_none());
    assert!(runtime.generation() > generation);
    assert_eq!(
        desktop::initialized_user_dir().unwrap().unwrap(),
        addon_dirs.user_data_dir
    );
    assert_eq!(session.current_schema().unwrap().id, "keytao_c");
    assert!(session.state().preedit.is_empty());
    runtime.switch_package("first").unwrap();
    runtime.reload_without_deploy().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_b");

    use crate::key_policy::{
        candidate_arrow_key, should_bypass_empty_composition, CandidatePanelLayout, XK_DOWN, XK_F4,
        XK_RETURN, XK_UP,
    };
    let menu = session.process_key_result(XK_F4, 0).unwrap();
    assert!(menu.accepted);
    assert_eq!(menu.state.schema_id, switcher_model::SCHEMA_ID);
    assert!(menu.state.is_package_switcher());
    assert_eq!(menu.state.highlighted_candidate_index, 1);
    assert!(!should_bypass_empty_composition(XK_UP, 0, &menu.state));
    assert!(!should_bypass_empty_composition(XK_DOWN, 0, &menu.state));
    assert!(!should_bypass_empty_composition(XK_RETURN, 0, &menu.state));
    assert!(
        session
            .peek_state()
            .candidates
            .iter()
            .all(|c| !c.text.ends_with(" 未部署"))
    );
    let moved_up = session
        .process_key_result(
            candidate_arrow_key(
                XK_UP,
                0,
                &session.state(),
                CandidatePanelLayout::Horizontal,
            ),
            0,
        )
        .unwrap();
    assert!(moved_up.accepted);
    assert_eq!(moved_up.state.highlighted_candidate_index, 0);
    let moved_down = session
        .process_key_result(
            candidate_arrow_key(
                XK_DOWN,
                0,
                &session.state(),
                CandidatePanelLayout::Horizontal,
            ),
            0,
        )
        .unwrap();
    assert!(moved_down.accepted);
    assert_eq!(moved_down.state.highlighted_candidate_index, 1);
    let closed_on_active = session.process_enter().unwrap();
    assert!(closed_on_active.accepted);
    assert_eq!(closed_on_active.state.schema_id, "keytao_b");
    let ctrl_grave = [(0xffe3, 0), (0x60, RIME_MOD_CONTROL)];
    for expected in [switcher_model::SCHEMA_ID, "keytao_b"] {
        let result = session.process_keys_result(&ctrl_grave).unwrap();
        assert!(result.accepted);
        assert_eq!(result.state.schema_id, expected);
        session
            .process_keys_result(&[
                (0x60, RIME_MOD_CONTROL | RIME_RELEASE_MASK),
                (0xffe3, RIME_RELEASE_MASK),
            ])
            .unwrap();
    }
    let composition = session.process_keys_result(&[(b'a'.into(), 0), (b'a'.into(), 0)]).unwrap();
    assert!(!composition.state.preedit.is_empty());
    let preview = composition.state.commit_preview;
    assert!(!preview.is_empty());
    session.process_key_result(XK_F4, 0).unwrap();
    isolated_deploy::with_isolated_package_lock(&root, || {
        let rejected = session.select_candidate_on_page(3).unwrap();
        assert_eq!(rejected.preedit, composition.state.preedit);
        assert!(rejected.committed.is_none());
        assert_eq!(RuntimeDirs::active_package_key(&root).as_deref(), Some("first"));
        Ok(())
    }).unwrap();
    session.process_key_result(XK_F4, 0).unwrap();
    let generation = runtime.generation();
    let switched = session.select_candidate_on_page(3).unwrap();
    assert_eq!(RuntimeDirs::active_package_key(&root).as_deref(), Some("rime_ice"));
    assert_eq!(switched.schema_id, "keytao_d");
    assert_eq!(switched.committed.as_deref(), Some(preview.as_str()));
    assert!(runtime.generation() > generation);
    assert!(!runtime.reload_without_deploy_changed().unwrap());
    assert_eq!(session.state().schema_id, "keytao_d");
    ReloadStamp::write_package(&root, "rime_ice").unwrap();
    assert!(runtime.reload_without_deploy_changed().unwrap());
    runtime.switch_package("first").unwrap();
    runtime.reload_without_deploy().unwrap();

    let packages = session.list_switcher_packages();
    assert!(packages
        .iter()
        .any(|package| package.package_key == "first" && package.active));
    let other = packages
        .iter()
        .find(|package| package.package_key == "rime_ice")
        .unwrap();
    assert!(other.ready && !other.active);
    assert!(other.schemas.iter().any(|schema| schema.id == "keytao_c"));
    assert_eq!(session.state().schema_id, "keytao_b");
    let pending = session
        .process_keys_result(&[(b'a'.into(), 0), (b'a'.into(), 0)])
        .unwrap();
    assert!(!pending.state.commit_preview.is_empty());
    isolated_deploy::with_isolated_package_lock(&root, || {
        let rejected = session.select_package_schema("rime_ice", "keytao_c").unwrap();
        assert!(!rejected.accepted);
        assert_eq!(rejected.state.preedit, pending.state.preedit);
        assert!(rejected.state.committed.is_none());
        Ok(())
    }).unwrap();
    let selected = session.select_package_schema("rime_ice", "keytao_c").unwrap();
    assert!(selected.accepted);
    assert_eq!(selected.state.schema_id, "keytao_c");
    assert_eq!(
        selected.state.committed.as_deref(),
        Some(pending.state.commit_preview.as_str())
    );
    assert!(session.state().committed.is_none());

    drop(session);
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn key_batch_uses_one_access_and_preserves_an_earlier_commit() {
    let Some((runtime, dir)) =
        native_fixture("key_batch_uses_one_access_and_preserves_an_earlier_commit")
    else {
        return;
    };
    let session = runtime.create_session().unwrap();
    for key in smoke_fixture::CODE.bytes() {
        session.process_key_result(u32::from(key), 0).unwrap();
    }
    ENGINE_ACCESS_BUDGET.with(|budget| *budget.borrow_mut() = Some((1, 0)));
    let result = session.process_keys_result(&[(u32::from(b' '), 0), (0xffe1, RIME_RELEASE_MASK)]);
    let (_, attempts) = ENGINE_ACCESS_BUDGET.with(|budget| budget.borrow_mut().take().unwrap());
    assert_eq!(attempts, 1, "a later key must not reacquire runtime access");
    let result = result.expect("a later access failure must not lose a completed commit");
    assert_eq!(
        result.state.committed.as_deref(),
        Some(smoke_fixture::WORDS[0])
    );
    assert!(result.accepted);
    assert!(result.state.preedit.is_empty());
    drop(session);
    drop(runtime);
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn lua_file_regeneration_does_not_wait_for_librime() {
    let Some((_runtime, dir)) = native_fixture("lua_file_regeneration_does_not_wait_for_librime")
    else {
        return;
    };
    std::fs::write(
        dir.join(ACTIVE_SCHEME_MANIFEST),
        serde_json::json!({
            "root": dir.canonicalize().unwrap(),
            "packages": {"smoke": {
                "schemas": [smoke_fixture::SCHEMA_ID],
                "rime_lua": "file_only = true\n"
            }}
        })
        .to_string(),
    )
    .unwrap();
    let rime = desktop::rime_api_lock();
    let (sent, received) = mpsc::channel();
    let root = dir.clone();
    let worker = std::thread::spawn(move || {
        let result = regenerate_active_rime_lua_for_schema(&root, smoke_fixture::SCHEMA_ID)
            .and_then(|()| regenerate_active_rime_lua(&root));
        let _ = sent.send(result);
    });
    let result = received.recv_timeout(Duration::from_secs(2));
    drop(rime);
    worker.join().unwrap();
    result
        .expect("Lua file regeneration waited for librime")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("rime.lua")).unwrap(),
        "file_only = true\n"
    );
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn schema_lua_switches_reinitialize_before_the_next_key() {
    let Some((runtime, dir)) =
        native_fixture("schema_lua_switches_reinitialize_before_the_next_key")
    else {
        return;
    };
    let ids = [
        "keytao_smoke",
        "keytao_lua_a2",
        "keytao_lua_b",
        "keytao_lua_same",
        "keytao_lua_plain",
    ];
    let list = ids
        .iter()
        .map(|id| format!("    - schema: {id}\n"))
        .collect::<String>();
    std::fs::write(dir.join("default.yaml"), format!("config_version: '2'\nswitcher:\n  caption: Schemas\n  hotkeys: [F4]\n  save_options: []\nschema_list:\n{list}")).unwrap();
    std::fs::write(
        dir.join("default.custom.yaml"),
        format!("patch:\n  switcher/hotkeys: [F4]\n  schema_list:\n{list}"),
    )
    .unwrap();
    for id in ids {
        let global = if id.ends_with("plain") {
            ""
        } else {
            "    - lua_translator@probe_global\n"
        };
        std::fs::write(dir.join(format!("{id}.schema.yaml")), format!("schema:\n  schema_id: {id}\n  name: {id}\n  version: '1'\nswitches:\n  - name: ascii_mode\n    reset: 0\nengine:\n  processors: [ascii_composer, speller, express_editor]\n  segmentors: [ascii_segmentor, abc_segmentor, fallback_segmentor]\n  translators:\n{global}    - lua_translator@*probe\nspeller:\n  alphabet: a\n")).unwrap();
    }
    let lua = |label| {
        format!(
            "function probe_global(input, seg) yield(Candidate('probe',seg.start,seg._end,'{label}-GLOBAL','')) end\n"
        )
    };
    for (key, label) in [("a", "A"), ("b", "B"), ("same", "B"), ("plain", "PLAIN")] {
        let source = dir.join(format!(".keytao-package-lua/{key}/lua"));
        std::fs::create_dir_all(&source).unwrap();
        let source = if matches!(key, "b" | "same") {
            source.join("probe/init.lua")
        } else {
            source.join("probe.lua")
        };
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(source, format!("return function(input, seg) yield(Candidate('probe',seg.start,seg._end,'{label}-MODULE','')) end\n")).unwrap();
    }
    let package = |key: &str, schemas: &[&str], content: Option<String>| {
        let module = if matches!(key, "b" | "same") {
            "lua/probe/init.lua"
        } else {
            "lua/probe.lua"
        };
        serde_json::json!({"schemas": schemas, "rime_lua": content, "lua_files": {module: format!(".keytao-package-lua/{key}/{module}")}})
    };
    let marker = serde_json::json!({"root": dir.canonicalize().unwrap(), "packages": {
        "a": package("a", &ids[..2], Some(lua("A"))),
        "b": package("b", &ids[2..3], Some(lua("B"))),
        "same": package("same", &ids[3..4], Some(lua("B"))),
        "plain": package("plain", &ids[4..], None)
    }});
    std::fs::write(dir.join(ACTIVE_SCHEME_MANIFEST), marker.to_string()).unwrap();
    persist_selected_schema(&dir, ids[0]).unwrap();
    runtime.reload().unwrap();
    // Keep this fixture independent of librime's second-resolution source timestamps.
    let path = dir.join("build/default.yaml");
    let mut config: Value = serde_yaml::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["switcher"] =
        serde_yaml::from_str("caption: Schemas\nhotkeys: [F4]\nsave_options: []\n").unwrap();
    std::fs::write(path, serde_yaml::to_string(&config).unwrap()).unwrap();
    runtime.reload_without_deploy().unwrap();
    let session = runtime.create_session().unwrap();
    let peer = runtime.create_session().unwrap();
    let candidates = |session: &ImeRuntimeSession| {
        runtime.prewarm(Some(session)).unwrap();
        session.reset().unwrap();
        session
            .process_key_result(b'a' as u32, 0)
            .unwrap()
            .state
            .candidates
            .into_iter()
            .map(|candidate| candidate.text)
            .collect::<Vec<_>>()
    };
    assert_eq!(candidates(&session), ["A-GLOBAL", "A-MODULE"]);
    session.reset().unwrap();
    let generation = runtime.generation();
    session.select_schema(ids[1]).unwrap();
    assert_eq!(runtime.generation(), generation);
    let deployed = ReloadStamp::signature_at(&dir.join("build").join(DEPLOYED_FILE));
    request_schema(&dir, ids[2]).unwrap();
    ReloadStamp::write_schema(&dir).unwrap();
    runtime.reload_without_deploy().unwrap();
    assert!(runtime.generation() > generation);
    assert_eq!(session.current_schema().unwrap().id, ids[2]);
    assert_eq!(peer.current_schema().unwrap().id, ids[2]);
    assert_eq!(candidates(&session), ["B-GLOBAL", "B-MODULE"]);
    session.reset().unwrap();
    let generation = runtime.generation();
    session.select_schema(ids[3]).unwrap();
    assert_eq!(
        runtime.generation(),
        generation,
        "identical effective Lua reloaded"
    );
    session.select_schema(ids[4]).unwrap();
    assert!(runtime.generation() > generation);
    assert_eq!(session.current_schema().unwrap().id, ids[4]);
    assert_eq!(candidates(&session), ["PLAIN-MODULE"]);
    assert!(std::fs::read(dir.join("rime.lua"))
        .unwrap_or_default()
        .is_empty());
    assert_eq!(
        ReloadStamp::signature_at(&dir.join("build").join(DEPLOYED_FILE)),
        deployed
    );
    session.reset().unwrap();
    let menu = session.process_key_result(0xffc1, 0).unwrap().state;
    assert!(!menu.candidates.is_empty(), "{menu:?}");
    let index = menu
        .candidates
        .iter()
        .position(|candidate| candidate.text.contains(ids[2]))
        .unwrap();
    session
        .process_key_result(b'1' as u32 + index as u32, 0)
        .unwrap();
    assert_eq!(session.current_schema().unwrap().id, ids[2]);
    assert_eq!(candidates(&session), ["B-GLOBAL", "B-MODULE"]);
    session.reset().unwrap();
    let snapshot = dir.join(".keytao-package-lua/a/lua/probe.lua");
    let snapshot_bytes = std::fs::read(&snapshot).unwrap();
    let lua_bytes = std::fs::read(dir.join("rime.lua")).unwrap();
    std::fs::remove_file(&snapshot).unwrap();
    request_schema(&dir, ids[0]).unwrap();
    let generation = runtime.generation();
    assert_eq!(candidates(&session), ["B-GLOBAL", "B-MODULE"]);
    let deferred = lock_ignore_poison(&PROCESS_RIME.lua_retry_after).clone();
    assert!(deferred.is_some());
    assert_eq!(candidates(&session), ["B-GLOBAL", "B-MODULE"]);
    assert_eq!(*lock_ignore_poison(&PROCESS_RIME.lua_retry_after), deferred);
    assert_eq!(runtime.generation(), generation);
    assert_eq!(std::fs::read(dir.join("rime.lua")).unwrap(), lua_bytes);
    assert!(schema_switch_request::pending_for_process(&dir)
        .unwrap()
        .is_some());
    let during_failure = runtime.create_session().unwrap();
    assert_eq!(during_failure.current_schema().unwrap().id, ids[2]);
    assert_eq!(candidates(&during_failure), ["B-GLOBAL", "B-MODULE"]);
    assert!(schema_switch_request::pending_for_process(&dir)
        .unwrap()
        .is_some());
    drop(during_failure);
    std::fs::write(snapshot, snapshot_bytes).unwrap();
    *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) =
        Some((dir.clone(), Instant::now() - Duration::from_secs(1)));
    assert_eq!(session.current_schema().unwrap().id, ids[0]);
    assert_eq!(candidates(&session), ["A-GLOBAL", "A-MODULE"]);
    assert!(schema_switch_request::pending_for_process(&dir)
        .unwrap()
        .is_none());
    session.reset().unwrap();
    session.select_schema(ids[2]).unwrap();
    let marker_bytes = std::fs::read(dir.join(ACTIVE_SCHEME_MANIFEST)).unwrap();
    let completed = session.with_engine_access(true, false, |_, _, _, _, _| {
        std::fs::write(dir.join(ACTIVE_SCHEME_MANIFEST), "invalid").unwrap();
        rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
        "committed"
    });
    assert_eq!(
        completed,
        Some("committed"),
        "post-action reload discarded completed output"
    );
    assert!(session.process_key_result(b'a' as u32, 0).is_some());
    std::fs::write(dir.join(ACTIVE_SCHEME_MANIFEST), marker_bytes).unwrap();
    session.select_schema(ids[0]).unwrap();
    assert_eq!(candidates(&session), ["A-GLOBAL", "A-MODULE"]);
    session.reset().unwrap();
    session.select_schema(ids[2]).unwrap();
    runtime.prewarm(Some(&session)).unwrap();
    session.state();
    session.reset().unwrap();
    let user_bytes = std::fs::read(dir.join("user.yaml")).unwrap();
    let current_schema = dir.join(".keytao-current-schema");
    std::fs::remove_file(&current_schema).unwrap();
    // Linux librime must still be able to read user.yaml when rebuilding sessions.
    std::fs::create_dir(&current_schema).unwrap();
    session.select_schema(ids[0]).unwrap();
    let retry = Some((dir.clone(), ids[0].to_owned()));
    assert_eq!(*lock_ignore_poison(&rime_lua::SAVE_RETRY), retry);
    // Keep the old on-disk selection so the rebuild must honor the pending save.
    std::fs::write(dir.join("user.yaml"), &user_bytes).unwrap();
    let key = session
        .process_key_result(b'a' as u32, 0)
        .expect("selection persistence failure stopped typing");
    assert!(key.accepted);
    assert_eq!(key.state.schema_id, ids[0]);
    assert_eq!(candidates(&session), ["A-GLOBAL", "A-MODULE"]);
    assert_eq!(*lock_ignore_poison(&rime_lua::SAVE_RETRY), retry);
    std::fs::remove_dir(&current_schema).unwrap();
    *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) = None;
    runtime.prewarm(Some(&session)).unwrap();
    assert!(session.try_finish_lua_reload());
    assert!(lock_ignore_poison(&rime_lua::SAVE_RETRY).is_none());
    assert_eq!(std::fs::read_to_string(&current_schema).unwrap(), ids[0]);
    let user: Value =
        serde_yaml::from_slice(&std::fs::read(dir.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(
        user["var"]["previously_selected_schema"].as_str(),
        Some(ids[0])
    );
    assert_eq!(session.current_schema().unwrap().id, ids[0]);
    assert_eq!(candidates(&session), ["A-GLOBAL", "A-MODULE"]);
    session.reset().unwrap();
    session.select_schema(ids[2]).unwrap();
    drop((session, peer));
    // A fresh process must repair a stale A snapshot before initializing B.
    std::fs::write(dir.join("rime.lua"), lua("A")).unwrap();
    persist_selected_schema(&dir, ids[2]).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime_access_tests::persisted_schema_selects_lua_before_startup",
            "--nocapture",
        ])
        .env("KEYTAO_LUA_STARTUP_ROOT", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stale = runtime.create_session().unwrap();
    let other_dir = smoke_fixture::scratch_dir("lua-stale-session");
    smoke_fixture::write(&other_dir).unwrap();
    let other =
        ImeRuntime::with_dirs(&other_dir, other_dir.to_string_lossy().into_owned()).with_ime_host();
    other.init().unwrap();
    rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
    assert!(stale.state().schema_id.is_empty());
    assert_eq!(
        lock_ignore_poison(&PROCESS_RIME.initialized)
            .as_ref()
            .unwrap()
            .user,
        other_dir
    );
    rime_lua::RELOAD_PENDING.store(false, Ordering::Release);
    runtime.shutdown().unwrap();
    assert!(other.is_initialized());
    other.shutdown().unwrap();
    assert!(!other.is_initialized());
    drop(stale);
    std::fs::remove_dir_all(other_dir).unwrap();
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn lua_regeneration_errors_do_not_block_startup_or_reload() {
    let Some((runtime, dir)) =
        native_fixture("lua_regeneration_errors_do_not_block_startup_or_reload")
    else {
        return;
    };
    std::fs::write(dir.join("rime.lua"), "last_good = true\n").unwrap();
    std::fs::write(dir.join(ACTIVE_SCHEME_MANIFEST), "invalid").unwrap();
    // Exercise setup in a fresh process, independently from the reinitialize path.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime_access_tests::lua_regeneration_startup_child",
            "--nocapture",
        ])
        .env("KEYTAO_LUA_FAILURE_ROOT", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    reinitialize(
        dir.to_string_lossy().into_owned(),
        dir.to_string_lossy().into_owned(),
    )
    .unwrap();
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().name, "KeyTao Smoke");
    let schema_path = dir.join(format!("build/{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let mut schema: Value = serde_yaml::from_slice(&std::fs::read(&schema_path).unwrap()).unwrap();
    schema["schema"]["name"] = Value::String("Reloaded Smoke".into());
    std::fs::write(schema_path, serde_yaml::to_string(&schema).unwrap()).unwrap();
    ReloadStamp::write(&dir).unwrap();
    let generation = runtime.generation();
    runtime.reload_without_deploy().unwrap();
    assert!(
        runtime.generation() > generation,
        "deployment reload was skipped"
    );
    assert_eq!(session.current_schema().unwrap().name, "Reloaded Smoke");
    assert!(session.process_key_result(b'a' as u32, 0).is_some());
    assert_eq!(
        std::fs::read_to_string(dir.join("rime.lua")).unwrap(),
        "last_good = true\n"
    );
    drop(session);
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn lua_regeneration_startup_child() {
    let Ok(dir) = std::env::var("KEYTAO_LUA_FAILURE_ROOT") else {
        return;
    };
    let runtime = ImeRuntime::with_dirs(&dir, dir.clone()).with_ime_host();
    runtime.init_without_deploy().unwrap();
    let session = runtime.create_session().unwrap();
    assert!(session.process_key_result(b'a' as u32, 0).is_some());
}

#[test]
fn persisted_schema_selects_lua_before_startup() {
    let Ok(dir) = std::env::var("KEYTAO_LUA_STARTUP_ROOT") else {
        return;
    };
    let runtime = ImeRuntime::with_dirs(&dir, dir.clone()).with_ime_host();
    let snapshot = Path::new(&dir).join(".keytao-package-lua/b/lua/probe/init.lua");
    let bytes = std::fs::read(&snapshot).unwrap();
    std::fs::remove_file(&snapshot).unwrap();
    runtime.init_without_deploy().unwrap();
    let session = runtime.create_session().unwrap();
    assert_eq!(session.current_schema().unwrap().id, "keytao_lua_b");
    std::fs::write(snapshot, bytes).unwrap();
    *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) = None;
    runtime.prewarm(Some(&session)).unwrap();
    let texts: Vec<_> = session
        .process_key_result(b'a' as u32, 0)
        .unwrap()
        .state
        .candidates
        .into_iter()
        .map(|candidate| candidate.text)
        .collect();
    assert_eq!(texts, ["B-GLOBAL", "B-MODULE"]);
}

#[test]
fn plain_reload_prewarm_rebuilds_focused_then_other_sessions() {
    let Some((runtime, dir)) =
        native_fixture("plain_reload_prewarm_rebuilds_focused_then_other_sessions")
    else {
        return;
    };
    let sessions: Vec<_> = (0..3).map(|_| runtime.create_session().unwrap()).collect();
    sessions[0].set_ascii_mode(true).unwrap();
    let previous = runtime.generation();
    runtime.reload_without_deploy().unwrap();
    assert!(runtime.generation() > previous);
    assert!(
        sessions
            .iter()
            .all(|session| lock_ignore_poison(&session.inner).engine.is_none()),
        "plain reload eagerly rebuilt a session"
    );
    let (ready, started) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let worker_runtime = runtime.clone();
    let focused = sessions[2].clone();
    let worker = std::thread::spawn(move || {
        ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = Some((ready, resumed)));
        worker_runtime.prewarm(Some(&focused)).unwrap();
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        try_lock_ignore_poison(&sessions[2].inner).is_none(),
        "focused session was not rebuilt first"
    );
    assert!(lock_ignore_poison(&sessions[0].inner).engine.is_none());
    resume.send(()).unwrap();
    worker.join().unwrap();
    // A gate on the UI thread must never be reached after the worker warmed it.
    let (ready, unexpected) = mpsc::channel();
    let (_resume, resumed) = mpsc::channel();
    ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = Some((ready, resumed)));
    assert!(
        sessions[0]
            .process_key_result(b'a' as u32, 0)
            .unwrap()
            .state
            .ascii_mode
    );
    ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = None);
    assert!(unexpected.try_recv().is_err());
    for session in &sessions {
        let inner = lock_ignore_poison(&session.inner);
        assert!(inner.engine.is_some());
        assert_eq!(inner.generation, runtime.generation());
    }
    drop(sessions);
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn a_switch_reload_rebuilds_every_session_before_acknowledging() {
    let Some((runtime, dir)) =
        native_fixture("a_switch_reload_rebuilds_every_session_before_acknowledging")
    else {
        return;
    };
    let sessions: Vec<_> = (0..3).map(|_| runtime.create_session().unwrap()).collect();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    runtime.reload_without_deploy().unwrap();
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 1);
    for session in &sessions {
        let inner = lock_ignore_poison(&session.inner);
        assert_eq!(inner.generation, runtime.generation());
        assert!(inner
            .engine
            .as_ref()
            .expect("acknowledged before rebuilding")
            .is_ascii_mode());
    }
    drop(sessions);
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn f4_schema_calls_and_creation_wait_for_another_sessions_rebuild() {
    let Some((runtime, dir)) =
        native_fixture("f4_schema_calls_and_creation_wait_for_another_sessions_rebuild")
    else {
        return;
    };
    let first = runtime.create_session().unwrap();
    let second = runtime.create_session().unwrap();
    let path = dir.join("build/default.yaml");
    let mut config: Value = serde_yaml::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["switcher"] =
        serde_yaml::from_str("caption: Schemas\nhotkeys: [F4]\nsave_options: []\n").unwrap();
    std::fs::write(path, serde_yaml::to_string(&config).unwrap()).unwrap();
    for creating in [false, true] {
        for operation in 0..4 {
            runtime.reload_without_deploy().unwrap();
            let (ready, started) = mpsc::channel();
            let (resume, resumed) = mpsc::channel();
            let (finished, completion) = mpsc::channel();
            let worker_runtime = runtime.clone();
            let worker_session = first.clone();
            let worker = std::thread::spawn(move || {
                ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = Some((ready, resumed)));
                let result = if creating {
                    worker_runtime.create_session().map(Some)
                } else {
                    worker_runtime.prewarm(Some(&worker_session)).map(|_| None)
                };
                finished.send(result).unwrap();
            });
            started.recv_timeout(Duration::from_secs(5)).unwrap();
            let initialized_free = try_lock_ignore_poison(&PROCESS_RIME.initialized).is_some();
            let (sent, received) = mpsc::channel();
            let (calling, called) = mpsc::channel();
            let ui_runtime = runtime.clone();
            let ui_session = second.clone();
            let ui = std::thread::spawn(move || {
                calling.send(()).unwrap();
                let result = match operation {
                    0 => ui_session
                        .process_key_result(key_policy::XK_F4, 0)
                        .is_some_and(|result| {
                            result.accepted && result.state.schema_id == ".default"
                        }),
                    1 => ui_session.list_schemas().is_some_and(|schemas| {
                        schemas
                            .iter()
                            .any(|schema| schema.id == smoke_fixture::SCHEMA_ID)
                    }),
                    2 => ui_runtime
                        .create_session()
                        .is_ok_and(|session| lock_ignore_poison(&session.inner).engine.is_some()),
                    _ => ui_session.state().schema_id == smoke_fixture::SCHEMA_ID,
                };
                sent.send(result).unwrap();
            });
            called.recv_timeout(Duration::from_secs(1)).unwrap();
            let early = received.recv_timeout(Duration::from_millis(200));
            resume.send(()).unwrap();
            let _created = completion
                .recv_timeout(Duration::from_secs(5))
                .expect("engine build deadlocked with a waiting session")
                .unwrap();
            worker.join().unwrap();
            let waited = matches!(early, Err(mpsc::RecvTimeoutError::Timeout));
            let result = early.unwrap_or_else(|_| {
                received
                    .recv_timeout(Duration::from_secs(5))
                    .expect("session operation did not resume after rebuild")
            });
            ui.join().unwrap();
            assert!(initialized_free, "session build held the directory mutex");
            assert!(
                waited,
                "operation {operation} finished before rebuild (creating={creating})"
            );
            assert!(
                result,
                "operation {operation} failed after rebuild (creating={creating})"
            );
        }
    }
    drop((first, second));
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn key_on_another_session_waits_for_prewarm_and_gets_candidates() {
    let Some((runtime, dir)) =
        native_fixture("key_on_another_session_waits_for_prewarm_and_gets_candidates")
    else {
        return;
    };
    let focused = runtime.create_session().unwrap();
    let session = runtime.create_session().unwrap();
    runtime.reload_without_deploy().unwrap();
    let (ready, started) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let (finished, completion) = mpsc::channel();
    let worker_runtime = runtime.clone();
    let worker_focused = focused.clone();
    let worker = std::thread::spawn(move || {
        ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = Some((ready, resumed)));
        finished
            .send(worker_runtime.prewarm(Some(&worker_focused)))
            .unwrap();
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (sent, received) = mpsc::channel();
    let (calling, called) = mpsc::channel();
    let key_session = session.clone();
    let key = std::thread::spawn(move || {
        calling.send(()).unwrap();
        let first = key_session.process_key_result(b'a' as u32, 0);
        let second = key_session.process_key_result(b'a' as u32, 0);
        sent.send((first, second)).unwrap();
    });
    called.recv_timeout(Duration::from_secs(1)).unwrap();
    let early = received.recv_timeout(Duration::from_millis(200));
    resume.send(()).unwrap();
    completion
        .recv_timeout(Duration::from_secs(5))
        .expect("prewarm deadlocked with a waiting key")
        .unwrap();
    worker.join().unwrap();
    let waited = matches!(early, Err(mpsc::RecvTimeoutError::Timeout));
    let (first, second) = early.unwrap_or_else(|_| {
        received
            .recv_timeout(Duration::from_secs(5))
            .expect("key did not resume after prewarm")
    });
    key.join().unwrap();
    assert!(
        waited,
        "another session processed keys before prewarm finished"
    );
    let first = first.expect("first key was forwarded to the host");
    let second = second.expect("second key was forwarded to the host");
    assert!(first.accepted && second.accepted);
    assert!(first.state.committed.is_none() && second.state.committed.is_none());
    assert_eq!(first.state.preedit, "a");
    assert_eq!(second.state.preedit, smoke_fixture::CODE);
    assert_eq!(second.state.candidates[0].text, smoke_fixture::WORDS[0]);
    drop((focused, session));
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn failed_prewarm_key_rebuilds_engine_and_gets_candidates() {
    let Some((runtime, dir)) =
        native_fixture("failed_prewarm_key_rebuilds_engine_and_gets_candidates")
    else {
        return;
    };
    let session = runtime.create_session().unwrap();
    runtime.reload_without_deploy().unwrap();
    let compiled = dir.join(format!("build/{}.schema.yaml", smoke_fixture::SCHEMA_ID));
    let bytes = std::fs::read(&compiled).unwrap();
    std::fs::remove_file(&compiled).unwrap();
    assert!(runtime.prewarm(Some(&session)).is_err());
    assert!(lock_ignore_poison(&session.inner).engine.is_none());
    std::fs::write(&compiled, bytes).unwrap();
    let first = session.process_key_result(b'a' as u32, 0).unwrap();
    assert!(first.accepted);
    assert!(
        first.state.committed.is_none(),
        "failed prewarm committed raw input"
    );
    assert_eq!(first.state.preedit, "a");
    let second = session.process_key_result(b'a' as u32, 0).unwrap();
    assert!(second.accepted);
    assert!(second.state.committed.is_none());
    assert_eq!(second.state.preedit, smoke_fixture::CODE);
    assert_eq!(second.state.candidates[0].text, smoke_fixture::WORDS[0]);
    assert!(lock_ignore_poison(&session.inner).engine.is_some());
    drop(session);
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn key_during_prewarm_never_commits_raw_text() {
    let Some((runtime, dir)) = native_fixture("key_during_prewarm_never_commits_raw_text") else {
        return;
    };
    let session = runtime.create_session().unwrap();
    session.process_key_result(b'a' as u32, 0).unwrap();
    runtime.reload_without_deploy().unwrap();
    let (ready, started) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let (finished, completion) = mpsc::channel();
    let worker_runtime = runtime.clone();
    let focused = session.clone();
    let worker = std::thread::spawn(move || {
        ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = Some((ready, resumed)));
        finished
            .send(worker_runtime.prewarm(Some(&focused)))
            .unwrap();
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (sent, received) = mpsc::channel();
    let (calling, called) = mpsc::channel();
    let key_session = session.clone();
    let key = std::thread::spawn(move || {
        calling.send(()).unwrap();
        sent.send(key_session.process_key_result(b'a' as u32, 0))
            .unwrap();
    });
    called.recv_timeout(Duration::from_secs(1)).unwrap();
    let early = received.recv_timeout(Duration::from_millis(200));
    resume.send(()).unwrap();
    completion
        .recv_timeout(Duration::from_secs(5))
        .expect("prewarm deadlocked with a waiting key")
        .unwrap();
    worker.join().unwrap();
    let result = early
        .unwrap_or_else(|_| received.recv_timeout(Duration::from_secs(5)).unwrap())
        .expect("key was forwarded to the host");
    key.join().unwrap();
    assert!(result.accepted);
    assert!(
        result.state.committed.is_none(),
        "key during prewarm committed raw input: {:?}",
        result.state.committed
    );
    assert_eq!(result.state.preedit, "a");
    drop(session);
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn busy_policy_change_cannot_restore_sensitive_composition() {
    let Some((runtime, dir)) =
        native_fixture("busy_policy_change_cannot_restore_sensitive_composition")
    else {
        return;
    };
    let session = runtime.create_session().unwrap();
    session.process_key_result(b'a' as u32, 0).unwrap();
    let (ready, started) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let worker_session = session.clone();
    let worker = std::thread::spawn(move || {
        worker_session
            .with_engine(|engine| {
                ready.send(()).unwrap();
                resumed.recv_timeout(Duration::from_secs(5)).unwrap();
                engine.state()
            })
            .unwrap();
    });
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(session
        .set_input_policy(InputContextPolicy::sensitive())
        .is_none());
    assert!(session
        .set_input_policy(InputContextPolicy::default())
        .is_none());
    resume.send(()).unwrap();
    worker.join().unwrap();
    let result = session.process_key_result(b'b' as u32, 0).unwrap();
    assert!(result.state.committed.is_none());
    assert_eq!(result.state.preedit, "b");
    drop(session);
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn prewarm_reports_only_actual_rebuilds() {
    let Some((runtime, dir)) = native_fixture("prewarm_reports_only_actual_rebuilds") else {
        return;
    };
    let session = runtime.create_session().unwrap();
    assert!(!runtime.prewarm(Some(&session)).unwrap());
    runtime.reload_without_deploy().unwrap();
    assert!(runtime.prewarm(Some(&session)).unwrap());
    assert!(!runtime.prewarm(Some(&session)).unwrap());
    std::fs::write(
        dir.join(ACTIVE_SCHEME_MANIFEST),
        serde_json::json!({"root": dir.canonicalize().unwrap(), "packages": {"smoke": {
            "schemas": [smoke_fixture::SCHEMA_ID], "rime_lua": "prewarm_reloaded = true\n"
        }}})
        .to_string(),
    )
    .unwrap();
    schema_switch_request::request_schema(&dir, smoke_fixture::SCHEMA_ID).unwrap();
    rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
    assert!(runtime.prewarm(Some(&session)).unwrap());
    assert!(!runtime.prewarm(Some(&session)).unwrap());
    drop(session);
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

#[test]
fn deployment_keeps_new_and_rebuilt_sessions_composing() {
    let Some((runtime, dir)) =
        native_fixture("deployment_keeps_new_and_rebuilt_sessions_composing")
    else {
        return;
    };
    let dropped = runtime.create_session().unwrap();
    runtime.reload_without_deploy().unwrap();
    assert!(lock_ignore_poison(&dropped.inner).engine.is_none());
    let session = runtime.create_session().unwrap();
    let generation = runtime.generation();
    begin_deployment(&dir).unwrap();
    assert!(deployment_in_progress(&dir));
    let created = runtime
        .create_session()
        .expect("deployment rejected a new session");
    assert!(runtime.reload_without_deploy().is_err());
    assert!(runtime.prewarm(Some(&session)).is_err());
    assert_eq!(runtime.generation(), generation);
    for session in [&session, &created, &dropped] {
        let result = session
            .process_key_result(b'a' as u32, 0)
            .expect("deployment forwarded the key to the host");
        assert!(result.accepted);
        assert!(
            result.state.committed.is_none(),
            "deployment committed raw input"
        );
        assert!(!result.state.preedit.is_empty());
    }
    let mut deployment = DeploymentState::default();
    deployment.key_pressed(&dir);
    assert!(!deployment.blocks_input(&dir));
    assert_eq!(deployment.hint(&dir), None);
    {
        let _build = PROCESS_RIME.build.acquire().unwrap();
        assert!(session.process_key_result(b'a' as u32, 0).is_some());
    }
    mark_deployment_failed(&dir).unwrap();
    deployment.key_pressed(&dir);
    assert_eq!(deployment.hint(&dir), None);
    assert!(!deployment.blocks_input(&dir));
    std::fs::remove_file(dir.join("build").join(DEPLOYING_FILE)).unwrap();
    drop((session, created, dropped));
    runtime.shutdown().unwrap();
    runtime_log::flush_blocking(Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(dir); // best effort: librime may still flush files
}

static SERIAL: Mutex<()> = Mutex::new(());

fn empty_runtime_session() -> (ImeRuntime, ImeRuntimeSession) {
    let runtime = ImeRuntime::with_dirs(
        std::env::temp_dir().join("keytao-no-engine-contention-test"),
        "unused-shared-directory",
    );
    let session = ImeRuntimeSession {
        dirs: runtime.data_dirs().unwrap(),
        inner: Arc::new(Mutex::new(ImeRuntimeSessionInner {
            user_data_dir: runtime.data_dirs().unwrap().user,
            ime_host: false,
            engine: None,
            usable: None,
            generation: PROCESS_RIME.generation.load(Ordering::SeqCst),
            carried_ascii_mode: None,
            carried_english_mode: None,
            english_mode: english_mode::EnglishModeController::default(),
            applied_input_version: 0,
            last_input_state: None,
        })),
        policy: Arc::new(SessionInputPolicy::new(InputContextPolicy::default())),
        context_ascii_mode: Arc::new(AtomicU8::new(0)),
        pending: Arc::new(Mutex::new(PendingRuntimeInput::default())),
    };
    (runtime, session)
}

#[test]
fn ui_session_operations_do_not_wait_for_a_deployment_write_lock() {
    let _serial = lock_ignore_poison(&SERIAL);
    let (runtime, session) = empty_runtime_session();
    // Pretend setup completed without touching librime. The held write lock
    // must stop every tested entry point before an Engine could be built.
    let previous_initialized = {
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        std::mem::replace(&mut *initialized, Some(runtime.data_dirs().unwrap()))
    };
    let barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    let (sent, received) = mpsc::channel();
    let ui_session = session.clone();
    let ui = std::thread::spawn(move || {
        let creation_busy = runtime.create_session().is_err();
        let key_forwarded = ui_session.process_key_result(b'a' as u32, 0).is_none();
        let enter_forwarded = ui_session.process_enter().is_none();
        let empty = ui_session.state();
        let candidates_hidden = empty.preedit.is_empty() && empty.candidates.is_empty();
        let paging_disabled = !ui_session.supports_native_paging();
        let selection_disabled = !ui_session.supports_candidate_selection();
        let policy_deferred = ui_session
            .set_input_policy(InputContextPolicy::sensitive())
            .is_none();
        let sensitive = ui_session.input_policy() == InputContextPolicy::sensitive();
        sent.send([
            creation_busy,
            key_forwarded,
            enter_forwarded,
            candidates_hidden,
            paging_disabled,
            selection_disabled,
            policy_deferred,
            sensitive,
        ])
        .unwrap();
    });
    // On regression, release the writer before joining so the failure is
    // bounded instead of hanging the test runner forever.
    let result = received.recv_timeout(Duration::from_secs(1));
    drop(barrier);
    *lock_ignore_poison(&PROCESS_RIME.initialized) = previous_initialized;
    ui.join().unwrap();
    assert!(result
        .expect("UI waited for deployment")
        .into_iter()
        .all(|ok| ok));
    assert_eq!(session.input_policy(), InputContextPolicy::sensitive());
    // Even after deployment finishes a password key must never reach librime.
    let result = session.process_key_result(b'a' as u32, 0).unwrap();
    assert!(!result.accepted);
    assert!(result.state.preedit.is_empty());
    assert!(result.state.candidates.is_empty());
    assert!(!session.process_enter().unwrap().accepted);
    assert_eq!(
        session.policy.take_for_engine(),
        (InputContextPolicy::sensitive(), true)
    );
}

#[test]
fn initialization_and_session_mutex_contention_do_not_block_ui_or_lose_policy() {
    let _serial = lock_ignore_poison(&SERIAL);
    let (runtime, session) = empty_runtime_session();
    let initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let inner = lock_ignore_poison(&session.inner);
    let (sent, received) = mpsc::channel();
    let ui_session = session.clone();
    let ui = std::thread::spawn(move || {
        let creation_busy = runtime.create_session().is_err();
        let key_forwarded = ui_session.process_key_result(b'a' as u32, 0).is_none();
        let policy_deferred = ui_session
            .set_input_policy(InputContextPolicy::sensitive())
            .is_none();
        let sensitive = ui_session.input_policy() == InputContextPolicy::sensitive();
        sent.send([creation_busy, key_forwarded, policy_deferred, sensitive])
            .unwrap();
    });
    let result = received.recv_timeout(Duration::from_secs(1));
    drop(inner);
    drop(initialized);
    ui.join().unwrap();
    assert!(result
        .expect("UI waited for runtime mutexes")
        .into_iter()
        .all(|ok| ok));
    assert_eq!(session.input_policy(), InputContextPolicy::sensitive());
}

#[test]
fn a_sensitive_transition_while_busy_still_discards_the_previous_composition() {
    let policy = SessionInputPolicy::new(InputContextPolicy::default());
    policy.set(InputContextPolicy::sensitive());
    // Focus can move again before deployment releases the session. Returning
    // to a normal field must not revive the previous field's composition.
    policy.set(InputContextPolicy::private());
    assert_eq!(policy.current(), InputContextPolicy::private());
    assert_eq!(
        policy.take_for_engine(),
        (InputContextPolicy::private(), true)
    );
    assert_eq!(
        policy.take_for_engine(),
        (InputContextPolicy::private(), false)
    );
}
