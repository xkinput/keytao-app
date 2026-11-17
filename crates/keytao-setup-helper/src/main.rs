#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf};

#[cfg(windows)]
mod windows;

type Result<T> = std::result::Result<T, String>;

struct Options {
    install_dir: PathBuf,
    report_path: PathBuf,
    stop: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut install_dir = None;
        let mut report_path = None;
        let mut stop = false;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--install-dir") => install_dir = args.next().map(PathBuf::from),
                Some("--report-path") => report_path = args.next().map(PathBuf::from),
                Some("--stop-owned-processes") => stop = true,
                _ => return Err("Invalid setup helper argument.".into()),
            }
        }
        Ok(Self {
            install_dir: install_dir.ok_or("InstallDir must be an absolute directory path.")?,
            report_path: report_path
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or("ReportPath is required.")?,
            stop,
        })
    }
}

struct OwnedProcess {
    pid: u32,
    name: String,
    image_path: String,
    session: u32,
    created: u64,
}

fn owned_executable_paths(normalized_directory: &str) -> [String; 2] {
    let root = normalized_directory.trim_end_matches('\\');
    [
        format!("{root}\\keytao-app.exe"),
        format!("{root}\\keytao.exe"),
    ]
}

fn owned_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("keytao-app.exe") || name.eq_ignore_ascii_case("keytao.exe")
}

fn same_path(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
        let left: Vec<_> = left.encode_utf16().collect();
        let right: Vec<_> = right.encode_utf16().collect();
        unsafe {
            CompareStringOrdinal(
                left.as_ptr(),
                left.len() as i32,
                right.as_ptr(),
                right.len() as i32,
                1,
            ) == CSTR_EQUAL
        }
    }
    #[cfg(not(windows))]
    {
        fn uppercase(c: char) -> char {
            let mut upper = c.to_uppercase();
            let first = upper.next().unwrap();
            if upper.next().is_none() {
                first
            } else {
                c
            }
        }
        left.chars().map(uppercase).eq(right.chars().map(uppercase))
    }
}

fn owned_image(allowed: &[String; 2], image: &str) -> bool {
    allowed.iter().any(|path| same_path(path, image))
}

fn same_process(target: &OwnedProcess, image: &str, created: u64) -> bool {
    target.created == created && same_path(&target.image_path, image)
}

