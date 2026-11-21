use super::*;
use crate::{AppEnv, CoreEvent, EventSink, Platform};
use std::{
    collections::HashMap,
    io::{Cursor, Write},
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: CoreEvent) {}
}

fn open_core(dir: &Path) -> Arc<Core> {
    Core::new(
        AppEnv {
            data_dir: dir.join("state"),
            cache_dir: dir.join("cache"),
            resource_dir: dir.join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(NullSink),
    )
    .unwrap()
}

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, content) in entries {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn targz_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        for (path, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder.append_data(&mut header, *path, *content).unwrap();
        }
        builder.finish().unwrap();
    }
    encoder.finish().unwrap()
}

struct MockRoute {
    status: &'static str,
    headers: &'static str,
    body: Vec<u8>,
}

async fn spawn_mock_github(routes: Arc<Mutex<HashMap<String, MockRoute>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut buf = [0u8; 2048];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                }
                let first_line = String::from_utf8_lossy(&req);
                let path = first_line
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let route = routes.lock().unwrap().get(path).map(|r| {
                    (r.status, r.headers, r.body.clone())
                });
                let (status, headers, body) = route.unwrap_or_else(|| {
                    ("404 Not Found", "Content-Type: application/json\r\n", b"{}".to_vec())
                });
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn git_release_auto_picks_targz_unwraps_folder_and_detects_updates() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let user_root = dir.path().join("rime");
    fs::create_dir_all(&user_root).unwrap();

    let archive = targz_bytes(&[
        (
            "demo-release/demo.schema.yaml",
            "schema:\n  schema_id: demo\n  name: 示例方案\n".as_bytes(),
        ),
        (
            "demo-release/demo.dict.yaml",
            "---\nname: demo\nversion: \"1\"\n...\n测试\tcs\n".as_bytes(),
        ),
    ]);
    let routes = Arc::new(Mutex::new(HashMap::new()));
    let origin = spawn_mock_github(routes.clone()).await;

    routes.lock().unwrap().insert(
        "/github/api/repos/acme/rime-demo/releases/latest".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/json\r\n",
            body: serde_json::to_vec(&serde_json::json!({
                "tag_name": "v1.2.0",
                "assets": [
                    {
                        "name": "rime-demo.tar.gz.sha256",
                        "browser_download_url": format!("{origin}/files/rime-demo.tar.gz.sha256"),
                    },
                    {
                        "name": "rime-demo.tar.gz",
                        "browser_download_url": format!("{origin}/files/rime-demo.tar.gz"),
                    }
                ]
            }))
            .unwrap(),
        },
    );
    routes.lock().unwrap().insert(
        "/files/rime-demo.tar.gz".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/gzip\r\n",
            body: archive,
        },
    );

    let key = import_git_scheme(
        &core,
        &user_root,
        GitSchemeConfig {
            repo_url: format!("{origin}/github/acme/rime-demo.git"),
            mode: GitSchemeMode::Release,
            branch: None,
            asset_name: None,
        },
        "",
        None,
    )
    .await
    .unwrap();

    assert_eq!(key, "local-rime-demo");
    let installed = crate::packages::list_installed_packages(&core).unwrap();
    let pkg = installed.iter().find(|p| p.scheme_key == key).unwrap();
    assert_eq!(pkg.display_name.as_deref(), Some("示例方案"));
    assert_eq!(pkg.version.as_deref(), Some("v1.2.0"));
    assert_eq!(
        pkg.git.as_ref().unwrap().repo_url,
        format!("{origin}/github/acme/rime-demo")
    );

    // Bump latest release to v1.3.0 and verify check_scheme_updates spots it!
    routes.lock().unwrap().insert(
        "/github/api/repos/acme/rime-demo/releases/latest".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/json\r\n",
            body: serde_json::to_vec(&serde_json::json!({
                "tag_name": "v1.3.0",
                "assets": [{
                    "name": "rime-demo.tar.gz",
                    "browser_download_url": format!("{origin}/files/rime-demo.tar.gz"),
                }]
            }))
            .unwrap(),
        },
    );
    let updates = crate::packages::check_scheme_updates(&core).await;
    let update = updates.iter().find(|u| u.scheme_key == key).unwrap();
    assert!(update.has_update);
    assert_eq!(update.installed_version, "v1.2.0");
    assert_eq!(update.latest_version, "v1.3.0");
}

