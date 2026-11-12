use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, io::Write, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

struct Sink;
impl EventSink for Sink {
    fn emit(&self, _: CoreEvent) {}
}

fn fixture() -> (tempfile::TempDir, Arc<Core>, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
            resource_dir: dir.path().join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(Sink),
    )
    .unwrap();
    core.import_legacy_state(Some("fixture-token".into()), Some(json!({"id":7})), false)
        .unwrap();
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file(
            "xmjd6.schema.yaml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(b"schema: xmjd6\n").unwrap();
    let bytes = archive.finish().unwrap().into_inner();
    crate::packages::stage_install(&core, &bytes, Default::default(), String::new()).unwrap();
    crate::packages::finalize_pending(&core, "xmjd", Some("v1"), false).unwrap();
    core.update_state(|state| {
        state.installed_packages.insert(
            "xmjd".into(),
            crate::state::InstalledPackage {
                version: Some("v1".into()),
            },
        );
    })
    .unwrap();
    (dir, core, bytes)
}

fn package(bytes: &[u8]) -> Value {
    json!({"id":21,"schemeKey":"xmjd","name":"xmjd","version":"v1","size":bytes.len(),
        "sha256":format!("{:x}", Sha256::digest(bytes)),"createAt":"2026-11-11T00:00:00.000Z"})
}

fn deployed_fixture() -> (tempfile::TempDir, Arc<Core>, Vec<u8>) {
    let (dir, core, _) = fixture();
    let root = dir.path().join("rime");
    std::fs::create_dir_all(root.join("build")).unwrap();
    let schema = "schema:\n  schema_id: xmjd6\nspeller:\n  alphabet: abc\n";
    std::fs::write(root.join("xmjd6.schema.yaml"), schema).unwrap();
    std::fs::File::open(root.join("xmjd6.schema.yaml"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
        .unwrap();
    std::fs::write(root.join("build/xmjd6.schema.yaml"), schema).unwrap();
    core.update_state(|state| state.active_scheme = Some("xmjd".into()))
        .unwrap();
    crate::deploy::record_result(&core, &root, true).unwrap();
    let bytes = prepare_with_smoke(&core, "xmjd", &root, None, |_, _| Ok(()))
        .unwrap()
        .bundle
        .bytes;
    (dir, core, bytes)
}

struct Response {
    status: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
fn response(status: &'static str, body: Value) -> Response {
    Response {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.to_string().into_bytes(),
    }
}
struct Server {
    url: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Vec<Vec<u8>>>,
}
async fn server(make: impl FnOnce(&str) -> Vec<Response>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut responses: VecDeque<_> = make(&url).into();
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        loop {
            let incoming =
                tokio::select! {_= &mut stopped=>break,incoming=listener.accept()=>incoming};
            let (mut stream, _) = incoming.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            requests.push(bytes);
            let answer = responses
                .pop_front()
                .unwrap_or_else(|| response("500 Internal Server Error", json!({})));
            let mut wire = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                answer.status,
                answer.body.len()
            );
            for (name, value) in answer.headers {
                wire.push_str(&format!("{name}: {value}\r\n"));
            }
            wire.push_str("\r\n");
            stream.write_all(wire.as_bytes()).await.unwrap();
            stream.write_all(&answer.body).await.unwrap();
        }
        requests
    });
    Server { url, stop, task }
}
impl Server {
    async fn finish(self) -> Vec<Vec<u8>> {
        self.stop.send(()).unwrap();
        self.task.await.unwrap()
    }
}

