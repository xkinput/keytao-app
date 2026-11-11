use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform, user_words};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

struct Sink;
impl EventSink for Sink {
    fn emit(&self, _: CoreEvent) {}
}

struct Host(PathBuf);
impl ImeHost for Host {
    fn user_root(&self) -> Result<PathBuf, String> {
        Ok(self.0.clone())
    }
    fn write_reload_stamp(&self, _: &Path) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
}

fn fixture() -> (tempfile::TempDir, Arc<Core>, Host) {
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
    let host = Host(dir.path().join("rime"));
    std::fs::create_dir_all(&host.0).unwrap();
    std::fs::write(
        host.0.join("keytao.user.dict.yaml"),
        "# keep me\n---\nname: keytao.user\ncolumns: [code, text, weight]\n...\n",
    )
    .unwrap();
    (dir, core, host)
}

struct Server {
    url: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Vec<String>>,
}

async fn server(responses: Vec<(&str, Value)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let responses: VecDeque<_> = responses
        .into_iter()
        .map(|(status, body)| (status.to_owned(), body.to_string()))
        .collect();
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut responses = responses;
        loop {
            let incoming = tokio::select! {
                _ = &mut stopped => break,
                incoming = listener.accept() => incoming,
            };
            let (mut stream, _) = incoming.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 2048];
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
            requests.push(String::from_utf8(bytes).unwrap());
            let (status, body) = responses
                .pop_front()
                .unwrap_or_else(|| ("500 Internal Server Error".into(), "{}".into()));
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    Server { url, stop, task }
}

impl Server {
    async fn finish(self) -> Vec<String> {
        self.stop.send(()).unwrap();
        self.task.await.unwrap()
    }
}

fn remote(word: &str, code: &str, id: i64) -> Value {
    json!({"id":id,"word":word,"code":code,"type":"Phrase","weight":123,"userId":7})
}

fn local(core: &Core, host: &Host) -> Vec<(String, String)> {
    let mut entries: Vec<_> = user_words::list_user_words(core, "keytao", &host.0)
        .unwrap()
        .into_iter()
        .map(|v| (v.word, v.code))
        .collect();
    entries.sort();
    entries
}

#[tokio::test]
async fn first_sync_unions_local_and_remote_preserving_file_and_remote_weight() {
    let (_dir, core, host) = fixture();
    user_words::add_user_word(&core, "keytao", &host.0, "local", "aaaa", None).unwrap();
    let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("remote","bbbb",12)],"total":1}),
        ),
        ("200 OK", json!({"entry":remote("local","aaaa",13)})),
    ])
    .await;
    let result =
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
    let requests = mock.finish().await;
    assert!(result.is_ok(), "{}", result.err().unwrap());
    assert_eq!(
        local(&core, &host),
        [
            ("local".into(), "aaaa".into()),
            ("remote".into(), "bbbb".into())
        ]
    );
    let contents = std::fs::read_to_string(host.0.join("keytao.user.dict.yaml")).unwrap();
    assert!(contents.starts_with("# keep me\n"));
    assert!(contents.contains("bbbb\tremote\t123"));
    assert!(requests[0].starts_with("GET /api/user/dictionary HTTP/1.1"));
    assert!(requests[1].starts_with("POST /api/user/dictionary HTTP/1.1"));
    assert_eq!(
        serde_json::from_str::<Value>(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap(),
        json!({"word":"local","code":"aaaa","type":"Phrase","replacePublic":false})
    );
}

#[tokio::test]
async fn three_way_merge_table_covers_every_local_remote_base_membership() {
    let mut observed = Vec::new();
    for (in_local, in_remote, in_base) in [
        (true, true, false),
        (true, true, true),
        (true, false, false),
        (true, false, true),
        (false, true, false),
        (false, true, true),
        (false, false, false),
        (false, false, true),
    ] {
        let (_dir, core, host) = fixture();
        user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
        if in_local {
            user_words::add_user_word(&core, "keytao", &host.0, "word", "aaaa", None).unwrap();
        }
        save_base(
            &host.0,
            7,
            &if in_base {
                BTreeSet::from([
                    ("kept".into(), "zzzz".into()),
                    ("word".into(), "aaaa".into()),
                ])
            } else {
                BTreeSet::new()
            },
        )
        .unwrap();
        let mut entries = vec![remote("kept", "zzzz", 11)];
        if in_remote {
            entries.push(remote("word", "aaaa", 12));
        }
        let mock = server(vec![
            ("200 OK", json!({"total":entries.len(),"entries":entries})),
            (
                "200 OK",
                json!({"success":true,"entry":remote("word","aaaa",12)}),
            ),
        ])
        .await;
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None)
            .await
            .unwrap();
        let requests = mock.finish().await;
        let action = requests
            .get(1)
            .map(|request| request.split_whitespace().next().unwrap().to_owned());
        let base: DictionaryBase =
            serde_json::from_slice(&std::fs::read(base_path(&host.0, 7)).unwrap()).unwrap();
        let key = ("word".into(), "aaaa".into());
        observed.push((
            local(&core, &host).contains(&key),
            base.entries.contains(&key),
            action,
        ));
    }
    assert_eq!(
        observed,
        vec![
            (true, true, None),
            (true, true, None),
            (true, true, Some("POST".into())),
            (false, false, None),
            (true, true, None),
            (false, false, Some("DELETE".into())),
            (false, false, None),
            (false, false, None)
        ]
    );
}

