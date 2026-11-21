use std::{fs::File, io};

/// Hold the exclusive lock until the file is closed.
pub fn lock_file(file: &File, wait: bool) -> io::Result<()> {
    #[cfg(target_os = "android")]
    match android_record_lock(file, wait) {
        // Shared storage can reject flock. Keep flock for older kernels without OFD locks.
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
            ) => {}
        result => return result,
    }

    if wait {
        file.lock()
    } else {
        file.try_lock().map_err(Into::into)
    }
}

#[cfg(target_os = "android")]
fn android_record_lock(file: &File, wait: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // Bionic's fcntl also expects the 64-bit record layout on 32-bit Android.
    let mut lock: libc::flock64 = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    let command = if wait {
        libc::F_OFD_SETLKW
    } else {
        libc::F_OFD_SETLK
    };
    loop {
        let result =
            unsafe { libc::fcntl(file.as_raw_fd(), command, &lock as *const libc::flock64) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_lock_releases_when_the_file_closes() {
        let path = std::env::temp_dir().join(format!("keytao-file-lock-{}", std::process::id()));
        let first = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let second = File::options().write(true).open(&path).unwrap();

        lock_file(&first, true).unwrap();
        assert_eq!(
            lock_file(&second, false).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(first);
        lock_file(&second, false).unwrap();
        drop(second);
        std::fs::remove_file(path).unwrap();
    }
}
