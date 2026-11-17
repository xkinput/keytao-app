use std::path::Path;

use keytao_ime_ipc::ClientRole;

pub fn same_user_session(
    caller_sid: &str,
    server_sid: &str,
    caller_session: u32,
    server_session: u32,
) -> bool {
    caller_sid == server_sid && caller_session == server_session
}

pub fn installed_caller<'a>(
    caller_sid: &str,
    server_sid: &str,
    image: &Path,
    roots: impl IntoIterator<Item = &'a Path>,
) -> bool {
    caller_sid == server_sid
        && roots
            .into_iter()
            .any(|root| crate::engine_config::is_descendant(image, root))
}

pub fn deployment_allowed(role: ClientRole, is_appcontainer: bool, installed: bool) -> bool {
    role == ClientRole::App && !is_appcontainer && installed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_identity_must_match_both_user_and_session() {
        assert!(same_user_session("user", "user", 7, 7));
        assert!(!same_user_session("other", "user", 7, 7));
        assert!(!same_user_session("user", "user", 6, 7));
        assert!(!same_user_session("other", "user", 6, 7));
    }

    #[test]
    fn deployment_requires_installed_app_and_rejects_appcontainer_clients() {
        assert!(deployment_allowed(ClientRole::App, false, true));
        assert!(!deployment_allowed(ClientRole::Ime, false, true));
        assert!(!deployment_allowed(ClientRole::App, true, true));
        assert!(!deployment_allowed(ClientRole::App, false, false));
    }

    #[test]
    fn shutdown_requires_same_user_and_an_image_inside_the_installation_tree() {
        let root = Path::new("C:/Program Files/KeyTao");
        for image in [
            "C:/Program Files/KeyTao/keytao.exe",
            "c:/program files/keytao/runtime/v2/x64/keytao-windows-ime-server.exe",
        ] {
            assert!(installed_caller("user", "user", Path::new(image), [root]));
            assert!(!installed_caller("other", "user", Path::new(image), [root]));
        }
        for image in [
            "C:/Program Files/KeyTao",
            "C:/Program Files/KeyTao-other/server.exe",
            "C:/Users/user/server.exe",
            "C:/Program Files/KeyTao/../server.exe",
        ] {
            assert!(!installed_caller("user", "user", Path::new(image), [root]));
        }
    }

    #[test]
    fn default_and_custom_installs_use_separate_app_and_runtime_roots() {
        // Forward slashes keep Windows path component checks portable to macOS.
        let runtime_root = Path::new("C:/Program Files/KeyTao/keytao-windows-ime-runtime");
        let server = runtime_root.join("1.2.5-alpha.17-old/current/keytao-windows-ime-server.exe");
        let replacement =
            runtime_root.join("1.2.5-alpha.18-new/current/keytao-windows-ime-server.exe");
        assert!(!crate::engine_config::is_descendant(
            Path::new("C:/Program Files/KeyTao/keytao-windows-ime-runtime-other/server.exe"),
            runtime_root,
        ));
        for app_root in [
            Path::new("C:/Program Files/KeyTao"),
            Path::new("D:/Apps/KeyTao"),
        ] {
            let app = app_root.join("keytao-app.exe");
            assert!(crate::engine_config::is_descendant(&server, runtime_root));
            assert!(!crate::engine_config::is_descendant(&app, runtime_root));
            assert!(deployment_allowed(
                ClientRole::App,
                false,
                installed_caller("user", "user", &app, [app_root]),
            ));
            for image in [&app, &replacement] {
                assert!(installed_caller(
                    "user",
                    "user",
                    image,
                    [app_root, runtime_root]
                ));
                assert!(!installed_caller(
                    "other",
                    "user",
                    image,
                    [app_root, runtime_root]
                ));
            }
            for image in [
                "D:/Apps/random.exe",
                "D:/Apps/KeyTao-other/random.exe",
                "C:/Program Files/KeyTao-other/server.exe",
                "C:/Program Files (x86)/KeyTao/keytao-windows-ime-runtime/x86/server.exe",
            ] {
                let image = Path::new(image);
                assert!(!crate::engine_config::is_descendant(image, runtime_root));
                assert!(!deployment_allowed(
                    ClientRole::App,
                    false,
                    installed_caller("user", "user", image, [app_root]),
                ));
                assert!(!installed_caller(
                    "user",
                    "user",
                    image,
                    [app_root, runtime_root]
                ));
            }
        }
    }
}
