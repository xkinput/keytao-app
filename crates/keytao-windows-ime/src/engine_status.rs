use keytao_core::runtime_log::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT_STATUS_WRITE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs((5_u64 << failures.saturating_sub(1).min(4)).min(60))
}

pub(crate) fn write_status(
    user_dir: &Path,
    error: Option<&str>,
    schema: Option<&str>,
) -> io::Result<()> {
    fs::create_dir_all(user_dir)?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let pid = std::process::id();
    let status = json::json!({
        "ok": error.is_none(),
        "error": error,
        "schema": schema,
        "ts": ts,
        "pid": pid,
    });
    let sequence = NEXT_STATUS_WRITE.fetch_add(1, Ordering::Relaxed);
    let temporary = user_dir.join(format!(".keytao-ime-engine-{pid}-{ts}-{sequence}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(status.to_string().as_bytes())?;
        file.sync_all()?;
        drop(file);
        // The same-directory rename replaces the old status atomically on Windows.
        fs::rename(&temporary, user_dir.join("keytao-ime-engine.json"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_grows_and_stays_bounded() {
        let delays: Vec<_> = (1..=6).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(delays, [5, 10, 20, 40, 60, 60]);
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn completed_attempts_replace_status_and_escape_errors() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-engine-status-{}-{}",
            std::process::id(),
            NEXT_STATUS_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        write_status(&dir, None, Some("keytao")).unwrap();
        let read = || -> json::Value {
            json::from_slice(&fs::read(dir.join("keytao-ime-engine.json")).unwrap()).unwrap()
        };
        let ready = read();
        assert_eq!(ready["ok"], true);
        assert_eq!(ready["schema"], "keytao");
        assert!(ready["error"].is_null());
        assert_eq!(ready["pid"], std::process::id());
        assert!(ready["ts"].as_u64().unwrap() > 0);

        let error = "missing \"C:\\词库\\keytao\"\ninit timed out";
        write_status(&dir, Some(error), None).unwrap();
        let failed = read();
        assert_eq!(failed["ok"], false);
        assert_eq!(failed["error"], error);
        assert!(failed["schema"].is_null());

        write_status(&dir, None, Some("pinyin_simp")).unwrap();
        assert_eq!(read()["ok"], true);
        assert_eq!(read()["schema"], "pinyin_simp");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_replacement_cleans_up_temporary_file() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-engine-status-{}-{}",
            std::process::id(),
            NEXT_STATUS_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("keytao-ime-engine.json")).unwrap();
        assert!(write_status(&dir, Some("init failed"), None).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
