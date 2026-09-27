use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform};
use serde_json::json;
use std::{path::Path, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: CoreEvent) {}
}

fn open_core(path: &Path) -> Arc<Core> {
    Core::new(
        AppEnv {
            data_dir: path.into(),
            cache_dir: path.join("cache"),
            resource_dir: path.join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(NullSink),
    )
    .unwrap()
}

// Only the external HTTP boundary is replaced; parsing and persistence are real.
async fn respond(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    respond_after(status, body, || {}).await
}

async fn respond_after(
    status: &str,
    body: &str,
    before_response: impl FnOnce() + Send + 'static,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let n = stream.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0, "request ended prematurely");
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|s| s.parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            before_response();
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(bytes).unwrap()
        })
        .await
        .expect("local HTTP fixture timed out")
    });
    (url, server)
}

#[tokio::test]
async fn failed_auth_responses_preserve_the_stored_session() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    core.import_legacy_state(Some("stored-token".into()), Some(json!({"id":7})), false)
        .unwrap();
    let before = core.state();
    for login in [false, true] {
        for (status, body, expected) in [
            (
                "401 Unauthorized",
                r#"{"error":"fixture rejection"}"#,
                "fixture rejection",
            ),
            (
                "200 OK",
                "{}",
                if login {
                    "解析登录响应失败: "
                } else {
                    "解析账号信息失败: "
                },
            ),
        ] {
            let (url, server) = respond(status, body).await;
            let error = if login {
                keytao_login_at(&core, "alice".into(), "password".into(), &url)
                    .await
                    .err()
                    .unwrap()
            } else {
                keytao_me_at(&core, "stored-token".into(), &url)
                    .await
                    .err()
                    .unwrap()
            };
            assert!(matches!(&error, CoreError::Other(_)));
            assert!(error.to_string().starts_with(expected), "{error}");
            server.await.unwrap();
            assert!(core.state() == before);
            assert!(open_core(dir.path()).state() == before);
        }
    }
    assert_eq!(
        keytao_login(&core, " ".into(), "password".into())
            .await
            .err()
            .unwrap()
            .to_string(),
        "用户名和密码不能为空"
    );
    assert_eq!(
        keytao_me(&core, " ".into())
            .await
            .err()
            .unwrap()
            .to_string(),
        "未登录"
    );
    assert!(core.state() == before);
}

#[tokio::test]
async fn me_response_cannot_restore_cleared_auth_or_modify_a_new_login() {
    for clear in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        core.import_legacy_state(Some("old-token".into()), Some(json!({"id":7})), false)
            .unwrap();
        let pending = core.clone();
        let (url, server) = respond_after("200 OK", r#"{"id":7,"name":"stale"}"#, move || {
            if clear {
                pending.clear_auth().unwrap();
            } else {
                pending
                    .update_state(|state| {
                        state.auth_token = Some("new-token".into());
                        state.user = Some(json!({"id":9}));
                    })
                    .unwrap();
            }
        })
        .await;
        keytao_me_at(&core, "old-token".into(), &url).await.unwrap();
        server.await.unwrap();
        let state = open_core(dir.path()).state();
        assert_eq!(
            state.auth_token.as_deref(),
            if clear { None } else { Some("new-token") }
        );
        assert_eq!(state.user, if clear { None } else { Some(json!({"id":9})) });
    }
}

#[tokio::test]
async fn login_storage_failure_does_not_publish_a_partial_session() {
    for existing_session in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        if existing_session {
            core.import_legacy_state(Some("old-token".into()), Some(json!({"id":7})), true)
                .unwrap();
            std::fs::remove_file(dir.path().join("app-state.json")).unwrap();
        }
        let before = core.state();
        std::fs::create_dir(dir.path().join("app-state.json")).unwrap();
        let expected = json!({"token":"new-token","user":{"id":9,"name":null,"nickname":null,"email":null}});
        let (url, server) = respond("200 OK", &expected.to_string()).await;
        let session = keytao_login_at(&core, "alice".into(), "password".into(), &url)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(serde_json::to_value(session).unwrap(), expected);
        assert!(core.state() == before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn me_storage_failure_returns_user_without_changing_session() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    core.import_legacy_state(Some("stored-token".into()), Some(json!({"id":7})), true)
        .unwrap();
    let before = core.state();
    std::fs::remove_file(dir.path().join("app-state.json")).unwrap();
    std::fs::create_dir(dir.path().join("app-state.json")).unwrap();
    let expected = json!({"id":7,"name":"updated","nickname":null,"email":null});
    let (url, server) = respond("200 OK", &expected.to_string()).await;
    let user = keytao_me_at(&core, "stored-token".into(), &url)
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(serde_json::to_value(user).unwrap(), expected);
    assert!(core.state() == before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn me_refreshes_only_the_current_stored_token() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let old_user = json!({"id":9,"name":"old"});
    core.import_legacy_state(Some("stored-token".into()), Some(old_user.clone()), false)
        .unwrap();
    let expected = json!({"id":9,"name":"updated","nickname":null,"email":null});
    for token in ["other-token", "stored-token"] {
        let (url, server) = respond("200 OK", &expected.to_string()).await;
        let user = keytao_me_at(&core, token.into(), &url).await.unwrap();
        assert_eq!(serde_json::to_value(user).unwrap(), expected);
        let state = open_core(dir.path()).state();
        assert_eq!(state.auth_token.as_deref(), Some("stored-token"));
        assert_eq!(
            state.user,
            Some(if token == "stored-token" {
                expected.clone()
            } else {
                old_user.clone()
            })
        );
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /api/auth/me HTTP/1.1\r\n"));
        assert!(request.contains(&format!("authorization: Bearer {token}\r\n")));
    }
    let empty = tempfile::tempdir().unwrap();
    let core = open_core(empty.path());
    let (url, server) = respond("200 OK", &expected.to_string()).await;
    keytao_me_at(&core, "unstored-token".into(), &url)
        .await
        .unwrap();
    server.await.unwrap();
    assert!(open_core(empty.path()).state() == crate::AppState::default());
}

#[tokio::test]
async fn login_persists_token_and_user_without_changing_response() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let expected =
        json!({"token":"new-token","user":{"id":9,"name":"alice","nickname":null,"email":null}});
    let (url, server) = respond("200 OK", &expected.to_string()).await;
    let session = keytao_login_at(&core, " alice ".into(), "test-password".into(), &url)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(session).unwrap(), expected);
    let state = open_core(dir.path()).state();
    assert_eq!(state.auth_token.as_deref(), Some("new-token"));
    assert_eq!(state.user, Some(expected["user"].clone()));
    let request = server.await.unwrap();
    assert!(request.starts_with("POST /api/auth/login HTTP/1.1\r\n"));
    assert!(request.contains("user-agent: keytao-app/test\r\n"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(request.split("\r\n\r\n").nth(1).unwrap())
            .unwrap(),
        json!({"name":"alice","password":"test-password"})
    );
}
