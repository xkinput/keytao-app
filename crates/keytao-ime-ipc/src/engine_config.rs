//! Portable configuration shared by the DLL, server, and App.

use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineMode {
    InProc,
    #[default]
    Pipe,
}

impl EngineMode {
    /// Only an explicit, exactly sized REG_DWORD zero disables IPC.
    pub fn from_registry(value: Option<(u32, &[u8])>) -> Self {
        match value {
            Some((4, [0, 0, 0, 0])) => Self::InProc,
            _ => Self::Pipe,
        }
    }
}

pub fn pipe_name(sid: &str, session_id: u32) -> String {
    format!(r"\\.\pipe\KeyTao.Ime.{sid}.{session_id}.v1")
}

pub fn pipe_sddl(sid: &str) -> String {
    format!("D:(A;;GRGW;;;{sid})(A;;GRGW;;;S-1-15-2-1)S:(ML;;NW;;;LW)")
}

/// Select the first existing installation directory from trusted machine records.
pub fn resolve_install_root(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find_map(|candidate| {
        let value = candidate.to_str()?;
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        let path = Path::new(value);
        // Path::components normalizes interior dots, so reject them before canonicalizing.
        if !path.is_absolute()
            || value
                .split(['/', '\\'])
                .any(|part| matches!(part, "." | ".."))
        {
            return None;
        }
        path.canonicalize().ok().filter(|root| root.is_dir())
    })
}

pub fn is_descendant(image: &Path, root: &Path) -> bool {
    if image
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
    fn installation_root_uses_record_order_and_strips_surrounding_quotes() {
        let primary = std::env::current_dir().unwrap().canonicalize().unwrap();
        let fallback = primary.parent().unwrap().to_path_buf();
        let quoted = PathBuf::from(format!("\"{}\"", primary.display()));
        assert_eq!(
            resolve_install_root([quoted, fallback.clone()]),
            Some(primary.clone())
        );
        assert_eq!(
            resolve_install_root([fallback.clone(), primary]),
            Some(fallback)
        );
    }

    #[test]
    fn installation_root_skips_invalid_records_before_falling_back() {
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let executable = std::env::current_exe().unwrap();
        let separator = std::path::MAIN_SEPARATOR;
        // join() normalizes dots in verbatim Windows paths; preserve the raw invalid records.
        for invalid in [
            PathBuf::new(),
            PathBuf::from("KeyTao"),
            PathBuf::from("D:KeyTao"),
            PathBuf::from("\\KeyTao"),
            PathBuf::from(format!("{}{separator}.", root.display())),
            PathBuf::from(format!("{}{separator}..", root.display())),
            PathBuf::from(format!(
                "{}{separator}.{separator}{}",
                root.parent().unwrap().display(),
                root.file_name().unwrap().to_string_lossy()
            )),
            PathBuf::from(format!("\"{}", root.display())),
            PathBuf::from(format!("{}\"", root.display())),
            executable.clone(),
            executable.join("missing"),
        ] {
            assert_eq!(resolve_install_root([invalid.clone()]), None, "{invalid:?}");
            assert_eq!(
                resolve_install_root([invalid.clone(), root.clone()]),
                Some(root.clone()),
                "{invalid:?}"
            );
        }
        assert_eq!(resolve_install_root([]), None);
    }

    #[test]
    fn only_explicit_zero_selects_inproc() {
        assert_eq!(
            EngineMode::from_registry(Some((4, &[0, 0, 0, 0]))),
            EngineMode::InProc
        );
        for value in [
            None,
            Some((4, &[1, 0, 0, 0][..])),
            Some((4, &[2, 0, 0, 0][..])),
            Some((1, &[1, 0, 0, 0][..])),
            Some((4, &[1][..])),
            Some((4, &[1, 0, 0, 0, 0][..])),
            Some((4, &[255, 255, 255, 255][..])),
            Some((1, &[0, 0, 0, 0][..])),
            Some((4, &[0][..])),
        ] {
            assert_eq!(EngineMode::from_registry(value), EngineMode::Pipe);
        }
    }

    #[test]
    fn installation_descendants_require_a_component_boundary_and_no_traversal() {
        let root = Path::new("C:/Program Files/KeyTao");
        assert!(is_descendant(
            Path::new("c:/program files/keytao/runtime/server.exe"),
            root
        ));
        for image in [
            "C:/Program Files/KeyTao",
            "C:/Program Files/KeyTao-other/server.exe",
            "C:/Program Files/KeyTao/../server.exe",
            "./C:/Program Files/KeyTao/server.exe",
        ] {
            assert!(!is_descendant(Path::new(image), root), "{image}");
        }
        for root in ["C:/Program Files/KeyTao/..", "./C:/Program Files/KeyTao"] {
            assert!(!is_descendant(
                Path::new("C:/Program Files/KeyTao/../server.exe"),
                Path::new(root)
            ));
        }
    }
}
