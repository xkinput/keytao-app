use super::{safe_path, validate_relative};
use crate::package_store::PackageStore;
use crate::Core;
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

pub(crate) fn package_logo_path(core: &Core, scheme_key: &str) -> Option<PathBuf> {
    let (archive, meta) = PackageStore::new(core).open_archive(scheme_key).ok()?;
    render_archive_logo(&core.env().cache_dir, archive, &meta.zip_sha256, scheme_key)
}

fn read_entry<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    validate_relative(name).ok()?;
    let file = archive.by_name(name).ok()?;
    if file.is_dir()
        || file.size() > MAX_SOURCE_BYTES
        || file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_SOURCE_BYTES).then_some(bytes)
}

fn render_archive_logo<R: Read + Seek>(
    cache_root: &Path,
    reader: R,
    archive_sha: &str,
    scheme_key: &str,
) -> Option<PathBuf> {
    let mut archive = zip::ZipArchive::new(reader).ok()?;
    if archive.len() > 10_000 {
        return None;
    }
    let names: BTreeSet<_> = archive.file_names().map(str::to_owned).collect();
    let mut selected = None;
    for config in ["default.custom.yaml", "default-custom.yaml", "default.yaml"] {
        let Some(content) = read_entry(&mut archive, config) else {
            continue;
        };
        let Ok(content) = std::str::from_utf8(&content) else {
            continue;
        };
        selected = crate::scheme_files::parse_schema_list(content)
            .into_iter()
            .next()
            .map(|id| format!("{id}.schema.yaml"));
        if selected.is_some() {
            break;
        }
    }
    let selected = selected.or_else(|| {
        let matching = format!("{scheme_key}.schema.yaml");
        if names.contains(&matching) {
            return Some(matching);
        }
        let schemas: Vec<_> = names
            .iter()
            .filter(|name| name.ends_with(".schema.yaml"))
            .collect();
        (schemas.len() == 1).then(|| schemas[0].clone())
    })?;
    let schema_bytes = read_entry(&mut archive, &selected)?;
    let schema: serde_yaml::Value = serde_yaml::from_slice(&schema_bytes).ok()?;
    let id = schema["schema"]["schema_id"]
        .as_str()
        .unwrap_or(selected.strip_suffix(".schema.yaml")?);
    validate_relative(id).ok()?;
    if id.contains('/') {
        return None;
    }
    let name = schema["schema"]["name"].as_str().unwrap_or("");
    let icon = schema["schema"]["icon"].as_str();
    let cache = safe_path(cache_root, &format!("scheme-logos-v2/{archive_sha}")).ok()?;
    fs::create_dir_all(&cache).ok()?;
    let output = safe_path(&cache, &format!(".keytao-logo/{id}.png")).ok()?;
    if output.is_file() {
        return Some(output);
    }
    let schema_path = safe_path(&cache, &format!("{id}.schema.yaml")).ok()?;
    crate::scheme_files::write_file_atomic(&schema_path, &schema_bytes).ok()?;
    let parent = Path::new(&selected).parent()?;
    for candidate in keytao_schema_logo::schema_logo_candidates(id, name, icon) {
        let source = parent.join(&candidate);
        let Some(source) = source.to_str() else {
            continue;
        };
        let Some(content) = read_entry(&mut archive, source) else {
            continue;
        };
        let path = safe_path(&cache, &candidate).ok()?;
        crate::scheme_files::write_file_atomic(&path, &content).ok()?;
    }
    keytao_schema_logo::render_schema_logos(&cache, &[(id.into(), name.into())], 192);
    output.is_file().then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn render(cache: &Path, bytes: &[u8]) -> Option<PathBuf> {
        render_archive_logo(
            cache,
            std::io::Cursor::new(bytes),
            &format!("{:x}", Sha256::digest(bytes)),
            "package",
        )
    }

    const RED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="red"/></svg>"#;
    const BLUE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="blue"/></svg>"#;

    #[test]
    fn extracts_primary_schema_icon_with_shared_lookup_and_archive_sha_cache() {
        let dir = tempfile::tempdir().unwrap();
        let schema = b"schema:\n  schema_id: id\n  name: Name\n  icon: own.svg\n";
        let bytes = archive(&[
            (
                "default.custom.yaml",
                b"patch:\n  schema_list:\n    - schema: id\n",
            ),
            ("aux.schema.yaml", b"schema:\n  schema_id: aux\n"),
            ("id.schema.yaml", schema),
            ("own.svg", RED),
            ("id.svg", BLUE),
        ]);
        let path = render(dir.path(), &bytes).unwrap();
        assert!(
            path.to_string_lossy()
                .contains(&format!("{:x}", Sha256::digest(&bytes)))
        );
        let first = fs::read(&path).unwrap();
        assert!(first.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(render(dir.path(), &bytes), Some(path.clone()));
        let changed = archive(&[("id.schema.yaml", schema), ("own.svg", BLUE)]);
        let changed_path = render(dir.path(), &changed).unwrap();
        assert_ne!(path, changed_path);
        assert_ne!(first, fs::read(changed_path).unwrap());
        let fallback = archive(&[("id.schema.yaml", schema), ("id.svg", RED)]);
        assert_eq!(
            first,
            fs::read(render(dir.path(), &fallback).unwrap()).unwrap()
        );
    }

    #[test]
    fn missing_unsafe_oversized_and_invalid_archive_icons_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let oversized = vec![0u8; MAX_SOURCE_BYTES as usize + 1];
        for (schema, icon_name, icon_bytes) in [
            (
                b"schema:\n  schema_id: id\n".as_slice(),
                "unrelated.svg",
                RED,
            ),
            (
                b"schema:\n  schema_id: id\n  icon: ../escape.svg\n",
                "../escape.svg",
                RED,
            ),
            (
                b"schema:\n  schema_id: id\n  icon: own.svg\n",
                "own.svg",
                oversized.as_slice(),
            ),
            (
                b"schema:\n  schema_id: id\n  icon: own.svg\n",
                "own.svg",
                b"invalid",
            ),
            (
                b"schema:\n  schema_id: id\n  icon: own.png\n",
                "own.png",
                RED,
            ),
        ] {
            let bytes = archive(&[("id.schema.yaml", schema), (icon_name, icon_bytes)]);
            assert!(render(dir.path(), &bytes).is_none(), "{icon_name}");
        }
        assert!(!dir.path().join("escape.svg").exists());
        let missing_primary = archive(&[
            (
                "default.custom.yaml",
                b"patch:\n  schema_list:\n    - schema: missing\n    - schema: english\n",
            ),
            ("english.schema.yaml", b"schema:\n  schema_id: english\n"),
            ("english.svg", RED),
        ]);
        assert!(render(dir.path(), &missing_primary).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_icon_and_cache_escape_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("id.schema.yaml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"schema:\n  schema_id: id\n  icon: own.svg\n")
            .unwrap();
        zip.add_symlink(
            "own.svg",
            "outside.svg",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(render(dir.path(), &bytes).is_none());
        let bytes = archive(&[
            ("id.schema.yaml", b"schema:\n  schema_id: id\n"),
            ("id.svg", RED),
        ]);
        let cache = dir.path().join("scheme-logos-v2");
        fs::create_dir_all(&cache).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path(),
            cache.join(format!("{:x}", Sha256::digest(&bytes))),
        )
        .unwrap();
        assert!(render(dir.path(), &bytes).is_none());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