fn report(
    allowed: &[String; 2],
    applications: &[OwnedProcess],
    stopped: &[String],
    stop: bool,
) -> (i32, String, String) {
    let mut details = format!("允许停止的可执行文件：\r\n{}", allowed.join("\r\n"));
    if !stopped.is_empty() {
        details.push_str(&format!("\r\n\r\n已处理：\r\n{}", stopped.join("\r\n")));
    }
    if applications.is_empty() {
        return (
            0,
            "KeyTao 自身进程已就绪，可以继续安装。其他输入法、浏览器、资源管理器及其他应用会保留。"
                .into(),
            details,
        );
    }
    let mut sorted: Vec<_> = applications.iter().collect();
    sorted.sort_by_key(|p| (p.session, p.pid));
    let lines: Vec<_> = sorted
        .iter()
        .map(|p| format!("{}（PID {}，会话 {}）", p.name, p.pid, p.session))
        .collect();
    let heading = "检测到以下 KeyTao 自身进程：\r\n";
    let ending = if stop {
        "\r\n以上 KeyTao 进程仍在运行，请重试。其他输入法、浏览器、资源管理器及其他应用不会被关闭。"
    } else {
        "\r\n继续安装时将自动强制停止这些 KeyTao 进程。其他输入法、浏览器、资源管理器及其他应用不会被关闭。"
    };
    let mut visible = Vec::new();
    for line in &lines {
        let mut candidate = visible.clone();
        candidate.push(line.clone());
        if format!("{heading}{}{ending}", candidate.join("\r\n"))
            .encode_utf16()
            .count()
            > 630
        {
            break;
        }
        visible.push(line.clone());
    }
    if visible.len() < lines.len() {
        visible.push(format!(
            "另有 {} 个自身进程，详见检测日志。",
            lines.len() - visible.len()
        ));
    }
    details.push_str("\r\n\r\n仍在运行：\r\n");
    details.push_str(
        &applications
            .iter()
            .map(|p| {
                format!(
                    "{} PID={} Session={} Path={}",
                    p.name, p.pid, p.session, p.image_path
                )
            })
            .collect::<Vec<_>>()
            .join("\r\n"),
    );
    (
        10,
        format!("{heading}{}{ending}", visible.join("\r\n")),
        details,
    )
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn write_report(path: &std::path::Path, summary: &str, details: &str) -> Result<()> {
    let units: Vec<_> = summary.encode_utf16().collect();
    let summary = if units.len() > 700 {
        format!("{}...", String::from_utf16_lossy(&units[..697]))
    } else {
        summary.to_owned()
    };
    std::fs::write(path, utf16le(&summary)).map_err(|e| e.to_string())?;
    let mut log_path = path.as_os_str().to_os_string();
    log_path.push(".log");
    std::fs::write(log_path, utf16le(details)).map_err(|e| e.to_string())
}

fn run(options: &Options) -> Result<i32> {
    #[cfg(windows)]
    {
        let allowed = windows::allowed_paths(&options.install_dir)?;
        let mut applications = windows::find(&allowed)?;
        let mut stopped = Vec::new();
        if options.stop {
            for application in &applications {
                windows::stop(application, &allowed)?;
                stopped.push(format!("{} (PID {})", application.name, application.pid));
            }
            applications = windows::find(&allowed)?;
        }
        let (code, summary, details) = report(&allowed, &applications, &stopped, options.stop);
        write_report(&options.report_path, &summary, &details)?;
        Ok(code)
    }
    #[cfg(not(windows))]
    {
        let _ = (&options.install_dir, options.stop);
        Err("This check requires Windows.".into())
    }
}

fn main() {
    let code = match Options::parse(std::env::args_os().skip(1)) {
        Ok(options) => match run(&options) {
            Ok(code) => code,
            Err(failure) => {
                let summary = format!("无法检查或停止 KeyTao 自身进程，安装尚未开始。请重试；若仍失败，请手动退出 KeyTao 后再安装。\r\n{failure}");
                if let Err(error) = write_report(&options.report_path, &summary, &failure) {
                    eprintln!("{error}");
                }
                eprintln!("{failure}");
                20
            }
        },
        Err(error) => {
            eprintln!("{error}");
            20
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, session: u32) -> OwnedProcess {
        OwnedProcess {
            pid,
            session,
            name: "keytao-app.exe".into(),
            image_path: r"C:\KeyTao\keytao-app.exe".into(),
            created: 123,
        }
    }

    #[test]
    fn exact_owned_paths_only() {
        let paths = owned_executable_paths("C:\\KeyTao 应用\\");
        assert_eq!(
            paths,
            [
                "C:\\KeyTao 应用\\keytao-app.exe",
                "C:\\KeyTao 应用\\keytao.exe"
            ]
        );
        for path in &paths {
            assert!(owned_image(&paths, path));
        }
        assert!(owned_image(&paths, r"c:\KEYTAO 应用\KEYTAO-APP.EXE"));
        for path in [
            r"C:\Other\keytao-app.exe",
            r"C:\KeyTao 应用2\keytao.exe",
            r"C:\KeyTao 应用\child\keytao.exe",
            r"C:\KeyTao 应用\WeaselServer.exe",
            r"C:\KeyTao 应用\chrome.exe",
            r"C:\KeyTao 应用\explorer.exe",
            r"C:\KeyTao 应用\keytao-windows-ime-server.exe",
            r"C:\KeyTao 应用\keytao.exe.bak",
        ] {
            assert!(!owned_image(&paths, path), "{path}");
        }
        assert_eq!(owned_executable_paths(r"C:\")[0], r"C:\keytao-app.exe");
        assert!(owned_image(
            &owned_executable_paths(r"\\server\share\KeyTao"),
            r"\\SERVER\share\KeyTao\keytao.exe"
        ));
        assert!(same_path(r"C:\é\keytao.exe", r"c:\É\KEYTAO.EXE"));
        assert!(!same_path(
            r"C:\straße\keytao.exe",
            r"C:\strasse\keytao.exe"
        ));
    }

    #[test]
    fn names_and_pid_identity_cannot_expand_ownership() {
        assert!(owned_name("KEYTAO.EXE"));
        assert!(!owned_name("chrome.exe"));
        assert!(!owned_name("WeaselServer.exe"));
        let target = process(1, 1);
        assert!(same_process(&target, r"c:\KEYTAO\KEYTAO-APP.EXE", 123));
        assert!(!same_process(&target, &target.image_path, 124));
        assert!(!same_process(&target, r"C:\Other\keytao-app.exe", 123));
    }

    #[test]
    fn report_contract_and_bounds() {
        let paths = owned_executable_paths(r"C:\KeyTao");
        let (code, summary, details) = report(&paths, &[], &[], false);
        assert_eq!(code, 0);
        assert_eq!(
            summary,
            "KeyTao 自身进程已就绪，可以继续安装。其他输入法、浏览器、资源管理器及其他应用会保留。"
        );
        assert_eq!(
            details,
            format!("允许停止的可执行文件：\r\n{}", paths.join("\r\n"))
        );
        let processes: Vec<_> = (1..100).rev().map(|pid| process(pid, 1)).collect();
        let (code, summary, details) = report(&paths, &processes, &["stopped".into()], true);
        assert_eq!(code, 10);
        assert!(
            summary.starts_with("检测到以下 KeyTao 自身进程：\r\nkeytao-app.exe（PID 1，会话 1）")
        );
        assert!(summary.encode_utf16().count() <= 700);
        assert!(summary.contains("另有 ") && summary.contains("仍在运行，请重试"));
        assert!(details.contains("已处理：\r\nstopped") && details.contains("PID=99"));
        assert_eq!(utf16le("中\r\n"), [0x2d, 0x4e, 13, 0, 10, 0]);
    }

    #[test]
    fn cli_requires_paths_and_explicit_stop() {
        let parse = |args: &[&str]| Options::parse(args.iter().map(OsString::from));
        assert!(parse(&[]).is_err());
        assert!(parse(&["--install-dir", "C:\\KeyTao", "--report-path"]).is_err());
        let options =
            parse(&["--install-dir", "C:\\KeyTao", "--report-path", "report.txt"]).unwrap();
        assert!(!options.stop);
        assert!(
            parse(&[
                "--stop-owned-processes",
                "--install-dir",
                "C:\\KeyTao",
                "--report-path",
                "report.txt"
            ])
            .unwrap()
            .stop
        );
        assert!(parse(&["--other"]).is_err());
    }
}
