use crate::{Core, CoreError, CoreEvent, events::InstallProgress, scheme::build_client};
use futures_util::StreamExt;

pub async fn download_to_temp(core: &Core, url: String) -> Result<String, CoreError> {
    let result: Result<_, String> = async {
        let emit = |stage: &str, percent: u32, message: &str| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            }));
        };

        emit("downloading", 0, "正在下载...");

        let client = build_client(core)?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("下载失败: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("下载失败，HTTP {}", response.status()));
        }

        let total_size = response.content_length().unwrap_or(0);
        let mut downloaded = 0u64;
        let mut bytes: Vec<u8> = if total_size > 0 {
            Vec::with_capacity(total_size as usize)
        } else {
            Vec::new()
        };

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
            downloaded += chunk.len() as u64;
            bytes.extend_from_slice(&chunk);
            if total_size > 0 {
                let percent = (downloaded * 60 / total_size) as u32;
                emit(
                    "downloading",
                    percent,
                    &format!(
                        "正在下载... {:.1}MB / {:.1}MB",
                        downloaded as f64 / 1_048_576.0,
                        total_size as f64 / 1_048_576.0
                    ),
                );
            }
        }

        let cache_dir = core.env().cache_dir.clone();
        std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
        let temp_path = cache_dir.join("keytao_download.zip");
        std::fs::write(&temp_path, &bytes).map_err(|e| format!("保存临时文件失败: {e}"))?;

        emit("downloading", 60, "下载完成，准备解压...");
        Ok(temp_path.to_string_lossy().into_owned())
    }
    .await;
    result.map_err(CoreError::Other)
}
