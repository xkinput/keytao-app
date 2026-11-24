use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const MAX_DICTIONARY_TEXT_BYTES: usize = 64 * 1024 * 1024;
pub const BUNDLED_ENGINE_VERSION: &str = "v0.14.0";

pub fn bundled_engine_library() -> Option<PathBuf> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return engine_library_for_executable(&std::env::current_exe().ok()?);
    #[cfg(target_os = "ios")]
    return cfg!(keytao_ios_libjd)
        .then(|| std::env::current_exe().ok())
        .flatten();
    #[cfg(target_os = "android")]
    android_bundled_engine_library()
}

pub fn engine_library_for_executable(executable: &Path) -> Option<PathBuf> {
    if !executable.is_absolute() {
        return None;
    }
    let relative = match std::env::consts::OS {
        "macos" => "../Frameworks/libjd-engine.dylib",
        "windows" => "libjd-engine.dll",
        "linux" => "runtime/lib/libjd-engine.so",
        _ => return None,
    };
    let library = executable.parent()?.join(relative);
    fs::symlink_metadata(&library)
        .is_ok_and(|metadata| metadata.is_file())
        .then_some(library)
}

#[cfg(target_os = "android")]
fn android_bundled_engine_library() -> Option<PathBuf> {
    use std::{ffi::CStr, mem::MaybeUninit, os::unix::ffi::OsStrExt, sync::OnceLock};
    static LIBRARY: OnceLock<Option<PathBuf>> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            if !cfg!(any(target_arch = "arm", target_arch = "aarch64", target_arch = "x86_64")) {
                return None;
            }
            let mut info = MaybeUninit::<libc::Dl_info>::uninit();
            let address = android_bundled_engine_library as *const () as *const libc::c_void;
            if unsafe { libc::dladdr(address, info.as_mut_ptr()) } == 0 {
                return None;
            }
            let filename = unsafe { info.assume_init() }.dli_fname;
            if filename.is_null() {
                return None;
            }
            let filename = unsafe { CStr::from_ptr(filename) }.to_bytes();
            let library = Path::new(std::ffi::OsStr::from_bytes(filename))
                .parent()?
                .join("libjd-engine.so");
            if !library.is_absolute() {
                return None;
            }
            // An unextracted JNI library has a linker path inside the signed APK.
            unsafe { libloading::Library::new(&library) }.ok()?;
            Some(library)
        })
        .clone()
}

fn dir_or_missing(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

pub fn engine_root(data: &Path) -> Option<PathBuf> {
    let engines = data.join("engines");
    let root = engines.join("libjd");
    (dir_or_missing(&engines) && dir_or_missing(&root)).then_some(root)
}

fn small_text(path: &Path, limit: u64) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    fs::read_to_string(path).ok()
}

pub fn dictionary_directory(data: &Path, key: &str) -> Option<PathBuf> {
    installed_package_directory(data, key)?;
    stable_dictionary_directory(data, key)
}

#[cfg(any(target_os = "ios", test))]
pub fn projected_dictionary_directory(root: &Path, key: &str) -> Option<PathBuf> {
    let path = installed_package_directory(root, key)?.join("libjd-dictionary");
    dir_or_missing(&path).then_some(path)
}

fn stable_dictionary_directory(data: &Path, key: &str) -> Option<PathBuf> {
    let dictionaries = engine_root(data)?.join("dictionaries");
    let package = dictionaries.join(key);
    let directory = package.join("libjd-dictionary");
    [dictionaries, package, directory.clone()]
        .iter()
        .all(|path| dir_or_missing(path))
        .then_some(directory)
}

fn installed_package_directory(data: &Path, key: &str) -> Option<PathBuf> {
    if key.is_empty()
        || key.len() > 128
        || !key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return None;
    }
    let store = crate::package_store::root(data);
    if !fs::symlink_metadata(&store).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    crate::runtime_dirs::package_files_dir(&store, key)
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

pub fn imported_dictionary(data: &Path, key: &str) -> Option<(PathBuf, crate::SchemaInfo)> {
    let legacy = installed_package_directory(data, key)?;
    let directory = stable_dictionary_directory(data, key)?;
    let previous = directory.with_file_name(".old-libjd-dictionary");
    [
        directory,
        previous,
        legacy.join("libjd-dictionary"),
        legacy.join(".old-libjd-dictionary"),
    ]
    .into_iter()
    .find_map(|directory| {
        if !fs::symlink_metadata(&directory).is_ok_and(|meta| meta.is_dir()) {
            return None;
        }
        let dictionary = directory.join("dictionary.bin");
        if !fs::symlink_metadata(&dictionary).is_ok_and(|meta| meta.is_file()) {
            return None;
        }
        let schema =
            serde_json::from_str(&small_text(&directory.join("schema.json"), 8192)?).ok()?;
        Some((dictionary, schema))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionaries_survive_source_replacement_and_reject_symlinked_cache() {
        let root =
            std::env::temp_dir().join(format!("keytao-libjd-dictionary-{}", std::process::id()));
        let package = crate::package_store::root(&root).join("cloud/example");
        let files = package.join("files");
        fs::create_dir_all(&files).unwrap();
        let write_dictionary = |directory: &Path, id: &str| {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("dictionary.bin"), b"dictionary").unwrap();
            fs::write(
                directory.join("schema.json"),
                serde_json::json!({"id": id, "name": id}).to_string(),
            )
            .unwrap();
        };
        let legacy = package.join("libjd-dictionary");
        assert_eq!(projected_dictionary_directory(&root, "example"), Some(legacy.clone()));
        assert_eq!(projected_dictionary_directory(&root, "../example"), None);
        write_dictionary(&legacy, "legacy");
        assert_eq!(
            imported_dictionary(&root, "example").unwrap().1.id,
            "legacy"
        );
        let stable = dictionary_directory(&root, "example").unwrap();
        assert!(stable.starts_with(root.join("engines/libjd/dictionaries")));
        let backup = stable.with_file_name(".old-libjd-dictionary");
        write_dictionary(&backup, "backup");
        assert_eq!(
            imported_dictionary(&root, "example").unwrap().1.id,
            "backup"
        );
        write_dictionary(&stable, "stable");
        fs::remove_dir_all(&package).unwrap();
        assert!(imported_dictionary(&root, "example").is_none());
        fs::create_dir_all(&files).unwrap();
        assert_eq!(
            imported_dictionary(&root, "example").unwrap().1.id,
            "stable"
        );
        #[cfg(unix)]
        {
            fs::remove_dir_all(&stable).unwrap();
            std::os::unix::fs::symlink(&backup, &stable).unwrap();
            assert!(dictionary_directory(&root, "example").is_none());
            assert!(imported_dictionary(&root, "example").is_none());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
