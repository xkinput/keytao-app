use crate::{AppEnv, Core, CoreEvent, EventSink, Platform, install::smart_install};
use std::{
    fs,
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct ProgressSink(Mutex<Vec<(String, u32, String)>>);

impl EventSink for ProgressSink {
    fn emit(&self, event: CoreEvent) {
        if let CoreEvent::InstallProgress(progress) = event {
            self.0
                .lock()
                .unwrap()
                .push((progress.stage, progress.percent, progress.message));
        }
    }
}

#[tokio::test]
async fn archive_install_preserves_user_merges_invalidates_build_and_emits_progress() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("rime");
    fs::create_dir_all(dest.join("lua")).unwrap();
    fs::create_dir_all(dest.join("build")).unwrap();
    fs::write(
        dest.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: personal\n",
    )
    .unwrap();
    fs::write(dest.join("rime.lua"), "custom = require(\"custom\")\n").unwrap();
    fs::write(dest.join("lua/custom.lua"), "user code").unwrap();
    fs::write(dest.join("build/keydo.schema.yaml"), "stale schema").unwrap();
    fs::write(dest.join("build/keydo.table.bin"), "stale dictionary").unwrap();
    fs::write(
        dest.join("build/personal.table.bin"),
        "unrelated dictionary",
    )
    .unwrap();
    let zip_path = dir.path().join("package.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
    let entries = [
        (
            "default.custom.yaml",
            "patch:\n  schema_list:\n    - schema: keydo\n",
        ),
        ("rime.lua", "package = require(\"package\")\n"),
        ("lua/custom.lua", "package code"),
        ("lua/package.lua", "package module"),
        ("keydo.schema.yaml", "schema: {schema_id: keydo}\n"),
        ("keydo.dict.yaml", "name: keydo\n"),
    ];
    for (name, content) in entries {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let sink = Arc::new(ProgressSink::default());
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("state"),
            cache_dir: dir.path().join("cache"),
            resource_dir: dir.path().join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        sink.clone(),
    )
    .unwrap();
    let result = smart_install(
        &core,
        zip_path.to_string_lossy().into_owned(),
        dest.to_string_lossy().into_owned(),
        #[cfg(target_os = "windows")]
        |root, schemas, dictionaries, _| {
            keytao_core::invalidate_rime_build_artifacts(root, schemas, dictionaries).map(|paths| {
                paths
                    .into_iter()
                    .map(|p| format!("[INVALIDATED] {p}"))
                    .collect()
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(result.merged_schemas, vec!["personal"]);
    assert_eq!(
        crate::scheme_files::parse_schema_list(
            &fs::read_to_string(dest.join("default.custom.yaml")).unwrap()
        ),
        vec!["personal", "keydo"]
    );
    assert_eq!(
        fs::read_to_string(dest.join("lua/custom_user.lua")).unwrap(),
        "user code"
    );
    assert_eq!(
        fs::read_to_string(dest.join("lua/custom.lua")).unwrap(),
        "package code"
    );
    assert!(
        fs::read_to_string(dest.join("rime.lua"))
            .unwrap()
            .contains("require(\"custom_user\")")
    );
    assert!(!dest.join("build/keydo.schema.yaml").exists());
    assert!(!dest.join("build/keydo.table.bin").exists());
    assert_eq!(
        fs::read_to_string(dest.join("build/personal.table.bin")).unwrap(),
        "unrelated dictionary"
    );
    assert!(result.verify.iter().all(|entry| entry.ok));
    assert!(result.logs.iter().any(|line| line.starts_with("[RENAMED]")));
    assert!(!zip_path.exists());
    let progress = sink.0.lock().unwrap();
    assert_eq!(
        progress.first().unwrap(),
        &("extracting".into(), 61, "正在解压...".into())
    );
    assert_eq!(
        progress.last().unwrap(),
        &("done".into(), 100, "安装完成！".into())
    );
    assert_eq!(progress.len(), entries.len() + 2);
    for (index, (_, percent, _)) in progress[1..progress.len() - 1].iter().enumerate() {
        assert_eq!(*percent, 61 + ((index + 1) * 39 / entries.len()) as u32);
    }
}
