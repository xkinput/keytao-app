use crate::{
    Core, CoreError, CoreEvent, events::InstallProgress, generic_rime::DEFAULT_MAX_EXPANDED_BYTES,
    scheme::build_client,
};
use futures_util::StreamExt;
use std::{
    io::{Seek, Write},
    time::Duration,
};

pub const DOWNLOAD_CANCELLED: &str = "下载已取消";

pub(crate) fn emit_progress(core: &Core, stage: &str, percent: u32, message: &str) {
    core.emit(CoreEvent::InstallProgress(InstallProgress {
        stage: stage.to_string(),
        percent,
        message: message.to_string(),
    }));
}

pub async fn download_to_temp(core: &Core, url: String) -> Result<String, CoreError> {
    let mut cancel = core.download_cancel.subscribe();
    cancel.borrow_and_update();
    let result: Result<_, String> = async {
        let client = build_client(core)?;
        let mut temp = download_to_temp_with(
            core,
            client.get(&url),
            &mut cancel,
            None,
            Some(DEFAULT_MAX_EXPANDED_BYTES),
        )
        .await?;

        if url.starts_with("https://github.com/rimeinn/rime-snow-pinyin/releases/download/")
            && url.ends_with("/snow-pinyin.zip")
        {
            let bytes = crate::rime_snow::prepare_archive(
                core,
                std::fs::read(temp.path()).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            temp.as_file_mut().set_len(0).map_err(|e| e.to_string())?;
            temp.as_file_mut().rewind().map_err(|e| e.to_string())?;
            temp.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        let temp_path = core.env().cache_dir.join("keytao_download.zip");
        temp.persist(&temp_path)
            .map_err(|e| format!("保存临时文件失败: {e}"))?;

        emit_progress(core, "extracting", 60, "下载完成，准备解压...");
        Ok(temp_path.to_string_lossy().into_owned())
    }
    .await;
    result.map_err(CoreError::Other)
}

pub(crate) async fn download_to_temp_with(
    core: &Core,
    request: reqwest::RequestBuilder,
    cancel: &mut tokio::sync::watch::Receiver<u64>,
    expected_size: Option<u64>,
    max_size: Option<u64>,
) -> Result<tempfile::NamedTempFile, String> {
    emit_progress(core, "downloading", 0, "正在下载...");
    download_with_progress(
        core,
        request,
        cancel,
        expected_size,
        max_size,
        |downloaded, total_size| {
            if total_size > 0 {
                let percent = (downloaded.saturating_mul(60) / total_size).min(60) as u32;
                emit_progress(
                    core,
                    "downloading",
                    percent,
                    &format!(
                        "正在下载... {:.1}MB / {:.1}MB",
                        downloaded as f64 / 1_048_576.0,
                        total_size as f64 / 1_048_576.0
                    ),
                );
            }
        },
    )
    .await
}

pub(crate) async fn download_with_progress(
    core: &Core,
    request: reqwest::RequestBuilder,
    cancel: &mut tokio::sync::watch::Receiver<u64>,
    expected_size: Option<u64>,
    max_size: Option<u64>,
    mut progress: impl FnMut(u64, u64),
) -> Result<tempfile::NamedTempFile, String> {
    let response = tokio::select! {
        biased;
        _ = cancel.changed() => return Err(DOWNLOAD_CANCELLED.to_string()),
        response = send_get_with_retry(request) => response.map_err(|e| format!("下载失败: {}", e.without_url()))?,
    };
    if !response.status().is_success() {
        return Err(format!("下载失败，HTTP {}", response.status()));
    }

    let total_size = expected_size
        .or_else(|| response.content_length())
        .unwrap_or(0);
    if max_size.is_some_and(|limit| total_size > limit) {
        return Err("下载文件超过大小限制".into());
    }
    let mut downloaded = 0u64;
    let cache_dir = &core.env().cache_dir;
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let mut temp =
        tempfile::NamedTempFile::new_in(cache_dir).map_err(|e| format!("保存临时文件失败: {e}"))?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::select! {
        biased;
        _ = cancel.changed() => return Err(DOWNLOAD_CANCELLED.to_string()),
        chunk = tokio::time::timeout(Duration::from_secs(30), stream.next()) =>
            chunk.map_err(|_| "下载超时：30 秒内未收到数据".to_string())?,
    } {
        let chunk = chunk.map_err(|e| format!("下载中断: {}", e.without_url()))?;
        downloaded += chunk.len() as u64;
        if max_size.is_some_and(|limit| downloaded > limit) {
            return Err("下载文件超过大小限制".into());
        }
        if expected_size.is_some_and(|size| downloaded > size) {
            return Err("下载大小与发布记录不符".into());
        }
        temp.write_all(&chunk)
            .map_err(|e| format!("保存临时文件失败: {e}"))?;
        progress(downloaded, total_size);
    }
    if expected_size.is_some_and(|size| downloaded != size) {
        return Err("下载大小与发布记录不符".into());
    }
    Ok(temp)
}

pub(crate) async fn send_get_with_retry(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    let retry = request.try_clone();
    let result = request.send().await;
    if let (Some(retry), Err(error)) = (retry, &result)
        && !error.is_redirect()
        && (error.is_connect() || error.is_timeout() || error.is_request())
    {
        tokio::time::sleep(Duration::from_millis(300)).await;
        return retry.send().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, EventSink, Platform};
    use std::{path::Path, sync::Arc};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
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

    async fn read_request_headers(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0, "request ended prematurely");
            request.extend_from_slice(&buffer[..count]);
        }
    }

    #[tokio::test]
    async fn cancel_download_interrupts_a_stalled_response() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let download = tokio::spawn({
            let core = core.clone();
            async move { download_to_temp(&core, url).await }
        });
        let (_connection, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("local server was not connected")
            .unwrap();

        core.cancel_download();
        let error = tokio::time::timeout(Duration::from_secs(1), download)
            .await
            .expect("download cancellation timed out")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), DOWNLOAD_CANCELLED);
    }

    #[tokio::test]
    async fn cancel_before_download_does_not_cancel_the_next_download() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body: Vec<u8> = (0..3 * 65_536 + 31).map(|i| (i % 251) as u8).collect();
        let expected = body.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body).await.unwrap();
        });

        core.cancel_download();
        let path = tokio::time::timeout(Duration::from_secs(5), download_to_temp(&core, url))
            .await
            .expect("local download timed out")
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), expected);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn scheme_download_rejects_an_oversized_archive_before_writing_to_cache() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        DEFAULT_MAX_EXPANDED_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let error = tokio::time::timeout(Duration::from_secs(5), download_to_temp(&core, url))
            .await
            .expect("local download timed out")
            .unwrap_err();
        assert_eq!(error.to_string(), "下载文件超过大小限制");
        assert!(!core.env().cache_dir.exists());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn interrupted_download_keeps_existing_archive_and_removes_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let core = open_core(dir.path());
        std::fs::create_dir_all(core.env().cache_dir.clone()).unwrap();
        let archive = core.env().cache_dir.join("keytao_download.zip");
        std::fs::write(&archive, b"previous archive").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial",
                )
                .await
                .unwrap();
        });

        let error = tokio::time::timeout(Duration::from_secs(5), download_to_temp(&core, url))
            .await
            .expect("local download timed out")
            .unwrap_err();
        assert!(error.to_string().contains("下载中断"), "{error}");
        assert_eq!(std::fs::read(&archive).unwrap(), b"previous archive");
        assert_eq!(
            std::fs::read_dir(core.env().cache_dir.clone())
                .unwrap()
                .count(),
            1
        );
        server.await.unwrap();
    }
}
