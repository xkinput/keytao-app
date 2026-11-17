use std::{fs, path::Path};

const MARKER: &str = " # keytao-candidate-page-size v1 ";

fn decode(content: &str) -> Result<String, String> {
    if !content.len().is_multiple_of(2) || !content.is_ascii() {
        return Err("Invalid managed candidate page size marker".into());
    }
    let bytes = (0..content.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&content[offset..offset + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid managed candidate page size marker")?;
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

pub(crate) fn strip_legacy(content: &str) -> Result<String, String> {
    let mut updated = content.to_owned();
    let mut offset = 0;
    let mut managed = None;
    for line in content.split_inclusive('\n') {
        if let Some((directive, metadata)) = line.trim_end_matches(['\r', '\n']).split_once(MARKER)
        {
            let directive = directive.trim_start().trim_start_matches(", ");
            if !directive.starts_with("__patch:") && !directive.starts_with("- {patch/+:") {
                offset += line.len();
                continue;
            }
            if managed.is_some() {
                return Err("Multiple managed candidate page size entries".into());
            }
            let (joined, original) = metadata
                .split_once(':')
                .ok_or("Invalid managed candidate page size marker")?;
            let joined = match joined {
                "0" => 0,
                "1" if content[..offset].ends_with('\n') => 1,
                "2" if content[..offset].ends_with("\r\n") => 2,
                _ => return Err("Invalid managed candidate page size separator".into()),
            };
            managed = Some((offset - joined..offset + line.len(), decode(original)?));
        }
        offset += line.len();
    }
    if let Some((range, original)) = managed {
        updated.replace_range(range, &original);
    }
    Ok(updated)
}

#[cfg(test)]
fn remove_managed(content: &str) -> Result<String, String> {
    crate::managed_custom::set_patch(content, "candidate-page-size", None)
}

pub(crate) fn patch_candidate_page_size(content: &str, size: Option<u8>) -> Result<String, String> {
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    let patch = size.map(|size| format!("{{menu/page_size: {size}}}"));
    crate::managed_custom::set_patch(content, "candidate-page-size", patch.as_deref())
}

#[cfg(test)]
fn apply_candidate_page_size(root: &Path, size: Option<u8>) -> Result<bool, String> {
    apply_for_schemas(root, size, &crate::managed_custom::active_schema_ids(root)?)
}

pub(crate) fn apply_for_schemas(
    root: &Path,
    size: Option<u8>,
    schemas: &std::collections::BTreeSet<String>,
) -> Result<bool, String> {
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    let mut changed = false;
    for id in schemas {
        let result = (|| -> Result<(), String> {
            let schema_path = crate::packages::safe_path(root, &format!("{id}.schema.yaml"))?;
            if !schema_path.is_file() {
                return Ok(());
            }
            let relative = format!("{id}.custom.yaml");
            let path = crate::packages::safe_path(root, &relative)?;
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            let updated = patch_candidate_page_size(&content, size)?;
            changed |= crate::managed_custom::write_custom(root, &relative, &content, &updated)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(schema = id, %error, "Skipped candidate page size override");
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_page_size_insert_update_remove_preserves_user_bytes() {
        for original in [
            "",
            "# User settings without final newline",
            "patch:\n  menu/page_size: 9 # User preference\n",
            "# Keep CRLF\r\npatch: {menu/page_size: 9}\r\n",
            "patch: &settings\n  menu/page_size: 9\ncopy: *settings\n",
            "patch:\n    menu/page_size: 9 # No final newline",
            "---\npatch: {menu/page_size: 9}\n...\n",
            "---\npatch: {menu/page_size: 9}\n... # End of document",
            "__patch: existing:/patch # Keep this\npatch: {menu/page_size: 9}\n",
            "__patch: [existing:/patch, {patch/name: 'Keep this'}]\n",
            "__patch:\n  - existing:/patch\n  - {patch/name: value}\npatch: {menu/page_size: 9}\n",
            "__patch:\n- existing:/patch\n- {patch/name: value}",
            "__patch:\n  patch/name: value\npatch: {menu/page_size: 9}\n",
        ] {
            let patched = patch_candidate_page_size(original, Some(6))
                .unwrap_or_else(|error| panic!("{original:?}: {error}"));
            assert_eq!(
                patch_candidate_page_size(&patched, Some(6)).unwrap(),
                patched
            );
            let updated = patch_candidate_page_size(&patched, Some(10)).unwrap();
            assert!(updated.contains("menu/page_size: 10"));
            assert_eq!(patch_candidate_page_size(&updated, None).unwrap(), original);
            assert_eq!(patch_candidate_page_size(original, None).unwrap(), original);
            if !original.contains("__patch:") {
                let restored = remove_managed(&patched).unwrap();
                assert_eq!(restored.as_bytes(), original.as_bytes());
                for line in original.split_inclusive('\n') {
                    assert!(patched.contains(line), "Missing user bytes: {line:?}");
                }
            }
        }
    }

    #[test]
    fn candidate_page_size_applies_to_every_installed_schema() {
        let dir = tempfile::tempdir().unwrap();
        let original = "patch: {menu/page_size: 9} # Keep user preference\n";
        for id in ["keytao", "xmjd6", "txjx", "keydo", "wanxiang", "custom"] {
            fs::write(
                dir.path().join(format!("{id}.schema.yaml")),
                "menu: {page_size: 6}\n",
            )
            .unwrap();
            fs::write(dir.path().join(format!("{id}.custom.yaml")), original).unwrap();
        }
        fs::write(dir.path().join("uninstalled.custom.yaml"), original).unwrap();
        assert!(apply_candidate_page_size(dir.path(), Some(3)).unwrap());
        assert!(!apply_candidate_page_size(dir.path(), Some(3)).unwrap());
        for id in ["keytao", "xmjd6", "txjx", "keydo", "wanxiang", "custom"] {
            let patched = fs::read_to_string(dir.path().join(format!("{id}.custom.yaml"))).unwrap();
            assert!(patched.starts_with(original));
            assert!(patched.contains("menu/page_size: 3"));
        }
        assert!(apply_candidate_page_size(dir.path(), None).unwrap());
        assert!(!apply_candidate_page_size(dir.path(), None).unwrap());
        for id in [
            "keytao",
            "xmjd6",
            "txjx",
            "keydo",
            "wanxiang",
            "custom",
            "uninstalled",
        ] {
            assert_eq!(
                fs::read_to_string(dir.path().join(format!("{id}.custom.yaml"))).unwrap(),
                original
            );
        }
    }

    #[test]
    fn candidate_page_size_flow_root_and_root_indentation_preserve_user_bytes() {
        for original in [
            "{patch: {menu/page_size: 9}}\n",
            "# Keep comments before flow root\n---\n{patch: {menu/page_size: 9}}\n...\n",
            "--- {patch: {menu/page_size: 9}}\n",
            "{}",
            "{patch: {menu/page_size: 9}, # Keep trailing comma\n}\n",
            "{patch: &settings {menu/page_size: 9}, copy: *settings}\n",
            "{__patch: existing:/patch, patch: {menu/page_size: 9}}\n",
            "{patch: {menu/page_size: 9}, '__patch': [{patch/name: 'Keep, braces {}'}]}\n",
            "\u{feff}{patch: {menu/page_size: 9}}\r\n",
            "  patch: {menu/page_size: 9}\n",
            "\u{feff}  patch: {menu/page_size: 9}\n",
            "\u{feff}__patch: existing:/patch\npatch: {menu/page_size: 9}\n",
            "  __patch:\n    - existing:/patch\n  patch: {menu/page_size: 9}\n",
        ] {
            let patched = patch_candidate_page_size(original, Some(6))
                .unwrap_or_else(|error| panic!("{original:?}: {error}"));
            assert_eq!(
                patch_candidate_page_size(&patched, Some(6)).unwrap(),
                patched
            );
            let updated = patch_candidate_page_size(&patched, Some(3)).unwrap();
            assert!(updated.contains("menu/page_size: 3"));
            assert_eq!(patch_candidate_page_size(&updated, None).unwrap(), original);
            assert_eq!(
                remove_managed(&updated).unwrap().as_bytes(),
                original.as_bytes()
            );
        }
    }

    #[test]
    fn candidate_page_size_invalid_custom_does_not_block_other_schemes() {
        for invalid in [
            "patch: [\n",
            "- scalar\n",
            "patch:\n  menu/page_size: 9\n  menu/page_size: 8\n",
            "patch: {menu/page_size: 9}\n---\n",
            "__patch: {{nested: key}: value}\n",
            "__patch: [] # keytao-candidate-page-size v1 invalid\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            for id in ["valid", "invalid"] {
                fs::write(
                    dir.path().join(format!("{id}.schema.yaml")),
                    "menu: {page_size: 6}\n",
                )
                .unwrap();
            }
            let valid = "patch: {menu/page_size: 9}\n";
            fs::write(dir.path().join("valid.custom.yaml"), valid).unwrap();
            fs::write(dir.path().join("invalid.custom.yaml"), invalid).unwrap();
            assert!(apply_candidate_page_size(dir.path(), Some(5)).unwrap());
            assert!(!apply_candidate_page_size(dir.path(), Some(5)).unwrap());
            let patched = fs::read_to_string(dir.path().join("valid.custom.yaml")).unwrap();
            assert!(patched.starts_with(valid));
            assert!(patched.contains("menu/page_size: 5"));
            assert_eq!(
                fs::read_to_string(dir.path().join("invalid.custom.yaml")).unwrap(),
                invalid
            );
            assert!(apply_candidate_page_size(dir.path(), None).unwrap());
            assert!(!apply_candidate_page_size(dir.path(), None).unwrap());
            assert_eq!(
                fs::read_to_string(dir.path().join("valid.custom.yaml")).unwrap(),
                valid
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("invalid.custom.yaml")).unwrap(),
                invalid
            );
        }
        for size in [0, 11, u8::MAX] {
            assert!(patch_candidate_page_size("", Some(size)).is_err());
        }
        let user_marker = "# keytao-candidate-page-size\npatch: {menu/page_size: 9}\n";
        assert_eq!(
            patch_candidate_page_size(user_marker, None).unwrap(),
            user_marker
        );
    }
}
