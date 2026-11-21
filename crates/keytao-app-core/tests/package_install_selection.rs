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

#[tokio::test]
async fn lua_user_data_dir_auxiliary_files_and_preset_default_compile_and_render() {
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let source = fixture.dir.path().join("lua-aux-source");
    smoke_fixture::write(&source).unwrap();
    // Write a preset default.yaml with punctuator/full_shape into bundled rime-data.
    fs::write(
        fixture.core.env().resource_dir.join("rime-data/default.yaml"),
        "config_version: '1.0'\nschema_list: []\nmenu:\n  page_size: 6\npunctuator:\n  full_shape:\n    ',': '，'\n    '.': '。'\n  half_shape:\n    ',': '，'\n    '.': '。'\n",
    )
    .unwrap();
    fs::create_dir_all(source.join("lua/aux_demo")).unwrap();
    fs::write(
        source.join("lua/aux_demo/ss.lua"),
        r#"local M = {}
function M.init(env)
  env.words = {}
  local f = io.open(rime_api.get_user_data_dir() .. "/aux_demo_ss.txt", "r")
  if f then
    for line in f:lines() do
      local code, word = line:match("([^\t]+)\t([^\t]+)")
      if code and word then
        env.words[code] = word
      end
    end
    f:close()
  end
end
function M.func(translation, env)
  local input = env.engine.context.input
  local word = env.words[input]
  local first = true
  for cand in translation:iter() do
    if word and first then
      cand:get_genuine().comment = "aoo " .. word .. "'"
      first = false
    end
    yield(cand)
  end
end
return M
"#,
    )
    .unwrap();
    fs::write(source.join("aux_demo_ss.txt"), "mwt\t没问题\n").unwrap();
    fs::write(source.join("aux_demo.fixed.txt"), "mwt\t没问题\n").unwrap();
    fs::write(
        source.join("aux_demo.dict.yaml"),
        "---\nname: aux_demo\nversion: '1'\nsort: by_weight\n...\n没问题\tmwt\t100\n莫扎特\tmwt\t80\n",
    )
    .unwrap();
    fs::write(
        source.join("aux_demo.schema.yaml"),
        r#"schema:
  schema_id: aux_demo
  name: AuxDemo
  version: '1'
engine:
  processors:
    - ascii_composer
    - speller
    - punctuator
    - selector
    - navigator
    - express_editor
  segmentors:
    - ascii_segmentor
    - abc_segmentor
    - punct_segmentor
    - fallback_segmentor
  translators:
    - punct_translator
    - table_translator
  filters:
    - lua_filter@*aux_demo.ss
    - uniquifier
speller:
  alphabet: abcdefghijklmnopqrstuvwxyz
translator:
  dictionary: aux_demo
punctuator:
  full_shape:
    __include: default:/punctuator/full_shape
  half_shape:
    __include: default:/punctuator/half_shape
"#,
    )
    .unwrap();
    // Simulate a stub default.yaml (only schema_list) to verify preset default.yaml still materializes.
    fs::write(
        source.join("default.yaml"),
        "schema_list:\n  - schema: aux_demo\n",
    )
    .unwrap();
    fs::remove_file(source.join(format!("{}.schema.yaml", smoke_fixture::SCHEMA_ID))).unwrap();

    let key = packages::import_local_scheme(&fixture.core, &source, "AuxDemo", None).unwrap();
    fixture.activate(&key).await;

    let rt_dir = fixture.runtime_dir(&key);
    assert!(rt_dir.join("aux_demo_ss.txt").is_file());
    assert!(rt_dir.join("aux_demo.fixed.txt").is_file());
    assert!(
        fs::read_to_string(rt_dir.join("default.yaml"))
            .unwrap()
            .contains("punctuator:")
    );

    let runtime = fixture.runtime();
    runtime.init_without_deploy().unwrap();
    let session = runtime.create_session().unwrap();
    for ch in ['m', 'w', 't'] {
        session.process_key_result(ch as u32, 0).unwrap();
    }
    let state = session.state();
    assert_eq!(state.candidates[0].text, "没问题");
    assert_eq!(state.candidates[0].comment.as_deref(), Some("aoo 没问题'"));
}

