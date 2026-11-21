#![cfg(target_os = "macos")]

#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_app_core::{AppEnv, Core, CoreEvent, EventSink, Platform, install, packages};
use keytao_core::{ImeRuntime, RuntimeDirs, deployed_schema_id_from_dir};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

static LIBRIME: Mutex<()> = Mutex::new(());

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _: CoreEvent) {}
}

struct Fixture {
    dir: tempfile::TempDir,
    core: Arc<Core>,
    root: PathBuf,
    package: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        smoke_fixture::write(&source).unwrap();
        let template =
            fs::read_to_string(source.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID)))
                .unwrap();
        for id in ["keytao", "keytao-dz", "keytao-bj"] {
            fs::write(
                source.join(format!("{id}.schema.yaml")),
                template.replace(
                    &format!("schema_id: {}", smoke_fixture::SCHEMA_ID),
                    &format!("schema_id: {id}"),
                ),
            )
            .unwrap();
        }
        fs::write(
            source.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao\n    - schema: keytao-dz\n    - schema: keytao-bj\n",
        )
        .unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for entry in fs::read_dir(&source).unwrap() {
            let entry = entry.unwrap();
            zip.start_file(
                entry.file_name().to_string_lossy(),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&fs::read(entry.path()).unwrap()).unwrap();
        }
        let package = zip.finish().unwrap().into_inner();
        let root = dir.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        smoke_fixture::write(&dir.path().join("resources/rime-data")).unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: dir.path().join("state"),
                cache_dir: dir.path().join("cache"),
                resource_dir: dir.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(NullSink),
        )
        .unwrap();
        Self {
            dir,
            core,
            root,
            package,
        }
    }

    async fn install(&self, version: &str) {
        let archive = self.dir.path().join("package.zip");
        fs::write(&archive, &self.package).unwrap();
        install::install_live_scheme(
            &self.core,
            archive.to_string_lossy().into_owned(),
            &self.root,
            "keytao",
            Some(version),
        )
        .await
        .unwrap();
    }

    async fn activate(&self, key: &str) {
        keytao_app_core::deploy::switch_package(&self.core, &self.root, key)
            .await
            .unwrap();
        assert_eq!(
            RuntimeDirs::active_package_key(&self.root).as_deref(),
            Some(key)
        );
    }

    fn runtime(&self) -> ImeRuntime {
        ImeRuntime::with_dirs(
            &self.root,
            self.core
                .env()
                .resource_dir
                .join("rime-data")
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn assert_selected(&self, schema: &str) {
        assert_eq!(
            deployed_schema_id_from_dir(&self.root).as_deref(),
            Some(schema)
        );
    }

    fn runtime_dir(&self, key: &str) -> PathBuf {
        self.root.join("runtime").join(key)
    }
}

#[tokio::test]
async fn selected_local_schemas_survive_reimport_and_deploy_with_librime() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let source = fixture.dir.path().join("source");
    let selected_path = source.join("keytao-dz.schema.yaml");
    let template = fs::read_to_string(&selected_path).unwrap();
    fs::write(
        source.join("support.schema.yaml"),
        template.replace("schema_id: keytao-dz", "schema_id: support"),
    )
    .unwrap();
    fs::write(
        &selected_path,
        template.replace(
            "schema_id: keytao-dz",
            "schema_id: keytao-dz\n  dependencies: [support]",
        ),
    )
    .unwrap();
    let key = packages::import_local_scheme(&fixture.core, &source, "Local", None).unwrap();
    let schemas = keytao_app_core::package_schemas::package_schemas(&fixture.core, &key).unwrap();
    assert_eq!(schemas.iter().filter(|schema| schema.selected).count(), 3);
    assert!(
        keytao_app_core::package_schemas::set_package_schemas(&fixture.core, &key, &[]).is_err()
    );
    assert!(
        keytao_app_core::package_schemas::set_package_schemas(
            &fixture.core,
            &key,
            &["outside".into()]
        )
        .is_err()
    );
    keytao_app_core::package_schemas::set_package_schemas(
        &fixture.core,
        &key,
        &["keytao-dz".into()],
    )
    .unwrap();
    packages::import_local_scheme(&fixture.core, &source, "Local", Some(&key)).unwrap();
    fixture.activate(&key).await;

    let dirs = fixture.runtime_dir(&key);
    assert_eq!(
        keytao_core::parse_schema_list(
            &fs::read_to_string(dirs.join("default.custom.yaml")).unwrap()
        ),
        ["keytao-dz"]
    );
    assert_eq!(
        keytao_app_core::package_schemas::package_schemas(&fixture.core, &key)
            .unwrap()
            .len(),
        schemas.len()
    );
    assert!(!fixture.root.join("default.custom.yaml").exists());
    let runtime = fixture.runtime();
    runtime.init_without_deploy().unwrap();
    fixture.assert_selected("keytao-dz");
    assert_eq!(
        keytao_core::parse_schema_list(
            &fs::read_to_string(dirs.join("build/default.yaml")).unwrap()
        ),
        ["keytao-dz"]
    );
    assert!(dirs.join("build/keytao-dz.schema.yaml").is_file());
    assert!(dirs.join("build/support.schema.yaml").is_file());
}

#[tokio::test]
async fn cloud_update_keeps_saved_and_manual_schema_selection() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    fixture.install("v1").await;
    fixture.activate("keytao").await;
    let runtime = fixture.runtime();
    runtime.init_without_deploy().unwrap();
    fixture.assert_selected("keytao");

    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-dz").unwrap();
    drop(session);
    fixture.assert_selected("keytao-dz");
    // The deploy worker uses the same librime process as this fixture.
    drop(runtime);

    keytao_app_core::package_schemas::set_package_schemas(
        &fixture.core,
        "keytao",
        &["keytao".into(), "keytao-dz".into()],
    )
    .unwrap();
    fixture.install("v2").await;
    fixture.activate("keytao").await;
    let runtime = fixture.runtime();
    runtime.init_without_deploy().unwrap();
    fixture.assert_selected("keytao-dz");
    let reopened = Core::new(fixture.core.env().clone(), Arc::new(NullSink)).unwrap();
    let schemas = keytao_app_core::package_schemas::package_schemas(&reopened, "keytao").unwrap();
    assert_eq!(
        schemas
            .iter()
            .filter(|schema| schema.selected)
            .map(|schema| schema.id.as_str())
            .collect::<Vec<_>>(),
        ["keytao", "keytao-dz"]
    );
}