#[tokio::test]
async fn upload_uses_contract_sequence_and_only_ticket_headers_for_storage() {
    let (dir, core, bytes) = deployed_fixture();
    let ready = package(&bytes);
    let mock=server(|url|vec![
        response("200 OK",json!({"packages":[]})),
        response("201 Created",json!({"id":21,"uploadUrl":format!("{url}/storage"),"headers":{"Content-Length":bytes.len().to_string(),"x-amz-meta-fixture":"signed-value"}})),
        response("200 OK",json!({})), response("200 OK",json!({"id":21,"status":"READY"})),
        response("200 OK",json!({"packages":[ready]})),
    ]).await;
    let result = upload_at(&core, "xmjd", &dir.path().join("rime"), &mock.url).await;
    let requests = mock.finish().await;
    assert_eq!(result.unwrap().id, "21");
    let headers: Vec<_> = requests
        .iter()
        .map(|r| {
            String::from_utf8_lossy(r)
                .split("\r\n\r\n")
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(headers[0].starts_with("GET /api/user/scheme-packages HTTP/1.1"));
    assert!(headers[1].starts_with("POST /api/user/scheme-packages HTTP/1.1"));
    assert!(headers[2].starts_with("PUT /storage HTTP/1.1"));
    assert!(headers[3].starts_with("POST /api/user/scheme-packages/21/complete HTTP/1.1"));
    assert!(
        headers[2]
            .to_ascii_lowercase()
            .contains("x-amz-meta-fixture: signed-value")
    );
    assert!(!headers[2].to_ascii_lowercase().contains("authorization:"));
    assert!(!headers[2].to_ascii_lowercase().contains("content-type:"));
    let body = requests[2]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap()
        + 4;
    assert_eq!(&requests[2][body..], bytes.as_slice());
    let json_body = requests[1]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap()
        + 4;
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1][json_body..]).unwrap(),
        json!({"schemeKey":"xmjd","name":"xmjd","version":"v1","size":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes))})
    );
}

#[tokio::test]
async fn upload_reuses_ready_package_with_same_sha_without_creating_upload() {
    let (dir, core, bytes) = deployed_fixture();
    let mock = server(|_| vec![response("200 OK", json!({"packages":[package(&bytes)]}))]).await;
    let result = upload_at(&core, "xmjd", &dir.path().join("rime"), &mock.url).await;
    let requests = mock.finish().await;
    assert_eq!(result.unwrap().id, "21");
    assert_eq!(requests.len(), 1);
    assert_eq!(core.state().scheme_cloud_ids["xmjd"], "21");
}

#[tokio::test]
async fn download_sha_mismatch_rejects_without_writing_package_or_state() {
    let (dir, core, bytes) = fixture();
    let mut remote = package(&bytes);
    remote["schemeKey"] = json!("txjx");
    remote["sha256"] = json!("0".repeat(64));
    let mock = server(|url| {
        vec![
            response("200 OK", json!({"packages":[remote]})),
            Response {
                status: "302 Found",
                headers: vec![("Location".into(), format!("{url}/storage"))],
                body: vec![],
            },
            Response {
                status: "200 OK",
                headers: vec![],
                body: bytes.clone(),
            },
        ]
    })
    .await;
    let before = serde_json::to_value(core.state()).unwrap();
    let result = download_at(&core, "21", &dir.path().join("rime"), &mock.url).await;
    let requests = mock.finish().await;
    let error = result.unwrap_err().to_string();
    assert!(error.contains("SHA-256"), "{error}");
    assert_eq!(serde_json::to_value(core.state()).unwrap(), before);
    assert!(
        !core
            .env()
            .data_dir
            .join("scheme-packages/txjx.zip")
            .exists()
    );
    assert!(
        !core
            .env()
            .data_dir
            .join("scheme-packages/txjx.json")
            .exists()
    );
    assert_eq!(requests.len(), 3);
    let storage_headers = String::from_utf8_lossy(&requests[2]).to_ascii_lowercase();
    assert!(!storage_headers.contains("authorization:"));
}

