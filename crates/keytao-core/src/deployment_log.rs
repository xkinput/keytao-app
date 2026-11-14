use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub(super) struct DeploymentLog {
    directory: PathBuf,
    offsets: HashMap<PathBuf, u64>,
}

fn log_files(directory: &Path, severity: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let marker = format!(".log.{severity}.");
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_string_lossy().contains(&marker)
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .map(|entry| entry.path())
        .collect()
}

impl DeploymentLog {
    pub(super) fn capture(directory: PathBuf) -> Self {
        let offsets = ["ERROR", "WARNING"]
            .into_iter()
            .flat_map(|severity| log_files(&directory, severity))
            .filter_map(|path| Some((path.clone(), fs::metadata(path).ok()?.len())))
            .collect();
        Self { directory, offsets }
    }

    fn messages(&self, severity: &str, level: char) -> Vec<String> {
        let mut recent: Vec<(String, String)> = Vec::new();
        for path in log_files(&self.directory, severity) {
            let Ok(mut file) = File::open(&path) else {
                continue;
            };
            let offset = self.offsets.get(&path).copied().unwrap_or_default();
            // A truncated log starts a new window; newly created files start at zero.
            let offset = if file.metadata().is_ok_and(|meta| meta.len() >= offset) {
                offset
            } else {
                0
            };
            if file.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                if !line.starts_with(level)
                    || !line.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
                {
                    continue;
                }
                let Some((prefix, message)) = line.split_once("] ") else {
                    continue;
                };
                let message = message.trim();
                if message.is_empty()
                    || recent
                        .iter()
                        .any(|(previous, text)| text == message && previous.as_str() >= prefix)
                {
                    continue;
                }
                recent.retain(|(_, text)| text != message);
                recent.push((prefix.to_owned(), message.to_owned()));
                // glog's date/time prefix orders entries across process/rotation files.
                recent.sort_by(|a, b| a.0.cmp(&b.0));
                if recent.len() > 8 {
                    recent.remove(0);
                }
            }
        }
        recent.into_iter().map(|(_, message)| message).collect()
    }

    fn summary(&self) -> String {
        let mut messages = self.messages("ERROR", 'E');
        if messages.is_empty() {
            messages = self.messages("WARNING", 'W');
        }
        let mut remaining = 800;
        let mut lines = Vec::new();
        for message in messages.into_iter().rev() {
            if remaining == 0 {
                break;
            }
            let line = if message.chars().count() > remaining {
                message.chars().take(remaining - 1).chain(['…']).collect()
            } else {
                message
            };
            remaining = remaining.saturating_sub(line.chars().count() + 1);
            lines.push(line);
        }
        lines.reverse();
        lines.join("\n")
    }

    pub(super) fn failure_message(&self, detail: Option<&str>) -> String {
        let mut message = String::from("Rime 部署失败：");
        if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
            message.push('\n');
            message.push_str(detail);
        }
        let summary = self.summary();
        if !summary.is_empty() {
            message.push('\n');
            message.push_str(&summary);
        }
        message.push_str(&format!("\n日志目录：{}", self.directory.display()));
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempLog(PathBuf);

    impl TempLog {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "keytao-deploy-log-{}-{suffix}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }

        fn append(&self, name: &str, contents: &str) {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.0.join(name))
                .unwrap();
            file.write_all(contents.as_bytes()).unwrap();
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_new_error_messages_are_stripped_and_deduplicated() {
        let logs = TempLog::new();
        let name = "rime.keytao.host.user.log.ERROR.20261115-020000.123.log";
        logs.append(
            name,
            concat!(
                "Log file created at: 2026/11/15 02:00:00\nRunning on machine: host\n",
                "E20261115 02:00:00.000001 0x16feeb000 old.cc:1] old deployment error\n",
            ),
        );
        let window = DeploymentLog::capture(logs.0.clone());
        logs.append(name, concat!(
            "E20261115 02:01:16.877607 0x16feeb000 dict_compiler.cc:55] source file '/tmp/xkjd6.user.dict.yaml' does not exist.\n",
            "E20261115 02:01:16.877912 0x16feeb000 deployment_tasks.cc:376] dictionary 'xkjd6.extended' failed to compile.\n",
            "E20261115 02:01:16.877913 0x16feeb000 deployment_tasks.cc:376] dictionary 'xkjd6.extended' failed to compile.\n",
        ));
        assert_eq!(
            window.summary(),
            concat!(
                "source file '/tmp/xkjd6.user.dict.yaml' does not exist.\n",
                "dictionary 'xkjd6.extended' failed to compile.",
            )
        );
        let message = window.failure_message(Some("validation detail"));
        assert!(message.starts_with("Rime 部署失败：\nvalidation detail\nsource file"));
        assert!(message.ends_with(&format!("\n日志目录：{}", logs.0.display())));
    }

    #[test]
    fn new_windows_logs_need_no_symlinks_and_headers_are_ignored() {
        let logs = TempLog::new();
        let window = DeploymentLog::capture(logs.0.clone());
        logs.append(
            "rime.keytao.HOST.user.log.ERROR.20261115-020100.42.log",
            concat!(
                "Log file created at: 2026/11/15 02:01:00\r\nRunning on machine: HOST\r\n",
                "E20261115 02:01:00.000001 123 dict_compiler.cc:55] missing dictionary\r\n",
            ),
        );
        logs.append(
            "rime.keytao.HOST.user.log.ERROR.20261115-020000.41.log",
            "E20261115 02:00:59.000001 456 config.cc:1] invalid config\n",
        );
        logs.append(
            "rime.keytao.ERROR",
            "E20261115 02:02:00.000001 123 a.cc:1] ignore alias\n",
        );
        assert_eq!(window.summary(), "invalid config\nmissing dictionary");
    }

    #[test]
    fn warnings_use_the_same_window_and_errors_take_priority() {
        let logs = TempLog::new();
        let warning = "rime.keytao.log.WARNING.20261115-020000.42.log";
        logs.append(
            warning,
            "W20261115 02:00:00.000001 123 a.cc:1] old warning\n",
        );
        let window = DeploymentLog::capture(logs.0.clone());
        logs.append(
            warning,
            "W20261115 02:01:00.000001 123 a.cc:1] new warning\n",
        );
        assert_eq!(window.summary(), "new warning");
        logs.append(
            "rime.keytao.log.ERROR.20261115-020100.42.log",
            "E20261115 02:01:01.000001 123 a.cc:1] new error\n",
        );
        assert_eq!(window.summary(), "new error");
    }

    #[test]
    fn summary_keeps_last_eight_unique_lines_and_caps_unicode_characters() {
        let logs = TempLog::new();
        let window = DeploymentLog::capture(logs.0.clone());
        let name = "rime.keytao.log.ERROR.20261115-020000.42.log";
        for index in 0..12 {
            logs.append(
                name,
                &format!("E20261115 02:01:{index:02}.000001 123 a.cc:1] error {index}\n"),
            );
        }
        let summary = window.summary();
        assert_eq!(summary.lines().count(), 8);
        assert_eq!(summary.lines().next(), Some("error 4"));
        assert_eq!(summary.lines().last(), Some("error 11"));
        logs.append(
            name,
            &format!(
                "E20261115 02:02:00.000001 123 a.cc:1] {}\n",
                "错".repeat(900)
            ),
        );
        let summary = window.summary();
        assert_eq!(summary.chars().count(), 800);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn missing_logs_preserve_generic_failure_and_truncation_reads_from_zero() {
        let logs = TempLog::new();
        let window = DeploymentLog::capture(logs.0.join("missing"));
        assert_eq!(
            window.failure_message(None),
            format!(
                "Rime 部署失败：\n日志目录：{}",
                logs.0.join("missing").display()
            )
        );
        let name = "rime.keytao.log.ERROR.20261115-020000.42.log";
        logs.append(name, &"header".repeat(100));
        let window = DeploymentLog::capture(logs.0.clone());
        fs::write(
            logs.0.join(name),
            "E20261115 02:01:00.000001 123 a.cc:1] truncated log error\n",
        )
        .unwrap();
        assert_eq!(window.summary(), "truncated log error");
    }
}
