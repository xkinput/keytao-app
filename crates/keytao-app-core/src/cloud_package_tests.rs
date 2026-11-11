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
    let (_dir, core, bytes) = fixture();
    let ready = package(&bytes);
    let mock=server(|url|vec![
        response("200 OK",json!({"packages":[]})),
        response("201 Created",json!({"id":21,"uploadUrl":format!("{url}/storage"),"headers":{"Content-Length":bytes.len().to_string(),"x-amz-meta-fixture":"signed-value"}})),
        response("200 OK",json!({})), response("200 OK",json!({"id":21,"status":"READY"})),
        response("200 OK",json!({"packages":[ready]})),
    ]).await;
    let result = upload_at(&core, "xmjd", &mock.url).await;
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
    let (_dir, core, bytes) = fixture();
    let mock = server(|_| vec![response("200 OK", json!({"packages":[package(&bytes)]}))]).await;
    let result = upload_at(&core, "xmjd", &mock.url).await;
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
    let (_dir, core, bytes) = fixture();
    core.update_state(|state| {
        state.scheme_cloud_ids.insert("xmjd".into(), "19".into());
    })
    .unwrap();
    let mut remote = package(&bytes);
    remote["schemeKey"] = json!("txjx");
    let mock = server(|_| vec![response("200 OK", json!({"packages":[remote]}))]).await;
    let result = upload_at(&core, "xmjd", &mock.url).await;
    let requests = mock.finish().await;
    assert_eq!(result.unwrap().id, "21");
    assert_eq!(requests.len(), 1);
    assert_eq!(core.state().scheme_cloud_ids["xmjd"], "19");
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
