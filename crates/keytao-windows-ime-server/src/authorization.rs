use std::path::{Component, Path};

pub fn installed_caller(caller_sid: &str, server_sid: &str, image: &Path, root: &Path) -> bool {
    if caller_sid != server_sid
        || image
            .components()
            .chain(root.components())
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let image: Vec<_> = image.components().collect();
    let root: Vec<_> = root.components().collect();
    image.len() > root.len()
        && root.iter().zip(&image).all(|(left, right)| {
            left.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
        })
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