#[tokio::test]
async fn snow_pinyin_zip_renders_shape_and_ss_comment_when_present() {
    let zip_path = std::path::Path::new("/Users/rea/Downloads/snow-pinyin (1).zip");
    if !zip_path.is_file() {
        return;
    }
    let _serialized = LIBRIME.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    // Copy full preset files from vendored/bundled rime-data if available, or write complete preset default.yaml.
    fs::write(
        fixture.core.env().resource_dir.join("rime-data/default.yaml"),
        "config_version: '1.0'\nschema_list: []\nmenu:\n  page_size: 6\nswitcher:\n  hotkeys: [F4]\npunctuator:\n  full_shape:\n    ',': '，'\n    '.': '。'\n  half_shape:\n    ',': '，'\n    '.': '。'\nkey_binder:\n  bindings: []\nrecognizer:\n  patterns: {}\n",
    )
    .unwrap();
    let unpacked = fixture.dir.path().join("snow-pinyin-unpacked");
    let mut archive = zip::ZipArchive::new(fs::File::open(zip_path).unwrap()).unwrap();
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx).unwrap();
        if entry.is_dir() {
            continue;
        }
        let target = unpacked.join(entry.name());
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut out = fs::File::create(&target).unwrap();
        std::io::copy(&mut entry, &mut out).unwrap();
    }

    let key = packages::import_local_scheme(&fixture.core, &unpacked, "冰雪拼音", None).unwrap();
    let schemas = keytao_app_core::package_schemas::package_schemas(&fixture.core, &key).unwrap();
    assert!(schemas.len() > 3, "expected > 3 schemas, got {}", schemas.len());

    keytao_app_core::package_schemas::set_package_schemas(
        &fixture.core,
        &key,
        &["snow_flyjd".into(), "snow_jiandao".into()],
    )
    .unwrap();
    fixture.activate(&key).await;

    let rt_dir = fixture.runtime_dir(&key);
    assert!(rt_dir.join("snow_flyjd.schema.yaml").is_file());
    assert!(rt_dir.join("snow_jiandao.schema.yaml").is_file());
    assert!(rt_dir.join("snow_bihua.dict.yaml").is_file());
    assert!(rt_dir.join("frost_dicts/cn_dicts/ext.dict.yaml").is_file());
    assert!(rt_dir.join("lua").is_dir());
    assert!(rt_dir.join("default.yaml").is_file());
    assert!(rt_dir.join("default.custom.yaml").is_file());
    assert!(rt_dir.join("snow_flyjd_ss.txt").is_file());
    assert!(rt_dir.join("snow_jiandao_ss.txt").is_file());
    assert!(rt_dir.join("snow_flyjd.fixed.txt").is_file());
    assert!(rt_dir.join("snow_jiandao.fixed.txt").is_file());
    assert!(
        fixture
            .core
            .env()
            .data_dir
            .join(format!("scheme-packages/local/{key}/files/build/kMandarin17.reverse.bin"))
            .is_file()
    );

    let runtime = fixture.runtime();
    runtime.init_without_deploy().unwrap();
    for schema_id in ["snow_flyjd", "snow_jiandao"] {
        let session = runtime.create_session().unwrap();
        session.select_schema(schema_id).unwrap();
        for ch in ['m', 'w', 't'] {
            session.process_key_result(ch as u32, 0).unwrap();
        }
        let state = session.state();
        assert!(
            !state.candidates.is_empty(),
            "expected candidates for mwt in {schema_id}"
        );
        assert_eq!(state.candidates[0].text, "没问题", "schema={schema_id}");
        assert_eq!(
            state.candidates[0].comment.as_deref(),
            Some("aoo 没问题'"),
            "schema={schema_id}, candidates={:?}",
            state.candidates
        );
        if schema_id == "snow_flyjd" {
            assert_eq!(state.candidates[1].text, "莫扎特", "schema={schema_id}");
            assert_eq!(
                state.candidates[1].comment.as_deref(),
                Some("iiu"),
                "schema={schema_id}"
            );
        }
    }
}
