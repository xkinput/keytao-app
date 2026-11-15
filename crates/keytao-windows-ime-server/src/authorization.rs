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

pub fn installed_caller(caller_sid: &str, server_sid: &str, image: &Path, root: &Path) -> bool {
    caller_sid == server_sid && crate::engine_config::is_descendant(image, root)
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
            assert!(installed_caller("user", "user", Path::new(image), root));
            assert!(!installed_caller("other", "user", Path::new(image), root));
        }
        for image in [
            "C:/Program Files/KeyTao",
            "C:/Program Files/KeyTao-other/server.exe",
            "C:/Users/user/server.exe",
            "C:/Program Files/KeyTao/../server.exe",
        ] {
            assert!(!installed_caller("user", "user", Path::new(image), root));
        }
    }
}
