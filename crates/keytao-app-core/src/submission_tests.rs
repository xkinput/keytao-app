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
    let core = Core::new(
        AppEnv {
            data_dir: path.into(),
            cache_dir: path.join("cache"),
            resource_dir: path.join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(NullSink),
    )
    .unwrap();
    core.import_legacy_state(Some("stored-token".into()), Some(json!({"id":7})), true)
        .unwrap();
    core
}

async fn server(responses: Vec<(&str, Value)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let responses: Vec<_> = responses
        .into_iter()
        .map(|(status, body)| (status.to_string(), body.to_string()))
        .collect();
    let handle = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let request = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers.lines().find_map(|line| line.to_ascii_lowercase()
                            .strip_prefix("content-length: ").map(|s| s.parse::<usize>().unwrap())).unwrap_or(0);
                        if bytes.len() >= end + 4 + length { break; }
                    }
                }
                stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                String::from_utf8(bytes).unwrap()
            }).await.expect("local HTTP fixture timed out");
            requests.push(request);
        }
        requests
    });
    (url, handle)
}

fn batches_empty() -> Value {
    json!({"batches":[],"pagination":{"page":1,"pageSize":100,"total":0,"totalPages":0}})
}

fn phrases_empty() -> Value {
    json!({"phrases":[],"pagination":{"page":1,"pageSize":6,"total":0,"totalPages":0}})
}

fn word(word: &str, code: &str) -> SubmissionWord {
    SubmissionWord {
        word: word.into(),
        code: code.into(),
        remark: None,
    }
}

fn payload(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn submit_follows_server_payload_and_warning_snapshot_contract() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let warning =
        json!({"id":"0","word":"existing","code":"abcd","weight":2,"impact":"fixture warning"});
    let mut submit_warning = warning.clone();
    submit_warning["id"] = json!("91");
    let digest = "a".repeat(64);
    let (url, handle) = server(vec![
        ("200 OK", batches_empty()),
        ("200 OK", phrases_empty()),
        ("200 OK", json!({"results":[{"id":"0","conflict":{"hasConflict":false}}],"warnings":[warning.clone()]})),
        ("200 OK", batches_empty()),
        ("200 OK", json!({"batch":{"id":"batch-1","status":"Draft","contentVersion":0},"contentVersion":1,"pullRequests":[{"id":91}]})),
        ("400 Bad Request", json!({"error":"confirm","requiresConfirmation":true,"batchId":"batch-1","contentVersion":1,"warningDigest":digest,"warnings":[submit_warning]})),
        ("200 OK", json!({"batch":{"id":"batch-1","status":"Submitted","contentVersion":2}})),
    ]).await;
    let mut entry = word("new word", "abcd");
    entry.remark = Some("fixture remark".into());
    let preview = prepare_submission_at(&core, vec![entry.clone()], &url)
        .await
        .unwrap();
    assert_eq!(preview.warnings, vec![warning]);
    let result = submit_at(&core, preview, &url).await.unwrap();
    assert_eq!(result.status, "Submitted");
    assert_eq!(result.items, vec![entry]);
    assert!(result.confirmation.is_none());
    let requests = handle.await.unwrap();
    let paths: Vec<_> = requests
        .iter()
        .map(|request| request.lines().next().unwrap())
        .collect();
    assert_eq!(
        paths,
        vec![
            "GET /api/batches?onlyMine=true&page=1&pageSize=100 HTTP/1.1",
            "GET /api/phrases/by-word?word=new+word&page=1 HTTP/1.1",
            "POST /api/pull-requests/check-conflicts-batch HTTP/1.1",
            "GET /api/batches?onlyMine=true&page=1&pageSize=100 HTTP/1.1",
            "POST /api/pull-requests/batch HTTP/1.1",
            "POST /api/batches/batch-1/submit HTTP/1.1",
            "POST /api/batches/batch-1/submit HTTP/1.1",
        ]
    );
    assert_eq!(
        payload(&requests[2]),
        json!({"items":[{"id":"0","action":"Create","word":"new word","code":"abcd","type":"Phrase"}]})
    );
    assert_eq!(
        payload(&requests[4]),
        json!({"items":[{"action":"Create","word":"new word","code":"abcd","type":"Phrase","remark":"fixture remark"}]})
    );
    assert_eq!(
        payload(&requests[5]),
        json!({"confirmed":false,"expectedContentVersion":1})
    );
    assert_eq!(
        payload(&requests[6]),
        json!({"confirmed":true,"expectedContentVersion":1,"expectedWarningDigest":digest})
    );
    assert!(requests.iter().all(|request| {
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer stored-token\r\n")
    }));
}