#[tokio::test]
async fn remote_list_failure_or_malformed_response_never_deletes_or_updates_base() {
    for (status, body) in [
        ("500 Internal Server Error", json!({"error":"fixture"})),
        ("200 OK", json!({"entries":[],"total":1})),
        ("200 OK", json!({"entries":[]})),
        (
            "200 OK",
            json!({"entries":[{"id":12,"word":"word","code":"aaaa","type":"Phrase","weight":123,"userId":99}],"total":1}),
        ),
        (
            "200 OK",
            json!({"entries":[{"word":"bad","code":"aaaa"}],"total":1}),
        ),
    ] {
        let (_dir, core, host) = fixture();
        user_words::add_user_word(&core, "keytao", &host.0, "word", "aaaa", None).unwrap();
        save_base(
            &host.0,
            7,
            &BTreeSet::from([("word".into(), "aaaa".into())]),
        )
        .unwrap();
        let before = std::fs::read(base_path(&host.0, 7)).unwrap();
        let dictionary = std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap();
        let mock = server(vec![(status, body)]).await;
        assert!(
            sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None)
                .await
                .is_err()
        );
        let requests = mock.finish().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /api/user/dictionary HTTP/1.1"));
        assert_eq!(std::fs::read(base_path(&host.0, 7)).unwrap(), before);
        assert_eq!(
            std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap(),
            dictionary
        );
    }
}

#[tokio::test]
async fn rate_limit_stops_sync_without_partial_base_update() {
    let (_dir, core, host) = fixture();
    for (word, code) in [("first", "aaaa"), ("second", "bbbb"), ("third", "cccc")] {
        user_words::add_user_word(&core, "keytao", &host.0, word, code, None).unwrap();
    }
    save_base(&host.0, 7, &BTreeSet::new()).unwrap();
    let before = std::fs::read(base_path(&host.0, 7)).unwrap();
    let mock = server(vec![
        ("200 OK", json!({"entries":[],"total":0})),
        ("200 OK", json!({"entry":remote("first","aaaa",1)})),
        ("429 Too Many Requests", json!({"error":"too fast"})),
    ])
    .await;
    let error = sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None)
        .await
        .err()
        .unwrap();
    let requests = mock.finish().await;
    assert!(error.to_string().contains("429"));
    assert_eq!(requests.len(), 3);
    assert_eq!(std::fs::read(base_path(&host.0, 7)).unwrap(), before);
    assert_eq!(local(&core, &host).len(), 3);
}

fn jwt(iat: u64, exp: u64) -> String {
    jwt_for(7, iat, exp)
}