#[tokio::test]
async fn storage_unavailable_is_short_error_and_preserves_login_and_profile_state() {
    let (_dir, core, _bytes) = fixture();
    let before = serde_json::to_value(core.state()).unwrap();
    let mock = server(|_| {
        vec![response(
            "503 Service Unavailable",
            json!({"error":"对象存储未配置"}),
        )]
    })
    .await;
    let result = list_at(&core, &mock.url).await;
    let requests = mock.finish().await;
    assert_eq!(result.unwrap_err().to_string(), "云端方案暂不可用");
    assert_eq!(serde_json::to_value(core.state()).unwrap(), before);
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn downloading_conflicting_local_package_preserves_existing_content_and_active_scheme() {
    for active in [false, true] {
        let (dir, core, bytes) = fixture();
        let source = dir.path().join("rime");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("own.schema.yaml"), "schema: {schema_id: own}\n").unwrap();
        let own = crate::packages::import_local_scheme(&core, &source, "Own", None).unwrap();
        assert_eq!(own, "local-rime");
        crate::packages::import_local_scheme(&core, &source, "Other", None).unwrap();
        let before = crate::packages::stored_archive(&core, &own).unwrap();
        core.update_state(|state| {
            state.scheme_cloud_ids.insert(own.clone(), "19".into());
            if active {
                state.active_scheme = Some(own.clone());
            }
        })
        .unwrap();
        let mut remote = package(&bytes);
        remote["schemeKey"] = json!(own);
        let mock = server(|url| {
            vec![
                response("200 OK", json!({"packages":[remote]})),
                Response {
                    status: "302 Found",
                    headers: vec![("Location".into(), format!("{url}/storage"))],
                    body: vec![],
                },
                Response {
                    status: "200 OK",
                    headers: vec![],
                    body: bytes.clone(),
                },
            ]
        })
        .await;
        download_at(&core, "21", &source, &mock.url).await.unwrap();
        assert_eq!(mock.finish().await.len(), 3);
        let after = crate::packages::stored_archive(&core, &own).unwrap();
        assert_eq!(after.0, before.0);
        assert_eq!(after.1.zip_sha256, before.1.zip_sha256);
        assert_eq!(core.state().scheme_cloud_ids[&own], "19");
        assert_eq!(core.state().scheme_cloud_ids["local-rime-3"], "21");
        assert_eq!(
            crate::packages::stored_archive(&core, "local-rime-3")
                .unwrap()
                .0,
            bytes
        );
        assert_eq!(core.state().active_scheme, active.then_some(own));
    }
}

#[tokio::test]
async fn downloading_identical_local_package_reuses_key() {
    let (dir, core, bytes) = fixture();
    let root = dir.path().join("rime");
    assert_eq!(
        crate::packages::store_cloud_archive(&core, "local-rime", Some("v1"), None, &bytes, &root)
            .unwrap(),
        "local-rime"
    );
    let mut remote = package(&bytes);
    remote["schemeKey"] = json!("local-rime");
    let mock = server(|url| {
        vec![
            response("200 OK", json!({"packages":[remote]})),
            Response {
                status: "302 Found",
                headers: vec![("Location".into(), format!("{url}/storage"))],
                body: vec![],
            },
            Response {
                status: "200 OK",
                headers: vec![],
                body: bytes.clone(),
            },
        ]
    })
    .await;
    download_at(&core, "21", &root, &mock.url).await.unwrap();
    assert_eq!(mock.finish().await.len(), 3);
    assert_eq!(core.state().scheme_cloud_ids["local-rime"], "21");
    assert!(!core.state().installed_packages.contains_key("local-rime-2"));
    assert_eq!(
        crate::packages::stored_archive(&core, "local-rime")
            .unwrap()
            .0,
        bytes
    );
}

#[tokio::test]
async fn downloaded_cloud_package_can_activate_without_losing_cloud_source_or_id() {
    let (dir, core, bytes) = fixture();
    let mock = server(|url| {
        vec![
            response("200 OK", json!({"packages":[package(&bytes)]})),
            Response {
                status: "302 Found",
                headers: vec![("Location".into(), format!("{url}/storage"))],
                body: vec![],
            },
            Response {
                status: "200 OK",
                headers: vec![],
                body: bytes.clone(),
            },
        ]
    })
    .await;
    let root = dir.path().join("rime");
    download_at(&core, "21", &root, &mock.url).await.unwrap();
    mock.finish().await;
    assert!(core.state().active_scheme.is_none());
    crate::packages::activate_scheme(&core, "xmjd", &root)
        .await
        .unwrap();
    let meta = crate::packages::stored_archive(&core, "xmjd").unwrap().1;
    assert_eq!(meta.source, "cloud");
    assert_eq!(core.state().scheme_cloud_ids["xmjd"], "21");
    assert_eq!(core.state().active_scheme.as_deref(), Some("xmjd"));
}

