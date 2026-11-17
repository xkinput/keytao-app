#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_app_core::{AppEnv, Core, CoreEvent, EventSink, Platform, install, packages, scheme};
use keytao_core::{ImeRuntime, deployed_schema_id_from_dir};
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
        install::smart_install(
            &self.core,
            archive.to_string_lossy().into_owned(),
            self.root.to_string_lossy().into_owned(),
            #[cfg(target_os = "windows")]
            |_, _, _, _| Ok(Vec::new()),
        )
        .await
        .unwrap();
        scheme::record_installed_scheme(&self.core, "keytao", Some(version)).unwrap();
    }

    fn runtime(&self) -> ImeRuntime {
        ImeRuntime::with_dirs(&self.root, self.root.to_string_lossy().into_owned())
    }

    fn assert_selected(&self, schema: &str) {
        assert_eq!(
            deployed_schema_id_from_dir(&self.root).as_deref(),
            Some(schema)
        );
    }
}

#[tokio::test]
async fn installing_package_replaces_stale_selection_with_its_primary_schema() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("user.yaml"),
        "custom:\n  preserved: keep\nvar:\n  previously_selected_schema: keytao-bj\n  preserved_option: true\n",
    )
    .unwrap();

    fixture.install("v1").await;
    let runtime = fixture.runtime();
    runtime.init().unwrap();
    fixture.assert_selected("keytao");
    let user: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(fixture.root.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(user["custom"]["preserved"].as_str(), Some("keep"));
    assert_eq!(user["var"]["preserved_option"].as_bool(), Some(true));

    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-bj").unwrap();
    drop(session);
    fixture.install("v1").await;
    runtime.reload().unwrap();
    fixture.assert_selected("keytao");

    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-bj").unwrap();
    drop(session);
    fixture
        .core
        .update_state(|state| state.active_scheme = Some("xmjd".into()))
        .unwrap();
    packages::activate_scheme(
        &fixture.core,
        "keytao",
        &fixture.root,
        #[cfg(target_os = "windows")]
        |_, _, _, _| Ok(Vec::new()),
    )
    .await
    .unwrap();
    runtime.reload().unwrap();
    fixture.assert_selected("keytao");
}

#[tokio::test]
async fn updating_same_package_keeps_the_manual_schema_selection() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    fixture.install("v1").await;
    let runtime = fixture.runtime();
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-dz").unwrap();
    drop(session);
    fixture.assert_selected("keytao-dz");

    fixture.install("v2").await;
    runtime.reload().unwrap();
    fixture.assert_selected("keytao-dz");
    let user = fs::read(fixture.root.join("user.yaml")).unwrap();
    scheme::record_installed_scheme(&fixture.core, "keytao", Some("v2")).unwrap();
    assert_eq!(fs::read(fixture.root.join("user.yaml")).unwrap(), user);
    fixture.assert_selected("keytao-dz");
}

#[tokio::test]
async fn local_reimport_update_keeps_selection_and_unchanged_reimport_selects_primary() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let source = fixture.dir.path().join("source");
    let key = packages::import_local_scheme(&fixture.core, &source, "Local", None).unwrap();
    packages::activate_scheme(
        &fixture.core,
        &key,
        &fixture.root,
        #[cfg(target_os = "windows")]
        |_, _, _, _| Ok(Vec::new()),
    )
    .await
    .unwrap();
    let runtime = fixture.runtime();
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-dz").unwrap();
    drop(session);
    let old_version = fixture.core.state().installed_packages[&key]
        .version
        .clone();

    let schema = source.join("keytao.schema.yaml");
    fs::OpenOptions::new()
        .append(true)
        .open(schema)
        .unwrap()
        .write_all(b"# Updated\n")
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            packages::import_local_scheme(&fixture.core, &source, "Local", Some(&key)).unwrap(),
            key
        );
    }
    assert_ne!(
        fixture.core.state().installed_packages[&key].version,
        old_version
    );
    let marker = fixture.root.join(keytao_core::ACTIVE_SCHEME_MANIFEST);
    let saved_marker = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    fs::create_dir(&marker).unwrap();
    assert!(
        packages::activate_scheme(
            &fixture.core,
            &key,
            &fixture.root,
            #[cfg(target_os = "windows")]
            |_, _, _, _| Ok(Vec::new()),
        )
        .await
        .is_err()
    );
    fs::remove_dir(&marker).unwrap();
    fs::write(&marker, saved_marker).unwrap();
    let core = Core::new(fixture.core.env().clone(), Arc::new(NullSink)).unwrap();
    packages::activate_scheme(
        &core,
        &key,
        &fixture.root,
        #[cfg(target_os = "windows")]
        |_, _, _, _| Ok(Vec::new()),
    )
    .await
    .unwrap();
    runtime.reload().unwrap();
    fixture.assert_selected("keytao-dz");

    packages::import_local_scheme(&core, &source, "Local", Some(&key)).unwrap();
    packages::activate_scheme(
        &core,
        &key,
        &fixture.root,
        #[cfg(target_os = "windows")]
        |_, _, _, _| Ok(Vec::new()),
    )
    .await
    .unwrap();
    runtime.reload().unwrap();
    let primary = keytao_core::parse_schema_list(
        &fs::read_to_string(fixture.root.join("default.custom.yaml")).unwrap(),
    )
    .remove(0);
    assert_ne!(primary, "keytao-dz");
    fixture.assert_selected(&primary);
}

#[tokio::test]
async fn plain_redeploy_keeps_the_manual_schema_selection() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    fixture.install("v1").await;
    let runtime = fixture.runtime();
    runtime.init().unwrap();
    let session = runtime.create_session().unwrap();
    session.select_schema("keytao-dz").unwrap();
    drop(session);

    runtime.reload().unwrap();
    fixture.assert_selected("keytao-dz");
}