fn jwt_for(id: i64, iat: u64, exp: u64) -> String {
    let source = json!({"id":id,"iat":iat,"exp":exp}).to_string();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    let mut bits = 0u32;
    let mut count = 0;
    for byte in source.bytes() {
        bits = (bits << 8) | u32::from(byte);
        count += 8;
        while count >= 6 {
            count -= 6;
            output.push(alphabet[((bits >> count) & 63) as usize] as char);
        }
    }
    if count > 0 {
        output.push(alphabet[((bits << (6 - count)) & 63) as usize] as char);
    }
    format!("e30.{output}.fixture-signature")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn unauthorized_refreshes_once_then_retries_with_the_new_bearer() {
    let (_dir, core, _host) = fixture();
    let old = jwt(now() - 172800, now() + 432000);
    let new = jwt(now(), now() + 604800);
    core.update_state(|state| state.auth_token = Some(old.clone()))
        .unwrap();
    let mock = server(vec![
        ("401 Unauthorized", json!({"error":"fixture"})),
        ("200 OK", json!({"token":new})),
        ("200 OK", json!({"entries":[],"total":0})),
    ])
    .await;
    let result = ApiClient::new(&core, &mock.url)
        .request(reqwest::Method::GET, "/api/user/dictionary", None)
        .await;
    let requests = mock.finish().await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(requests.len(), 3);
    assert!(requests[1].starts_with("POST /api/auth/refresh HTTP/1.1"));
    assert!(requests[1].contains(&format!("authorization: Bearer {old}\r\n")));
    assert!(requests[2].contains(&format!("authorization: Bearer {new}\r\n")));
    assert_eq!(core.state().auth_token, Some(new));
}

#[tokio::test]
async fn startup_refreshes_only_old_unexpired_tokens_and_clears_expired_tokens() {
    let mut observed = Vec::new();
    for (age, remaining) in [(172800, 432000), (100, 604700), (604900, 0)] {
        let (_dir, core, _host) = fixture();
        let token = jwt(now() - age, now() + remaining);
        core.update_state(|state| state.auth_token = Some(token))
            .unwrap();
        let mock = server(vec![("200 OK", json!({"token":jwt(now(),now()+604800)}))]).await;
        startup_refresh_at(&core, &mock.url).await.unwrap();
        let requests = mock.finish().await;
        observed.push((requests.len(), core.state().auth_token.is_some()));
    }
    assert_eq!(observed, [(1, true), (0, true), (0, false)]);
}

#[tokio::test]
async fn failed_refresh_clears_the_session_without_retrying_the_action() {
    let (_dir, core, _host) = fixture();
    core.update_state(|state| state.auth_token = Some(jwt(now() - 172800, now() + 432000)))
        .unwrap();
    let mock = server(vec![
        ("401 Unauthorized", json!({"error":"fixture"})),
        ("401 Unauthorized", json!({"error":"invalid token"})),
    ])
    .await;
    assert!(
        ApiClient::new(&core, &mock.url)
            .request(reqwest::Method::GET, "/api/user/dictionary", None)
            .await
            .is_err()
    );
    let requests = mock.finish().await;
    assert_eq!(requests.len(), 2);
    assert!(core.state().auth_token.is_none());
    assert!(core.state().user.is_none());
}

#[tokio::test]
async fn malformed_or_expired_refreshed_token_cannot_replace_the_session() {
    let mut accepted = Vec::new();
    for refreshed in ["opaque-token".into(), jwt(now() - 700000, now() - 1)] {
        let (_dir, core, _host) = fixture();
        core.update_state(|state| state.auth_token = Some(jwt(now() - 172800, now() + 432000)))
            .unwrap();
        let mock = server(vec![("200 OK", json!({"token":refreshed}))]).await;
        let result = startup_refresh_at(&core, &mock.url).await;
        mock.finish().await;
        accepted.push((result.is_ok(), core.state().auth_token.is_some()));
    }
    assert_eq!(accepted, [(false, false), (false, false)]);
}

#[tokio::test]
async fn malformed_upsert_response_stops_before_remote_or_local_deletions() {
    let (_dir, core, host) = fixture();
    user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
    user_words::add_user_word(&core, "keytao", &host.0, "new", "aaaa", None).unwrap();
    save_base(
        &host.0,
        7,
        &BTreeSet::from([
            ("deleted".into(), "bbbb".into()),
            ("kept".into(), "zzzz".into()),
        ]),
    )
    .unwrap();
    let before = std::fs::read(base_path(&host.0, 7)).unwrap();
    let mut uploaded = remote("new", "aaaa", 13);
    uploaded["userId"] = json!(99);
    let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("deleted","bbbb",12),remote("kept","zzzz",11)],"total":2}),
        ),
        ("200 OK", json!({"entry":uploaded})),
        ("200 OK", json!({"success":true})),
    ])
    .await;
    let result =
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
    let requests = mock.finish().await;
    assert_eq!(
        requests.len(),
        2,
        "malformed upsert must stop before DELETE"
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read(base_path(&host.0, 7)).unwrap(), before);
    assert_eq!(
        local(&core, &host),
        [
            ("kept".into(), "zzzz".into()),
            ("new".into(), "aaaa".into())
        ]
    );
}

#[tokio::test]
async fn missing_local_dictionary_restores_remote_union_without_deleting_from_saved_base() {
    let (_dir, core, host) = fixture();
    save_base(
        &host.0,
        7,
        &BTreeSet::from([("word".into(), "aaaa".into())]),
    )
    .unwrap();
    std::fs::remove_file(host.0.join("keytao.user.dict.yaml")).unwrap();
    let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("word","aaaa",12)],"total":1}),
        ),
        ("200 OK", json!({"success":true})),
    ])
    .await;
    let result =
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
    let requests = mock.finish().await;
    assert!(result.is_ok());
    assert_eq!(
        requests.len(),
        1,
        "missing dictionary must never be interpreted as local deletion"
    );
    assert_eq!(local(&core, &host), [("word".into(), "aaaa".into())]);
}

