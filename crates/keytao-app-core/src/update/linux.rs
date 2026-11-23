use std::{
    fs::{self, File},
    io::Seek,
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
};

const PKEXEC: &str = "/usr/bin/pkexec";
const INSTALLER: &str = "/usr/lib/KeyTao/install-update";

pub(super) fn package_suffix(arch: &str) -> Option<&'static str> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    if executable != Path::new("/usr/lib/KeyTao/keytao-app") {
        return None;
    }
    if Command::new("/usr/bin/dpkg-query")
        .arg("--search")
        .arg(&executable)
        .output()
        .is_ok_and(|result| result.status.success() && result.stdout.starts_with(b"key-tao:"))
    {
        return Some(if arch == "x86_64" {
            "linux-x64.deb"
        } else {
            "linux-arm64.deb"
        });
    }
    if Command::new("/usr/bin/rpm")
        .args(["-qf", "--queryformat", "%{NAME}"])
        .arg(executable)
        .output()
        .is_ok_and(|result| result.status.success() && result.stdout == b"key-tao")
    {
        return Some(if arch == "x86_64" {
            "linux-x64.rpm"
        } else {
            "linux-arm64.rpm"
        });
    }
    None
}

pub(super) fn can_install(suffix: &str) -> bool {
    Path::new(PKEXEC).is_file()
        && fs::symlink_metadata(INSTALLER).is_ok_and(|meta| {
            meta.is_file()
                && meta.uid() == 0
                && meta.mode() & 0o022 == 0
                && meta.mode() & 0o111 != 0
        })
        && (!suffix.ends_with(".deb") || Path::new("/usr/bin/apt-get").is_file())
}

pub(super) fn install_update(
    path: &Path,
    suffix: &str,
    digest: [u8; 32],
    mut file: File,
) -> Result<(), String> {
    let (_, format) = suffix.rsplit_once('.').ok_or("无法识别 Linux 安装包类型")?;
    if path.extension().and_then(|ext| ext.to_str()) != Some(format) || !can_install(suffix) {
        return Err("当前系统无法静默安装此升级包，请打开升级包手动安装".into());
    }

    // Pass the verified file descriptor, not a user-writable path, across authorization.
    file.rewind().map_err(|error| error.to_string())?;
    let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let output = Command::new(PKEXEC)
        .args(["--disable-internal-agent", INSTALLER, format])
        .arg(digest)
        .stdin(Stdio::from(file))
        .output()
        .map_err(|error| format!("无法启动 Linux 安装程序：{error}"))?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(126) => Err("已取消 Linux 安装授权".into()),
        Some(127) => Err("无法获得 Linux 安装授权，请打开升级包手动安装".into()),
        _ => {
            let details = String::from_utf8_lossy(&output.stderr);
            let message = details.lines().rfind(|line| !line.trim().is_empty());
            Err(match message {
                Some(message) => format!("安装升级包失败：{message}"),
                None => "安装升级包失败，请打开升级包手动安装".into(),
            })
        }
    }
}