#[tokio::test]
async fn identical_archive_under_another_scheme_skips_upload_without_changing_cloud_reference() {
    let (dir, core, bytes) = deployed_fixture();
    core.update_state(|state| {
        state.scheme_cloud_ids.insert("xmjd".into(), "19".into());
    })
    .unwrap();
    let mut remote = package(&bytes);
    remote["schemeKey"] = json!("txjx");
    let mock = server(|_| vec![response("200 OK", json!({"packages":[remote]}))]).await;
    let result = upload_at(&core, "xmjd", &dir.path().join("rime"), &mock.url).await;
    let requests = mock.finish().await;
    assert_eq!(result.unwrap().id, "21");
    assert_eq!(requests.len(), 1);
    assert_eq!(core.state().scheme_cloud_ids["xmjd"], "19");
}

#[tokio::test]
async fn android_prepare_resolves_bundled_shared_data() {
    let (dir, _, _) = deployed_fixture();
    let root = dir.path().join("rime");
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("android-state"),
            cache_dir: dir.path().join("cache"),
            resource_dir: dir.path().join("resources"),
            app_version: "test".into(),
            platform: Platform::Android,
        },
        Arc::new(Sink),
    )
    .unwrap();
    let shared = root.join("rime-data");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("default.yaml"), "menu: {page_size: 6}\n").unwrap();
    let schema = "schema:\n  schema_id: xmjd6\nspeller:\n  alphabet: abc\nmenu:\n  __include: default:/menu\n";
    std::fs::write(root.join("xmjd6.schema.yaml"), schema).unwrap();
    std::fs::File::open(root.join("xmjd6.schema.yaml"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
        .unwrap();
    std::fs::write(root.join("build/xmjd6.schema.yaml"), schema).unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(
        "xmjd6.schema.yaml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(schema.as_bytes()).unwrap();
    crate::packages::stage_install(
        &core,
        &zip.finish().unwrap().into_inner(),
        Default::default(),
        String::new(),
    )
    .unwrap();
    crate::packages::finalize_pending(&core, "xmjd", Some("v1"), false).unwrap();
    core.update_state(|state| state.active_scheme = Some("xmjd".into()))
        .unwrap();
    crate::deploy::record_result(&core, &root, true).unwrap();
    let prepared = prepare_android(&core, "xmjd", &root).unwrap();
    assert_eq!(prepared.shared.as_deref(), Some(shared.as_path()));
    assert!(!prepared.bundle.files.contains(&"default.yaml".into()));
}

#[test]
fn android_deploy_receipt_requires_every_active_schema_and_dictionary_artifact() {
    let (dir, core, _) = deployed_fixture();
    let root = dir.path().join("rime");
    std::fs::write(
        root.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: xmjd6\n    - schema: english\n",
    )
    .unwrap();
    let english = "schema:\n  schema_id: english\ntranslator:\n  dictionary: english\n";
    std::fs::write(root.join("english.schema.yaml"), english).unwrap();
    std::fs::write(root.join("english.dict.yaml"), "name: english\n").unwrap();
    for name in ["english.schema.yaml", "english.dict.yaml"] {
        std::fs::File::open(root.join(name))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
    }
    for next in [
        "english.schema.yaml",
        "english.table.bin",
        "english.reverse.bin",
        "english.prism.bin",
    ] {
        let result = crate::deploy::record_android_result(&core, &root, true);
        assert!(
            result.is_err(),
            "Accepted an incomplete build before {next}"
        );
        assert!(!core.state().last_deploy.unwrap().success);
        std::fs::write(
            root.join("build").join(next),
            if next.ends_with("yaml") {
                english
            } else {
                "compiled"
            },
        )
        .unwrap();
    }
    crate::deploy::record_android_result(&core, &root, true).unwrap();
    assert!(core.state().last_deploy.unwrap().success);
    std::fs::write(
        root.join("build/english.schema.yaml"),
        "schema:\n  schema_id: wrong\n",
    )
    .unwrap();
    let error = crate::deploy::record_android_result(&core, &root, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("schema/schema_id"), "{error}");
    assert!(!core.state().last_deploy.unwrap().success);
}

#[test]
fn android_deploy_checks_artifacts_without_requiring_upload_resources() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("rime");
    std::fs::create_dir_all(root.join("build")).unwrap();
    std::fs::create_dir_all(root.join("rime-data")).unwrap();
    std::fs::create_dir_all(root.join("opencc")).unwrap();
    std::fs::create_dir_all(root.join("lua")).unwrap();
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
            resource_dir: root.join("rime-data"),
            app_version: "test".into(),
            platform: Platform::Android,
        },
        Arc::new(Sink),
    )
    .unwrap();
    let schema = "schema:\n  schema_id: xmjd6\nengine:\n  filters: [lua_filter@*lazy_simplifier@emoji_cn]\ntranslator:\n  dictionary: xmjd6.extended\nemoji_cn:\n  opencc_config: emoji.json\n";
    for (name, text) in [
        (
            "default.custom.yaml",
            "patch:\n  schema_list:\n    - schema: xmjd6\n",
        ),
        ("xmjd6.schema.yaml", schema),
        ("build/xmjd6.schema.yaml", schema),
        ("xmjd6.extended.dict.yaml", "name: xmjd6.extended\n"),
        ("build/xmjd6.extended.table.bin", "compiled"),
        ("build/xmjd6.extended.reverse.bin", "compiled"),
        ("build/xmjd6.extended.prism.bin", "compiled"),
        ("lua/lazy_simplifier.lua", "return {}\n"),
        (
            "opencc/emoji.json",
            r#"{"conversion_chain":[{"dict":{"type":"text","file":"emoji.txt"}}]}"#,
        ),
    ] {
        std::fs::write(root.join(name), text).unwrap();
    }

    crate::deploy::record_android_result(&core, &root, true).unwrap();
    assert!(core.state().last_deploy.unwrap().success);
    let upload_error =
        crate::upload_bundle::build(&root, Some(&root.join("rime-data")), &["xmjd6".into()])
            .unwrap_err()
            .to_string();
    assert!(upload_error.contains("opencc/emoji.txt"), "{upload_error}");

    for artifact in [
        "build/xmjd6.schema.yaml",
        "build/xmjd6.extended.table.bin",
        "build/xmjd6.extended.reverse.bin",
        "build/xmjd6.extended.prism.bin",
    ] {
        let path = root.join(artifact);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let error = crate::deploy::record_android_result(&core, &root, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains(artifact), "{error}");
        assert!(!core.state().last_deploy.unwrap().success);
        std::fs::write(path, bytes).unwrap();
    }
}

