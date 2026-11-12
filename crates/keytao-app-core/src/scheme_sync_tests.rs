use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform};
use serde_json::json;
use std::sync::Arc;

struct Sink;
impl EventSink for Sink {
    fn emit(&self, _: CoreEvent) {}
}

fn fixture() -> (tempfile::TempDir, Arc<Core>) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(
        AppEnv {
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
            resource_dir: dir.path().join("resources"),
            app_version: "fixture".into(),
            platform: Platform::MacOs,
        },
        Arc::new(Sink),
    )
    .unwrap();
    core.import_legacy_state(
        Some("private-token".into()),
        Some(json!({"id":7,"email":"private@example.invalid"})),
        false,
    )
    .unwrap();
    (dir, core)
}

#[test]
fn profile_serialization_excludes_secrets_paths_logs_and_banner_state() {
    let (_dir, core) = fixture();
    core.update_state(|state| {
        state
            .dismissed_scheme_updates
            .insert("xmjd".into(), "banner".into());
        state.scheme_sync_settings = json!({
            "download_source":"gitee", "selected_scheme":"xmjd-feiding",
            "auth_token":"nested-secret", "log_dir":"/private/logs",
            "ime_ui":{"accent_color":"#112233","theme_path":"/private/theme"},
            "android":{"haptics_enabled":true,"device_path":"/private/device"}
        });
    })
    .unwrap();
    let profile = export_profile(&core).unwrap();
    assert_eq!(
        profile,
        json!({
            "version":1,"active_scheme":null,"packages":[],
            "settings":{"download_source":"gitee","selected_scheme":"xmjd-feiding",
                "ime_ui":{"accent_color":"#112233"},"android":{"haptics_enabled":true}}
        })
    );
}

#[test]
fn account_change_clears_private_package_references_but_token_refresh_preserves_them() {
    let (_dir, core) = fixture();
    core.update_state(|state| {
        state.scheme_cloud_ids.insert("xmjd".into(), "1".into());
    })
    .unwrap();
    core.update_state(|state| state.auth_token = Some("refreshed-token".into()))
        .unwrap();
    assert_eq!(
        core.state()
            .scheme_cloud_ids
            .get("xmjd")
            .map(String::as_str),
        Some("1")
    );
    core.update_state(|state| {
        state.user = Some(json!({"id":8}));
    })
    .unwrap();
    assert!(core.state().scheme_cloud_ids.is_empty());
}

#[test]
fn cloud_package_without_reference_keeps_its_source_in_the_profile() {
    let (_dir, core) = fixture();
    let store = core.env().data_dir.join("scheme-packages");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("xmjd.json"),json!({"scheme_key":"xmjd","version":"v1","installed_at_unix":0,"source":"cloud","manifest":{},"zip_sha256":"a".repeat(64)}).to_string()).unwrap();
    core.update_state(|state| {
        state.installed_packages.insert(
            "xmjd".into(),
            crate::state::InstalledPackage {
                version: Some("v1".into()),
            },
        );
    })
    .unwrap();
    assert_eq!(
        export_profile(&core).unwrap()["packages"][0]["source"],
        format!("cloud-sha256:{}", "a".repeat(64))
    );
}

#[tokio::test]
async fn apply_preserves_unlisted_packages_and_requires_separate_activation_confirmation() {
    let (dir, core) = fixture();
    core.update_state(|state| {
        for key in ["keytao", "xmjd"] {
            state.installed_packages.insert(
                key.into(),
                crate::state::InstalledPackage {
                    version: Some("v1".into()),
                },
            );
        }
        state.active_scheme = Some("keytao".into());
    })
    .unwrap();
    let profile = json!({"version":1,"active_scheme":"xmjd","packages":[
        {"scheme_key":"xmjd","version":"v1","source":"official"}
    ],"settings":{"download_source":"github"}});
    let result = apply_profile_at(&core, profile, dir.path(), "http://127.0.0.1:1")
        .await
        .unwrap();
    assert_eq!(
        core.state().scheme_sync_settings,
        json!({"download_source":"github"})
    );
    assert_eq!(core.state().installed_packages.len(), 2);
    assert_eq!(core.state().active_scheme.as_deref(), Some("keytao"));
    assert_eq!(result["active_scheme"], "xmjd");
}

#[tokio::test]
async fn package_storage_503_keeps_profile_put_get_working() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let (_dir, core) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let profile = export_profile(&core).unwrap();
    let expected = profile.clone();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in [
            (
                "503 Service Unavailable",
                json!({"error":"storage unavailable"}),
            ),
            (
                "200 OK",
                json!({"profile":expected,"updateAt":"2026-11-11T00:00:00Z"}),
            ),
            (
                "200 OK",
                json!({"profile":expected,"updateAt":"2026-11-11T00:00:00Z"}),
            ),
        ] {
            let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept()).await
            else {
                break;
            };
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0; 2048];
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .map(|v| v.parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(bytes).unwrap());
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    let packages = crate::cloud_packages::list_at(&core, &url).await;
    let upload = upload_profile_at(&core, &url).await;
    let download = fetch_profile_at(&core, &url).await;
    let requests = task.await.unwrap();
    assert!(packages.is_err());
    assert!(upload.is_ok(), "{upload:?}");
    assert!(download.is_ok(), "{download:?}");
    assert_eq!(requests.len(), 3);
    assert!(requests[1].starts_with("PUT /api/user/scheme-profile "));
    assert!(requests[2].starts_with("GET /api/user/scheme-profile "));
    let body: Value = serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"data":profile}));
}

#[tokio::test]
async fn cloud_reference_for_another_scheme_is_rejected_before_storing() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let (dir, core) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        stream.read(&mut buffer).await.unwrap();
        let body = json!({"packages":[{"id":1,"schemeKey":"keytao","name":"fixture","version":"v1","size":10,"sha256":"a".repeat(64),"createAt":"2026-11-11T00:00:00Z"}]}).to_string();
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    let profile = json!({"version":1,"active_scheme":"xmjd","packages":[{"scheme_key":"xmjd","version":"v1","source":"cloud:1"}],"settings":{}});
    let error = apply_profile_at(&core, profile, dir.path(), &url)
        .await
        .unwrap_err();
    task.await.unwrap();
    assert!(error.to_string().contains("来源不匹配"), "{error}");
    assert!(core.state().installed_packages.is_empty());
}