#[tokio::test]
async fn listing_recreated_local_dictionary_restores_remote_without_deleting_saved_base_entries() {
    let (_dir, core, host) = fixture();
    let expected = BTreeSet::from([
        ("X".into(), "aaaa".into()),
        ("Y".into(), "bbbb".into()),
        ("Z".into(), "cccc".into()),
    ]);
    save_base(&host.0, 7, &expected).unwrap();
    let path = host.0.join("keytao.user.dict.yaml");
    std::fs::remove_file(&path).unwrap();
    assert!(
        user_words::list_user_words(&core, "keytao", &host.0)
            .unwrap()
            .is_empty()
    );
    assert!(path.try_exists().unwrap());
    let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("X","aaaa",12),remote("Y","bbbb",13),remote("Z","cccc",14)],"total":3}),
        ),
        ("200 OK", json!({"success":true})),
        ("200 OK", json!({"success":true})),
        ("200 OK", json!({"success":true})),
    ])
    .await;
    let result =
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
    let requests = mock.finish().await;
    assert!(result.is_ok(), "{}", result.err().unwrap());
    assert_eq!(
        requests.len(),
        1,
        "listing a recreated dictionary must never cause DELETE requests"
    );
    assert!(requests[0].starts_with("GET /api/user/dictionary HTTP/1.1"));
    assert_eq!(
        local(&core, &host),
        expected.iter().cloned().collect::<Vec<_>>()
    );
    let saved: DictionaryBase =
        serde_json::from_slice(&std::fs::read(base_path(&host.0, 7)).unwrap()).unwrap();
    assert_eq!(saved.entries, expected);
    assert_eq!(result.unwrap().count, 3);
}

#[tokio::test]
async fn reset_local_dictionary_with_new_word_restores_remote_without_deleting_saved_base_entries()
{
    let (_dir, core, host) = fixture();
    let mut expected = BTreeSet::from([
        ("X".into(), "aaaa".into()),
        ("Y".into(), "bbbb".into()),
        ("Z".into(), "cccc".into()),
    ]);
    save_base(&host.0, 7, &expected).unwrap();
    std::fs::remove_file(host.0.join("keytao.user.dict.yaml")).unwrap();
    assert!(local(&core, &host).is_empty());
    user_words::add_user_word(&core, "keytao", &host.0, "W", "dddd", None).unwrap();
    assert_eq!(local(&core, &host), [("W".into(), "dddd".into())]);
    let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("X","aaaa",12),remote("Y","bbbb",13),remote("Z","cccc",14)],"total":3}),
        ),
        ("200 OK", json!({"entry":remote("W","dddd",15)})),
        ("200 OK", json!({"success":true})),
        ("200 OK", json!({"success":true})),
        ("200 OK", json!({"success":true})),
    ])
    .await;
    let result =
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
    let requests = mock.finish().await;
    assert!(result.is_ok(), "{}", result.err().unwrap());
    assert!(
        requests
            .iter()
            .all(|request| !request.starts_with("DELETE ")),
        "a new word in a reset dictionary must never cause cloud deletions"
    );
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /api/user/dictionary HTTP/1.1"));
    assert!(requests[1].starts_with("POST /api/user/dictionary HTTP/1.1"));
    let uploaded: Value =
        serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(uploaded["word"], "W");
    assert_eq!(uploaded["code"], "dddd");
    expected.insert(("W".into(), "dddd".into()));
    assert_eq!(
        local(&core, &host),
        expected.iter().cloned().collect::<Vec<_>>()
    );
    let saved: DictionaryBase =
        serde_json::from_slice(&std::fs::read(base_path(&host.0, 7)).unwrap()).unwrap();
    assert_eq!(saved.entries, expected);
    assert_eq!(result.unwrap().count, 4);
}

#[tokio::test]
async fn reset_directory_with_readded_base_word_never_deletes_remote_words() {
    for (legacy, reset_directory) in [(false, true), (false, false), (true, true)] {
        let (_dir, core, host) = fixture();
        save_base(
            &host.0,
            7,
            &BTreeSet::from([
                ("X".into(), "aaaa".into()),
                ("Y".into(), "bbbb".into()),
                ("Z".into(), "cccc".into()),
            ]),
        )
        .unwrap();
        if legacy {
            std::fs::write(
                legacy_base_path(&core, 7),
                br#"[["X","aaaa"],["Y","bbbb"],["Z","cccc"]]"#,
            )
            .unwrap();
            std::fs::remove_file(base_path(&host.0, 7)).unwrap();
        }
        if reset_directory {
            std::fs::remove_dir_all(&host.0).unwrap();
            std::fs::create_dir_all(&host.0).unwrap();
        } else {
            std::fs::remove_file(host.0.join("keytao.user.dict.yaml")).unwrap();
        }
        for (word, code) in [("X", "aaaa"), ("W", "dddd")] {
            user_words::add_user_word(&core, "keytao", &host.0, word, code, None).unwrap();
        }
        let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("X","aaaa",12),remote("Y","bbbb",13),remote("Z","cccc",14)],"total":3}),
        ),
        ("200 OK", json!({"entry":remote("W","dddd",15)})),
        ("200 OK", json!({"success":true})),
        ("200 OK", json!({"success":true})),
    ])
    .await;
        let result =
            sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
        let requests = mock.finish().await;
        assert!(result.is_ok());
        assert!(
            requests
                .iter()
                .all(|request| !request.starts_with("DELETE "))
        );
        assert_eq!(
            local(&core, &host),
            [
                ("W".into(), "dddd".into()),
                ("X".into(), "aaaa".into()),
                ("Y".into(), "bbbb".into()),
                ("Z".into(), "cccc".into()),
            ]
        );
        assert!(host.0.join(".keytao-sync-base-7.json").is_file());
        assert!(!legacy_base_path(&core, 7).try_exists().unwrap());
    }
}