#[test]
fn android_deploy_accepts_unchanged_sources_newer_than_artifacts() {
    for source in [
        "xmjd6.user.dict.yaml",
        "xmjd6.custom.yaml",
        "xmjd6.schema.yaml",
    ] {
        let (dir, core, _) = deployed_fixture();
        let root = dir.path().join("rime");
        let schema = "schema:\n  schema_id: xmjd6\ntranslator:\n  dictionary: xmjd6\n";
        for (name, text) in [
            (
                "default.custom.yaml",
                "patch:\n  schema_list:\n    - schema: xmjd6\n",
            ),
            ("xmjd6.schema.yaml", schema),
            (
                "xmjd6.dict.yaml",
                "name: xmjd6\nimport_tables: [xmjd6.user]\n",
            ),
            ("xmjd6.user.dict.yaml", "name: xmjd6.user\n"),
            ("xmjd6.custom.yaml", "# Comment-only edit\npatch: {}\n"),
        ] {
            std::fs::write(root.join(name), text).unwrap();
            std::fs::File::open(root.join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
        }
        for suffix in ["schema.yaml", "table.bin", "reverse.bin", "prism.bin"] {
            let path = root.join(format!("build/xmjd6.{suffix}"));
            std::fs::write(
                &path,
                if suffix == "schema.yaml" {
                    schema
                } else {
                    "compiled"
                },
            )
            .unwrap();
            std::fs::File::open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000)),
                )
                .unwrap();
        }
        std::fs::File::open(root.join(source))
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(2000)),
            )
            .unwrap();

        crate::deploy::record_android_result(&core, &root, true).unwrap();
        assert!(core.state().last_deploy.unwrap().success);
        let error = crate::upload_bundle::verified_artifacts(&root, None, &["xmjd6".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("不新于源文件"), "{source}: {error}");
    }
}

