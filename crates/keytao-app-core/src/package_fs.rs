use std::{fs, io, path::Path, time::Duration};

#[derive(Debug)]
struct PathError {
    message: String,
    error: io::Error,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}：{}", self.message, self.error)
    }
}

impl std::error::Error for PathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

pub(crate) fn context(operation: &str, path: &Path, error: io::Error) -> io::Error {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    io::Error::new(
        error.kind(),
        PathError {
            message: format!("{operation} {} 失败", path.display()),
            error,
        },
    )
}

pub(crate) fn windows_access_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(5 | 32 | 33))
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<PathError>())
            .is_some_and(|inner| windows_access_error(&inner.error))
}

fn retry_with<T>(
    mut action: impl FnMut() -> io::Result<T>,
    mut sleep: impl FnMut(Duration),
    windows: bool,
) -> io::Result<T> {
    let mut delays = [20, 40, 80, 160, 320, 640, 740].into_iter();
    loop {
        match action() {
            Err(error) if windows && windows_access_error(&error) => {
                if let Some(delay) = delays.next() {
                    sleep(Duration::from_millis(delay));
                } else {
                    return Err(error);
                }
            }
            result => return result,
        }
    }
}

pub(crate) fn retry_windows<T>(action: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_with(action, std::thread::sleep, cfg!(windows))
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

pub(crate) fn clear_readonly(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(context("读取文件属性", path, error)),
    };
    if is_link(&metadata) || !metadata.permissions().readonly() {
        return Ok(());
    }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).map_err(|error| context("清除只读属性", path, error))
}

fn clear_tree(path: &Path) -> io::Result<()> {
    clear_readonly(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(context("读取文件属性", path, error)),
    };
    if metadata.is_dir() && !is_link(&metadata) {
        for entry in fs::read_dir(path).map_err(|error| context("读取目录", path, error))? {
            let entry = entry.map_err(|error| context("读取目录项", path, error))?;
            clear_tree(&entry.path())?;
        }
    }
    Ok(())
}

pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    retry_windows(|| {
        clear_readonly(from)?;
        clear_readonly(to)?;
        fs::rename(from, to)
    })
    .map_err(|error| {
        let from = std::path::absolute(from).unwrap_or_else(|_| from.to_owned());
        context(&format!("重命名 {} →", from.display()), to, error)
    })
}

pub(crate) fn remove_file(path: &Path) -> io::Result<()> {
    retry_windows(|| {
        clear_readonly(path)?;
        fs::remove_file(path)
    })
    .map_err(|error| context("删除文件", path, error))
}

pub(crate) fn remove_dir_all(path: &Path) -> io::Result<()> {
    retry_windows(|| {
        clear_tree(path)?;
        fs::remove_dir_all(path)
    })
    .map_err(|error| context("删除目录", path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_retries_only_transient_errors_with_bounded_backoff() {
        for code in [5, 32, 33] {
            let mut attempts = 0;
            let mut waited = Duration::ZERO;
            let error = retry_with(
                || {
                    attempts += 1;
                    Err::<(), _>(io::Error::from_raw_os_error(code))
                },
                |delay| waited += delay,
                true,
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
            assert_eq!(attempts, 8);
            assert_eq!(waited, Duration::from_secs(2));
        }
        for windows in [false, true] {
            let mut attempts = 0;
            retry_with(
                || {
                    attempts += 1;
                    Err::<(), _>(io::Error::from(io::ErrorKind::NotFound))
                },
                |_| panic!("permanent errors must not sleep"),
                windows,
            )
            .unwrap_err();
            assert_eq!(attempts, 1);
        }
        let mut attempts = 0;
        retry_with(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(io::Error::from_raw_os_error(32))
                } else {
                    Ok(())
                }
            },
            |_| {},
            true,
        )
        .unwrap();
        assert_eq!(attempts, 3);

        let mut attempts = 0;
        retry_with(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(context(
                        "读取目录",
                        Path::new("owned"),
                        io::Error::from_raw_os_error(32),
                    ))
                } else {
                    Ok(())
                }
            },
            |_| {},
            true,
        )
        .unwrap();
        assert_eq!(attempts, 2);
    }

    #[test]
    fn owned_readonly_files_can_be_renamed_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        let file = source.join("readonly.yaml");
        fs::write(&file, b"scheme").unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();
        let renamed = source.join("renamed.yaml");
        rename(&file, &renamed).unwrap();
        assert!(!fs::metadata(&renamed).unwrap().permissions().readonly());
        let mut permissions = fs::metadata(&renamed).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&renamed, permissions).unwrap();
        remove_dir_all(&source).unwrap();
        assert!(!source.exists());
    }

    #[test]
    fn rename_error_includes_operation_and_both_paths() {
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("missing");
        let to = directory.path().join("destination");
        let error = rename(&from, &to).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let message = error.to_string();
        assert!(message.contains("重命名"));
        assert!(message.contains(&from.display().to_string()));
        assert!(message.contains(&to.display().to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_never_changes_symlink_target_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("outside");
        fs::write(&target, b"keep").unwrap();
        let mut permissions = fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&target, permissions).unwrap();
        let owned = directory.path().join("owned");
        fs::create_dir(&owned).unwrap();
        std::os::unix::fs::symlink(&target, owned.join("link")).unwrap();
        remove_dir_all(&owned).unwrap();
        assert!(fs::metadata(&target).unwrap().permissions().readonly());
        clear_readonly(&target).unwrap();
    }
}
