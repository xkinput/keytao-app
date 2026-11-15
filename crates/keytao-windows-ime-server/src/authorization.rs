use std::path::Path;

pub fn installed_caller(caller_sid: &str, server_sid: &str, image: &Path, root: &Path) -> bool {
    caller_sid == server_sid && crate::engine_config::is_descendant(image, root)
}

#[cfg(test)]
mod tests {
    use super::*;

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