#[tokio::test]
async fn upload_rechecks_with_the_prepared_shared_directory() {
    let (dir, core, _) = deployed_fixture();
    let root = dir.path().join("rime");
    let shared = dir.path().join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("symbols.yaml"), "punctuator: {}\n").unwrap();
    let schema = "schema:\n  schema_id: xmjd6\nspeller:\n  alphabet: abc\npunctuator:\n  import_preset: symbols\n";
    std::fs::write(root.join("xmjd6.schema.yaml"), schema).unwrap();
    std::fs::File::open(root.join("xmjd6.schema.yaml"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
        .unwrap();
    std::fs::write(root.join("build/xmjd6.schema.yaml"), schema).unwrap();
    let prepared = prepare_with_smoke(&core, "xmjd", &root, Some(&shared), |_, _| Ok(()))
        .unwrap();
    assert_eq!(prepared.bundle.files, ["default.custom.yaml", "xmjd6.schema.yaml"]);
    let ready = package(&prepared.bundle.bytes);
    let mock = server(|_| vec![response("200 OK", json!({"packages":[ready]}))]).await;
    let result = upload_prepared_at(&core, "xmjd", &root, prepared, &mock.url).await;
    let requests = mock.finish().await;
    assert_eq!(result.unwrap().id, "21");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn inactive_or_failed_deploy_refuses_before_any_cloud_request() {
    let (dir, core, _) = deployed_fixture();
    let root = dir.path().join("rime");
    let mock = server(|_| Vec::new()).await;
    core.update_state(|state| state.active_scheme = Some("txjx".into()))
        .unwrap();
    let error = upload_at(&core, "xmjd", &root, &mock.url)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("xmjd") && error.contains("不是当前"),
        "{error}"
    );
    core.update_state(|state| state.active_scheme = Some("xmjd".into()))
        .unwrap();
    crate::deploy::record_result(&core, &root, false).unwrap();
    let error = upload_at(&core, "xmjd", &root, &mock.url)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("xmjd") && error.contains("部署未成功"),
        "{error}"
    );
    assert!(mock.finish().await.is_empty());
}