#[tokio::test]
async fn git_fixed_latest_asset_survives_rate_limit_and_branch_tree_url_syncs_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let user_root = dir.path().join("rime");
    fs::create_dir_all(&user_root).unwrap();

    let fixed_zip = zip_bytes(&[(
        "fixed.schema.yaml",
        "schema:\n  schema_id: fixed\n  name: 固定发布包\n".as_bytes(),
    )]);
    let branch_v1 = zip_bytes(&[(
        "my-repo-dev/branch_demo.schema.yaml",
        "schema:\n  schema_id: branch_demo\n  name: 分支方案\n".as_bytes(),
    )]);
    let branch_v2 = zip_bytes(&[
        (
            "my-repo-dev/branch_demo.schema.yaml",
            "schema:\n  schema_id: branch_demo\n  name: 分支方案\n".as_bytes(),
        ),
        (
            "my-repo-dev/lua/helper.lua",
            b"return {}\n",
        ),
    ]);

    let routes = Arc::new(Mutex::new(HashMap::new()));
    let origin = spawn_mock_github(routes.clone()).await;

    // Simulate GitHub API 403 rate limit on releases/latest while serving /releases/latest/download/full.zip!
    routes.lock().unwrap().insert(
        "/github/api/repos/acme/fixed-repo/releases/latest".into(),
        MockRoute {
            status: "403 Forbidden",
            headers: "x-ratelimit-remaining: 0\r\nContent-Type: application/json\r\n",
            body: br#"{"message":"API rate limit exceeded"}"#.to_vec(),
        },
    );
    routes.lock().unwrap().insert(
        "/github/acme/fixed-repo/releases/latest/download/full.zip".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/zip\r\n",
            body: fixed_zip,
        },
    );

    let fixed_key = import_git_scheme(
        &core,
        &user_root,
        GitSchemeConfig {
            repo_url: format!("{origin}/github/acme/fixed-repo"),
            mode: GitSchemeMode::Release,
            branch: None,
            asset_name: Some("full.zip".into()),
        },
        "",
        None,
    )
    .await
    .unwrap();
    assert_eq!(fixed_key, "local-fixed-repo");

    // Branch archive via pasted /tree/dev URL + in-place sync update!
    routes.lock().unwrap().insert(
        "/github/api/repos/acme/my-repo/commits/dev".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/json\r\n",
            body: br#"{"sha":"111111122222223333333"}"#.to_vec(),
        },
    );
    routes.lock().unwrap().insert(
        "/github/codeload/acme/my-repo/zip/refs/heads/dev".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/zip\r\n",
            body: branch_v1,
        },
    );

    let branch_key = import_git_scheme(
        &core,
        &user_root,
        GitSchemeConfig {
            repo_url: format!("{origin}/github/acme/my-repo/tree/dev"),
            mode: GitSchemeMode::Release, // Auto-switched to Branch because of /tree/dev!
            branch: None,
            asset_name: None,
        },
        "自定义分支方案",
        None,
    )
    .await
    .unwrap();
    assert_eq!(branch_key, "local-my-repo");
    let meta = PackageStore::new(&core).read_meta(&branch_key).unwrap().unwrap();
    assert_eq!(meta.version.as_deref(), Some("111111122222"));
    assert_eq!(meta.git.as_ref().unwrap().mode, GitSchemeMode::Branch);
    assert_eq!(meta.git.as_ref().unwrap().branch.as_deref(), Some("dev"));

    // Update branch commit & archive and sync via download_scheme_package!
    routes.lock().unwrap().insert(
        "/github/api/repos/acme/my-repo/commits/dev".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/json\r\n",
            body: br#"{"sha":"aaaaaaa99999998888888"}"#.to_vec(),
        },
    );
    routes.lock().unwrap().insert(
        "/github/codeload/acme/my-repo/zip/refs/heads/dev".into(),
        MockRoute {
            status: "200 OK",
            headers: "Content-Type: application/zip\r\n",
            body: branch_v2,
        },
    );

    crate::packages::download_scheme_package(&core, &branch_key, "github", &user_root)
        .await
        .unwrap();
    let updated = PackageStore::new(&core).read_meta(&branch_key).unwrap().unwrap();
    assert_eq!(updated.version.as_deref(), Some("aaaaaaa99999"));
    assert_eq!(updated.display_name.as_deref(), Some("自定义分支方案"));
    assert!(updated.manifest.contains_key("lua/helper.lua"));
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[tokio::test]
async fn active_libjd_blocks_downloading_and_switching_other_schemes_until_reverted() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    let user_root = dir.path().join("rime");
    fs::create_dir_all(&user_root).unwrap();
    crate::package_migration::ensure_before_write(&core, &user_root)
        .await
        .unwrap();

    let keytao_zip = zip_bytes(&[(
        "keytao.schema.yaml",
        "schema:\n  schema_id: keytao\n  name: 键道6\n".as_bytes(),
    )]);
    crate::packages::store_cloud_archive(
        &core,
        "keytao",
        Some("v1.0.0"),
        "official",
        None,
        &keytao_zip,
        &user_root,
    )
    .unwrap();

    // Fake an installed libjd tag in core.env().data_dir and set keytao engine to libjd.
    let platform = keytao_core::libjd::platform().unwrap();
    let engine_root = keytao_core::libjd::engine_root(&core.env().data_dir).unwrap();
    let lib_path = platform.library_path(&core.env().data_dir, "v0.1.0").unwrap();
    fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
    fs::write(&lib_path, b"libjd").unwrap();
    fs::write(
        lib_path.with_file_name("manifest.json"),
        serde_json::to_vec(&keytao_core::libjd::LibjdManifest {
            tag: "v0.1.0".into(),
            asset: platform.asset_name("v0.1.0"),
            sha256: "a".repeat(64),
            abi_layout: serde_json::json!({}),
            installed_at: 1,
        })
        .unwrap(),
    )
    .unwrap();
    fs::write(platform.current_path(&engine_root), b"v0.1.0").unwrap();

    crate::packages::set_package_engine(&core, &user_root, "keytao", "libjd").unwrap();
    assert_eq!(
        keytao_core::RuntimeDirs::active_package_key(&user_root).as_deref(),
        Some("keytao")
    );

    // While using ime-jd, downloading and switching to other schemes must fail with the exact hint.
    let git_err = import_git_scheme(
        &core,
        &user_root,
        GitSchemeConfig {
            repo_url: "https://github.com/acme/other".into(),
            mode: GitSchemeMode::Release,
            branch: None,
            asset_name: None,
        },
        "",
        None,
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(git_err, crate::deploy::LIBJD_SWITCH_BACK_HINT);

    let dl_err = crate::packages::download_scheme_package(&core, "xmjd", "github", &user_root)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(dl_err, crate::deploy::LIBJD_SWITCH_BACK_HINT);

    let sw_err = crate::deploy::switch_package_with_schema(&core, &user_root, "xmjd", None)
        .await
        .unwrap_err();
    assert_eq!(sw_err, crate::deploy::LIBJD_SWITCH_BACK_HINT);
}