#[tokio::test]
async fn external_recreated_dictionary_with_stale_sidecar_never_deletes_remote_words() {
    let (_dir, core, host) = fixture();
    save_base(
        &host.0,
        7,
        &BTreeSet::from([
            ("X".into(), "aaaa".into()),
            ("Y".into(), "bbbb".into()),
            ("Z".into(), "cccc".into()),
        ]),
    )
    .unwrap();
    let dictionary = host.0.join("keytao.user.dict.yaml");
    std::fs::remove_file(&dictionary).unwrap();
    std::fs::write(
        &dictionary,
        "---\nname: keytao.user\ncolumns: [code, text, weight]\n...\naaaa\tX\t123\ndddd\tW\t123\n",
    )
    .unwrap();
    let mock = server(vec![
        (
            "200 OK",
            json!({"entries":[remote("X","aaaa",12),remote("Y","bbbb",13),remote("Z","cccc",14)],"total":3}),
        ),
        ("200 OK", json!({"entry":remote("W","dddd",15)})),
        ("200 OK", json!({"success":true})),
        ("200 OK", json!({"success":true})),
    ])
    .await;

    let result =
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None).await;
    let requests = mock.finish().await;

    assert!(result.is_ok());
    assert!(
        requests
            .iter()
            .all(|request| !request.starts_with("DELETE ")),
        "an externally recreated dictionary must never reuse its stale sidecar for deletions"
    );
    assert_eq!(
        local(&core, &host),
        [
            ("W".into(), "dddd".into()),
            ("X".into(), "aaaa".into()),
            ("Y".into(), "bbbb".into()),
            ("Z".into(), "cccc".into()),
        ]
    );
}

#[tokio::test]
async fn bulk_deletions_without_confirmation_leave_both_sides_and_base_unchanged() {
    for delete_on_server in [false, true] {
        let (_dir, core, host) = fixture();
        let mut base = BTreeSet::from([("kept".into(), "zzzz".into())]);
        user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
        let mut entries = vec![remote("kept", "zzzz", 1)];
        for (id, word, code) in [
            (2, "A", "aaaa"),
            (3, "B", "bbbb"),
            (4, "C", "cccc"),
            (5, "D", "dddd"),
        ] {
            base.insert((word.into(), code.into()));
            if delete_on_server {
                entries.push(remote(word, code, id));
            } else {
                user_words::add_user_word(&core, "keytao", &host.0, word, code, None).unwrap();
            }
        }
        save_base(&host.0, 7, &base).unwrap();
        let base_before = std::fs::read(base_path(&host.0, 7)).unwrap();
        let dictionary_before = std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap();
        let mut responses = vec![("200 OK", json!({"total":entries.len(),"entries":entries}))];
        responses.extend((0..4).map(|_| ("200 OK", json!({"success":true}))));
        let mock = server(responses).await;
        let result = sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None)
            .await
            .unwrap();
        let requests = mock.finish().await;
        assert_eq!(requests.len(), 1, "pending sync must not send DELETE");
        assert_eq!(
            std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap(),
            dictionary_before
        );
        assert_eq!(std::fs::read(base_path(&host.0, 7)).unwrap(), base_before);
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["pending"], true);
        let side = if delete_on_server {
            "server_deletions"
        } else {
            "local_deletions"
        };
        assert_eq!(
            result[side],
            json!([
                {"word":"A","code":"aaaa"}, {"word":"B","code":"bbbb"},
                {"word":"C","code":"cccc"}, {"word":"D","code":"dddd"},
            ])
        );
    }
}

#[tokio::test]
async fn confirmed_local_deletions_recheck_growth_before_deleting() {
    confirmed_deletions_recheck_growth(false, 4).await;
}