#[tokio::test]
async fn already_published_word_on_second_public_page_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let (url, handle) = server(vec![
        ("200 OK", batches_empty()),
        ("200 OK", json!({"phrases":[
            {"word":"published","code":"aaaa"},{"word":"published","code":"aaab"},
            {"word":"published","code":"aaac"},{"word":"published","code":"aaad"},
            {"word":"published","code":"aaae"},{"word":"published","code":"aaaf"}
        ],"pagination":{"page":1,"pageSize":6,"total":7,"totalPages":2}})),
        ("200 OK", json!({"phrases":[{"word":"published","code":"bbbb"}],"pagination":{"page":2,"pageSize":6,"total":7,"totalPages":2}})),
    ]).await;
    let entry = word("published", "bbbb");
    let preview = prepare_submission_at(&core, vec![entry.clone()], &url)
        .await
        .unwrap();
    assert!(preview.items.is_empty());
    assert_eq!(preview.already_published, vec![entry]);
    let requests = handle.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].starts_with("GET /api/phrases/by-word?word=published&page=2 HTTP/1.1"));
}

#[tokio::test]
async fn own_pending_word_beyond_three_item_batch_preview_is_not_resubmitted() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let first_three = json!([
        {"word":"one","code":"aaaa","action":"Create","status":"Pending"},
        {"word":"two","code":"aaab","action":"Create","status":"Pending"},
        {"word":"three","code":"aaac","action":"Create","status":"Pending"},
    ]);
    let mut all_prs = first_three.as_array().unwrap().clone();
    all_prs.push(json!({"word":"pending","code":"bbbb","action":"Create","status":"Pending"}));
    let (url, handle) = server(vec![
        ("200 OK", json!({"batches":[{"id":"batch-1","status":"Submitted","pullRequests":first_three,"_count":{"pullRequests":4}}],"pagination":{"page":1,"pageSize":100,"total":101,"totalPages":2}})),
        ("200 OK", json!({"batch":{"id":"batch-1","status":"Submitted","pullRequests":all_prs}})),
        ("200 OK", json!({"batches":[{"id":"batch-old","status":"Rejected","pullRequests":[],"_count":{"pullRequests":0}}],"pagination":{"page":2,"pageSize":100,"total":101,"totalPages":2}})),
    ]).await;
    let entry = word("pending", "bbbb");
    let preview = prepare_submission_at(&core, vec![entry.clone()], &url)
        .await
        .unwrap();
    assert!(preview.items.is_empty());
    assert_eq!(preview.already_pending, vec![entry]);
    let requests = handle.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].starts_with("GET /api/batches/batch-1 HTTP/1.1"));
    assert!(requests[2].starts_with("GET /api/batches?onlyMine=true&page=2&pageSize=100 HTTP/1.1"));
}

#[tokio::test]
async fn interrupted_submission_resumes_its_draft_without_showing_it_as_pending() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let conflicts = json!({"results":[{"id":"0","conflict":{"hasConflict":false}}],"warnings":[]});
    let (url, handle) = server(vec![
        ("200 OK", batches_empty()),
        ("200 OK", phrases_empty()),
        ("200 OK", conflicts.clone()),
        ("200 OK", batches_empty()),
        ("200 OK", json!({"batch":{"id":"recoverable","status":"Draft","contentVersion":0},"contentVersion":1})),
        ("429 Too Many Requests", json!({"error":"fixture limit"})),
        ("200 OK", json!({"batches":[{"id":"recoverable","status":"Draft","pullRequests":[{"word":"new","code":"bbbb","action":"Create","status":"Pending"}],"_count":{"pullRequests":1}}],"pagination":{"page":1,"pageSize":100,"total":1,"totalPages":1}})),
        ("200 OK", phrases_empty()),
        ("200 OK", conflicts),
        ("200 OK", json!({"batch":{"id":"recoverable","creatorId":7,"status":"Draft","contentVersion":1,"pullRequests":[{"word":"new","code":"bbbb","action":"Create","type":"Phrase","remark":null,"weight":1}]}})),
        ("200 OK", json!({"batch":{"id":"recoverable","status":"Submitted","contentVersion":2}})),
    ]).await;
    let entry = word("new", "bbbb");
    let preview = prepare_submission_at(&core, vec![entry.clone()], &url)
        .await
        .unwrap();
    assert!(submit_at(&core, preview, &url).await.is_err());
    let core = open_core(dir.path());
    let preview = prepare_submission_at(&core, vec![entry], &url)
        .await
        .unwrap();
    assert!(preview.already_pending.is_empty());
    assert_eq!(preview.batch_id.as_deref(), Some("recoverable"));
    let result = submit_at(&core, preview, &url).await.unwrap();
    assert_eq!(result.status, "Submitted");
    let requests = handle.await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /api/pull-requests/batch "))
            .count(),
        1
    );
    assert!(requests[9].starts_with("GET /api/batches/recoverable HTTP/1.1"));
    assert_eq!(
        payload(&requests[10]),
        json!({"confirmed":false,"expectedContentVersion":1})
    );
}
