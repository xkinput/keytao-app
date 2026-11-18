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
async fn selected_local_schemas_survive_reimport_and_deploy_with_librime() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let source = fixture.dir.path().join("source");
    let selected_path = source.join("keytao-dz.schema.yaml");
    let template = fs::read_to_string(&selected_path).unwrap();
    fs::write(source.join("support.schema.yaml"), template.replace("schema_id: keytao-dz", "schema_id: support")).unwrap();
    fs::write(&selected_path, template.replace("schema_id: keytao-dz", "schema_id: keytao-dz\n  dependencies: [support]")).unwrap();
    let key = packages::import_local_scheme(&fixture.core, &source, "Local", None).unwrap();
    let schemas = keytao_app_core::package_schemas::package_schemas(&fixture.core, &key).unwrap();
    assert_eq!(schemas.iter().filter(|schema| schema.selected).count(), 3);
    assert!(keytao_app_core::package_schemas::set_package_schemas(&fixture.core, &key, &[]).is_err());
    assert!(keytao_app_core::package_schemas::set_package_schemas(&fixture.core, &key, &["outside".into()]).is_err());
    packages::activate_scheme(&fixture.core, &key, &fixture.root,
        #[cfg(target_os = "windows")] |_, _, _, _| Ok(Vec::new()),
    ).await.unwrap();
    fs::write(fixture.root.join("user.yaml"), "var: {previously_selected_schema: keytao-bj}").unwrap();
    keytao_app_core::package_schemas::set_package_schemas(&fixture.core, &key, &["keytao-dz".into()]).unwrap();
    packages::activate_scheme(&fixture.core, &key, &fixture.root,
        #[cfg(target_os = "windows")] |_, _, _, _| Ok(Vec::new()),
    ).await.unwrap();
    assert_eq!(keytao_core::parse_schema_list(&fs::read_to_string(fixture.root.join("default.custom.yaml")).unwrap()), ["keytao-dz"]);
    assert_eq!(keytao_app_core::package_schemas::package_schemas(&fixture.core, &key).unwrap().len(), schemas.len());
    packages::import_local_scheme(&fixture.core, &source, "Local", Some(&key)).unwrap();
    let selected = keytao_app_core::package_schemas::package_schemas(&fixture.core, &key).unwrap();
    assert_eq!(selected.iter().filter(|schema| schema.selected).map(|schema| schema.id.as_str()).collect::<Vec<_>>(), ["keytao-dz"]);
    let runtime = fixture.runtime();
    runtime.init().unwrap();
    fixture.assert_selected("keytao-dz");
    assert_eq!(keytao_core::parse_schema_list(&fs::read_to_string(fixture.root.join("build/default.yaml")).unwrap()), ["keytao-dz"]);
    assert!(fixture.root.join("build/keytao-dz.schema.yaml").is_file());
    assert!(fixture.root.join("build/support.schema.yaml").is_file());
    eprintln!("librime selection deploy: schema_list=[keytao-dz], active=keytao-dz, dependency=support compiled");
}

#[tokio::test]
async fn saved_cloud_selection_survives_direct_update_and_reopen() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    fixture.install("v1").await;
    keytao_app_core::package_schemas::set_package_schemas(&fixture.core, "keytao", &["keytao-dz".into()]).unwrap();
    fs::write(fixture.root.join("user.yaml"), "var: {previously_selected_schema: keytao-bj}").unwrap();
    fixture.install("v2").await;
    assert_eq!(keytao_core::parse_schema_list(&fs::read_to_string(fixture.root.join("default.custom.yaml")).unwrap()), ["keytao-dz"]);
    let reopened = Core::new(fixture.core.env().clone(), Arc::new(NullSink)).unwrap();
    let schemas = keytao_app_core::package_schemas::package_schemas(&reopened, "keytao").unwrap();
    assert!(schemas.len() > 1);
    assert_eq!(schemas.iter().filter(|schema| schema.selected).map(|schema| schema.id.as_str()).collect::<Vec<_>>(), ["keytao-dz"]);
    let user: serde_yaml::Value = serde_yaml::from_slice(&fs::read(fixture.root.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(user["var"]["previously_selected_schema"].as_str(), Some("keytao-dz"));
    keytao_app_core::package_schemas::set_package_schemas(&fixture.core, "keytao", &["keytao".into(), "keytao-dz".into()]).unwrap();
    packages::activate_scheme(&fixture.core, "keytao", &fixture.root,
        #[cfg(target_os = "windows")] |_, _, _, _| Ok(Vec::new()),
    ).await.unwrap();
    let user: serde_yaml::Value = serde_yaml::from_slice(&fs::read(fixture.root.join("user.yaml")).unwrap()).unwrap();
    assert_eq!(user["var"]["previously_selected_schema"].as_str(), Some("keytao-dz"));
}

#[tokio::test]
async fn first_install_selects_primary_and_reinstall_preserves_manual_selection() {
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
    fixture.assert_selected("keytao-bj");

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
    fixture.assert_selected("keytao-bj");
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
async fn local_reimport_update_and_unchanged_reimport_preserve_manual_selection() {
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
    fixture.assert_selected("keytao-dz");
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