#[tokio::test]
async fn confirmed_server_deletions_recheck_growth_before_deleting() {
    confirmed_deletions_recheck_growth(true, 4).await;
}

#[tokio::test]
async fn confirmed_deletions_preserve_small_unconfirmed_excess_on_both_sides() {
    for delete_on_server in [false, true] {
        for extra in 1..=3 {
            confirmed_deletions_recheck_growth(delete_on_server, extra).await;
        }
    }
}

async fn confirmed_deletions_recheck_growth(delete_on_server: bool, extra: usize) {
    let (_dir, core, host) = fixture();
    let mut base = BTreeSet::from([("kept".into(), "zzzz".into())]);
    user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
    let mut entries = vec![remote("kept", "zzzz", 1)];
    let mut initial_entries = entries.clone();
    let words = &["a", "b", "c", "d", "e", "f", "g", "h"][..4 + extra];
    for (index, word) in words.iter().enumerate() {
        base.insert(((*word).into(), (*word).into()));
        if delete_on_server {
            entries.push(remote(word, word, index as i64 + 2));
        }
        if !delete_on_server || index >= 4 {
            user_words::add_user_word(&core, "keytao", &host.0, word, word, None).unwrap();
        }
        if delete_on_server || index >= 4 {
            initial_entries.push(remote(word, word, index as i64 + 2));
        }
    }
    save_base(&host.0, 7, &base).unwrap();
    let initial = server(vec![(
        "200 OK",
        json!({"total":initial_entries.len(),"entries":initial_entries}),
    )])
    .await;
    let pending = sync_user_dictionary_at(&core, &host, "fixture-token".into(), &initial.url, None)
        .await
        .unwrap();
    assert_eq!(initial.finish().await.len(), 1);
    assert!(pending.pending);
    assert_eq!(
        pending.local_deletions.len() + pending.server_deletions.len(),
        4
    );
    if delete_on_server {
        for entry in user_words::list_user_words(&core, "keytao", &host.0)
            .unwrap()
            .into_iter()
            .rev()
            .filter(|entry| entry.word != "kept")
        {
            user_words::delete_user_word(
                &core,
                "keytao",
                &host.0,
                entry.id,
                &entry.word,
                &entry.code,
            )
            .unwrap();
        }
    }
    let base_before = std::fs::read(base_path(&host.0, 7)).unwrap();
    let dictionary_before = std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap();
    let mut responses = vec![("200 OK", json!({"total":entries.len(),"entries":entries}))];
    responses.extend((0..8).map(|_| ("200 OK", json!({"success":true}))));
    let mock = server(responses).await;
    let result = sync_user_dictionary_at(
        &core,
        &host,
        "fixture-token".into(),
        &mock.url,
        Some(UserDictionaryDeletionConfirmation {
            local_deletions: pending.local_deletions,
            server_deletions: pending.server_deletions,
        }),
    )
    .await
    .unwrap();
    let requests = mock.finish().await;
    if extra <= 3 {
        assert!(!result.pending);
        assert_eq!(requests.len(), if delete_on_server { 5 } else { 1 });
        let mut expected = BTreeSet::from([("kept".into(), "zzzz".into())]);
        expected.extend(
            words[4..]
                .iter()
                .map(|word| ((*word).into(), (*word).into())),
        );
        let saved: DictionaryBase =
            serde_json::from_slice(&std::fs::read(base_path(&host.0, 7)).unwrap()).unwrap();
        assert_eq!(
            saved.entries, expected,
            "deferred deletions must remain in the base"
        );
        if delete_on_server {
            assert_eq!(local(&core, &host), [("kept".into(), "zzzz".into())]);
            for (request, id) in requests.iter().skip(1).zip(2..=5) {
                assert!(request.starts_with(&format!("DELETE /api/user/dictionary/{id} HTTP/1.1")));
            }
        } else {
            assert_eq!(
                local(&core, &host),
                expected.into_iter().collect::<Vec<_>>()
            );
        }
        return;
    }
    assert!(
        result.pending,
        "new deletions must require confirmation again"
    );
    assert_eq!(requests.len(), 1, "pending sync must not send DELETE");
    assert_eq!(std::fs::read(base_path(&host.0, 7)).unwrap(), base_before);
    assert_eq!(
        std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap(),
        dictionary_before
    );
    let deletions = if delete_on_server {
        result.server_deletions
    } else {
        result.local_deletions
    };
    assert_eq!(
        deletions
            .into_iter()
            .map(|entry| (entry.word, entry.code))
            .collect::<BTreeSet<_>>(),
        ["a", "b", "c", "d", "e", "f", "g", "h"]
            .map(|word| (word.into(), word.into()))
            .into()
    );
}