#[tokio::test]
async fn installing_into_another_directory_preserves_active_deploy_receipt() {
    let (dir, core, _) = deployed_fixture();
    let before = serde_json::to_value(core.state()).unwrap()["last_deploy"].clone();
    let bytes = crate::packages::stored_archive(&core, "xmjd").unwrap().0;
    let zip = dir.path().join("other.zip");
    let dest = dir.path().join("other-rime");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(&zip, bytes).unwrap();
    crate::install::smart_install(
        &core,
        zip.to_string_lossy().into_owned(),
        dest.to_string_lossy().into_owned(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(core.state()).unwrap()["last_deploy"],
        before
    );
}

#[cfg(target_os = "macos")]
#[test]
fn desktop_preview_smokes_a_shared_dictionary_from_a_small_temp_scheme() {
    let (dir, core, _) = fixture();
    let root = dir.path().join("rime");
    let shared = core.env().resource_dir.join("rime-data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        shared.join("default.yaml"),
        "config_version: '1'\nschema_list:\n  - schema: xmjd6\nmenu:\n  page_size: 5\n",
    )
    .unwrap();
    std::fs::write(
        root.join("default.custom.yaml"),
        "patch:\n  schema_list:\n    - schema: xmjd6\n",
    )
    .unwrap();
    std::fs::write(root.join("xmjd6.schema.yaml"), "schema:\n  schema_id: xmjd6\n  name: Temp fixture\n  version: '1'\nengine:\n  processors: [speller, selector, express_editor]\n  segmentors: [abc_segmentor, fallback_segmentor]\n  translators: [table_translator]\nspeller:\n  alphabet: abcdefghijklmnopqrstuvwxyz\ntranslator:\n  dictionary: xmjd6\n  enable_user_dict: false\n").unwrap();
    std::fs::write(shared.join("xmjd6.dict.yaml"), "---\nname: xmjd6\nversion: '1'\nsort: original\nuse_preset_vocabulary: false\n...\nalpha\ta\n").unwrap();
    core.update_state(|state| state.active_scheme = Some("xmjd".into()))
        .unwrap();
    let runtime = keytao_core::ImeRuntime::with_dirs(&root, shared.to_string_lossy().into_owned());
    runtime.init().unwrap();
    crate::deploy::record_result(&core, &root, true).unwrap();
    let preview = preview(&core, "xmjd", &root).unwrap();
    assert_eq!(
        preview.files,
        [
            "default.custom.yaml",
            "xmjd6.schema.yaml"
        ]
    );
    assert!(preview.validation.contains("冒烟检查"));
}

#[test]
fn oversized_distribution_can_prepare_a_small_dependency_closure() {
    let (dir, core, _) = deployed_fixture();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(
        "xmjd6.schema.yaml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(b"schema: xmjd6\n").unwrap();
    zip.start_file(
        "other_scheme.bin",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(&vec![0; 31 * 1024 * 1024]).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert!(bytes.len() as u64 > MAX_PACKAGE_SIZE);
    crate::packages::stage_install(&core, &bytes, Default::default(), String::new()).unwrap();
    crate::packages::finalize_pending(&core, "xmjd", Some("v1"), false).unwrap();
    let prepared =
        prepare_with_smoke(&core, "xmjd", &dir.path().join("rime"), None, |_, _| Ok(())).unwrap();
    assert!(prepared.bundle.bytes.len() < 1024);
    assert_eq!(
        prepared.bundle.files,
        ["default.custom.yaml", "xmjd6.schema.yaml"]
    );
}

#[tokio::test]
async fn logout_while_storage_download_is_running_prevents_local_account_reference_write() {
    let (dir, core, bytes) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let storage_url = format!("http://{}/storage", listener.local_addr().unwrap());
    let download_core = core.clone();
    let downloaded = bytes.clone();
    let storage = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).await.unwrap();
        download_core.clear_auth().unwrap();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    downloaded.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&downloaded).await.unwrap();
    });
    let mut remote = package(&bytes);
    remote["schemeKey"] = json!("txjx");
    let mock = server(|_| {
        vec![
            response("200 OK", json!({"packages":[remote]})),
            Response {
                status: "302 Found",
                headers: vec![("Location".into(), storage_url)],
                body: vec![],
            },
        ]
    })
    .await;
    let result = download_at(&core, "21", &dir.path().join("rime"), &mock.url).await;
    mock.finish().await;
    storage.await.unwrap();
    assert!(matches!(result, Err(CoreError::Permission(_))));
    assert!(!core.state().installed_packages.contains_key("txjx"));
    assert!(!core.state().scheme_cloud_ids.contains_key("txjx"));
    assert!(
        !core
            .env()
            .data_dir
            .join("scheme-packages/txjx.zip")
            .exists()
    );
}