#[tokio::test]
async fn confirmed_deletions_recheck_combined_excess_before_deleting() {
    let (_dir, core, host) = fixture();
    let mut base = BTreeSet::from([("kept".into(), "zzzz".into())]);
    user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
    for word in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        base.insert((word.into(), word.into()));
        if !matches!(word, "g" | "h") {
            user_words::add_user_word(&core, "keytao", &host.0, word, word, None).unwrap();
        }
    }
    save_base(&host.0, 7, &base).unwrap();
    let before = std::fs::read(base_path(&host.0, 7)).unwrap();
    let dictionary = std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap();
    let mock = server(vec![(
        "200 OK",
        json!({"total":3,"entries":[remote("kept","zzzz",1),remote("g","g",8),remote("h","h",9)]}),
    )])
    .await;
    let result = sync_user_dictionary_at(
        &core,
        &host,
        "fixture-token".into(),
        &mock.url,
        Some(UserDictionaryDeletionConfirmation {
            local_deletions: ["a", "b", "c", "d"]
                .map(|word| UserDictionaryDeletion {
                    word: word.into(),
                    code: word.into(),
                })
                .into(),
            server_deletions: Vec::new(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(mock.finish().await.len(), 1);
    assert!(
        result.pending,
        "four new deletions across both sides require confirmation"
    );
    assert_eq!(result.local_deletions.len(), 6);
    assert_eq!(result.server_deletions.len(), 2);
    assert_eq!(std::fs::read(base_path(&host.0, 7)).unwrap(), before);
    assert_eq!(
        std::fs::read(host.0.join("keytao.user.dict.yaml")).unwrap(),
        dictionary
    );
}

#[tokio::test]
async fn confirmed_deletions_match_both_code_and_side() {
    for delete_on_server in [false, true] {
        let (_dir, core, host) = fixture();
        user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
        let mut entries = vec![remote("kept", "zzzz", 1)];
        for (id, code) in [(2, "old"), (3, "new")] {
            if delete_on_server {
                entries.push(remote("same", code, id));
            } else {
                user_words::add_user_word(&core, "keytao", &host.0, "same", code, None).unwrap();
            }
        }
        save_base(
            &host.0,
            7,
            &BTreeSet::from([
                ("kept".into(), "zzzz".into()),
                ("same".into(), "old".into()),
                ("same".into(), "new".into()),
            ]),
        )
        .unwrap();
        let confirmed = vec![UserDictionaryDeletion {
            word: "same".into(),
            code: "old".into(),
        }];
        let wrong_side = vec![UserDictionaryDeletion {
            word: "same".into(),
            code: "new".into(),
        }];
        let (local_deletions, server_deletions) = if delete_on_server {
            (wrong_side, confirmed)
        } else {
            (confirmed, wrong_side)
        };
        let mock = server(vec![
            ("200 OK", json!({"total":entries.len(),"entries":entries})),
            ("200 OK", json!({"success":true})),
        ])
        .await;
        let result = sync_user_dictionary_at(
            &core,
            &host,
            "fixture-token".into(),
            &mock.url,
            Some(UserDictionaryDeletionConfirmation {
                local_deletions,
                server_deletions,
            }),
        )
        .await
        .unwrap();
        let requests = mock.finish().await;
        assert!(!result.pending);
        assert_eq!(requests.len(), if delete_on_server { 2 } else { 1 });
        if delete_on_server {
            assert!(requests[1].starts_with("DELETE /api/user/dictionary/2 HTTP/1.1"));
        } else {
            assert_eq!(
                local(&core, &host),
                [
                    ("kept".into(), "zzzz".into()),
                    ("same".into(), "new".into()),
                ]
            );
        }
        let saved: DictionaryBase =
            serde_json::from_slice(&std::fs::read(base_path(&host.0, 7)).unwrap()).unwrap();
        assert_eq!(
            saved.entries,
            BTreeSet::from([
                ("kept".into(), "zzzz".into()),
                ("same".into(), "new".into()),
            ])
        );
    }
}

#[tokio::test]
async fn bulk_deletions_matching_confirmation_apply_to_both_sides_and_update_base() {
    for delete_on_server in [false, true] {
        let (_dir, core, host) = fixture();
        let mut base = BTreeSet::from([("kept".into(), "zzzz".into())]);
        user_words::add_user_word(&core, "keytao", &host.0, "kept", "zzzz", None).unwrap();
        let mut entries = vec![remote("kept", "zzzz", 1)];
        for (id, word, code) in [
            (2, "A", "aaaa"),
            (3, "B", "bbbb"),
            (4, "C", "cccc"),
            (5, "D", "dddd"),
        ] {
            base.insert((word.into(), code.into()));
            if delete_on_server {
                entries.push(remote(word, code, id));
            } else {
                user_words::add_user_word(&core, "keytao", &host.0, word, code, None).unwrap();
            }
        }
        save_base(&host.0, 7, &base).unwrap();
        let initial = server(vec![(
            "200 OK",
            json!({"total":entries.len(),"entries":entries}),
        )])
        .await;
        let pending =
            sync_user_dictionary_at(&core, &host, "fixture-token".into(), &initial.url, None)
                .await
                .unwrap();
        assert!(pending.pending);
        assert_eq!(initial.finish().await.len(), 1);
        let mut responses = vec![("200 OK", json!({"total":entries.len(),"entries":entries}))];
        responses.extend((0..4).map(|_| ("200 OK", json!({"success":true}))));
        let mock = server(responses).await;
        let result = sync_user_dictionary_at(
            &core,
            &host,
            "fixture-token".into(),
            &mock.url,
            Some(UserDictionaryDeletionConfirmation {
                local_deletions: pending.local_deletions,
                server_deletions: pending.server_deletions,
            }),
        )
        .await
        .unwrap();
        let requests = mock.finish().await;
        assert!(!result.pending);
        assert_eq!(local(&core, &host), [("kept".into(), "zzzz".into())]);
        assert_eq!(requests.len(), if delete_on_server { 5 } else { 1 });
        assert!(
            requests
                .iter()
                .skip(1)
                .all(|request| request.starts_with("DELETE /api/user/dictionary/"))
        );
        let saved: DictionaryBase =
            serde_json::from_slice(&std::fs::read(base_path(&host.0, 7)).unwrap()).unwrap();
        assert_eq!(
            saved.entries,
            BTreeSet::from([("kept".into(), "zzzz".into())])
        );
    }
}

#[tokio::test]
async fn legacy_base_migrates_once_only_for_an_existing_dictionary_instance() {
    for recreated in [false, true] {
        let (_dir, core, host) = fixture();
        user_words::add_user_word(&core, "keytao", &host.0, "X", "aaaa", None).unwrap();
        let legacy = legacy_base_path(&core, 7);
        std::fs::write(&legacy, br#"[["X","aaaa"],["Y","bbbb"],["Z","cccc"]]"#).unwrap();
        if recreated {
            let dictionary = host.0.join("keytao.user.dict.yaml");
            std::fs::remove_file(&dictionary).unwrap();
            std::fs::write(&dictionary, "---\nname: keytao.user\n...\nX\taaaa\n").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&legacy)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH),
                )
                .unwrap();
        }
        let mock = server(vec![
            ("200 OK", json!({"entries":[remote("X","aaaa",12),remote("Y","bbbb",13),remote("Z","cccc",14)],"total":3})),
            ("200 OK", json!({"success":true})),
            ("200 OK", json!({"success":true})),
        ]).await;
        let result = sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None)
            .await
            .unwrap();
        let requests = mock.finish().await;
        assert_eq!(requests.len(), if recreated { 1 } else { 3 });
        assert_eq!(result.count, if recreated { 3 } else { 1 });
        assert!(!legacy.try_exists().unwrap());
        assert!(base_path(&host.0, 7).is_file());
        std::fs::remove_dir_all(&host.0).unwrap();
        std::fs::create_dir_all(&host.0).unwrap();
        user_words::add_user_word(&core, "keytao", &host.0, "X", "aaaa", None).unwrap();
        let mock = server(vec![("200 OK", json!({"entries":[remote("X","aaaa",12),remote("Y","bbbb",13),remote("Z","cccc",14)],"total":3}))]).await;
        sync_user_dictionary_at(&core, &host, "fixture-token".into(), &mock.url, None)
            .await
            .unwrap();
        assert_eq!(
            mock.finish().await.len(),
            1,
            "legacy base must never return after migration"
        );
        assert_eq!(local(&core, &host).len(), 3);
    }
}

#[tokio::test]
async fn refreshed_token_for_another_account_is_cleared_without_retrying_the_action() {
    let (_dir, core, _host) = fixture();
    core.update_state(|state| state.auth_token = Some(jwt(now() - 172800, now() + 432000)))
        .unwrap();
    let mock = server(vec![
        ("401 Unauthorized", json!({"error":"fixture"})),
        ("200 OK", json!({"token":jwt_for(99,now(),now()+604800)})),
        ("200 OK", json!({"success":true})),
    ])
    .await;
    let result = ApiClient::new(&core, &mock.url)
        .request(reqwest::Method::DELETE, "/api/user/dictionary/12", None)
        .await;
    let requests = mock.finish().await;
    assert_eq!(
        requests.len(),
        2,
        "wrong-account refresh must not retry the action"
    );
    assert!(result.is_err());
    assert!(core.state().auth_token.is_none());
    assert!(core.state().user.is_none());
}
