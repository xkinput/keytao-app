use std::{fs, path::Path};

use serde_yaml::Value;

const MARKER: &str = " # keytao-candidate-page-size v1 ";

fn parse_custom(content: &str) -> Result<Value, String> {
    let value: Value = serde_yaml::from_str(content.strip_prefix('\u{feff}').unwrap_or(content))
        .map_err(|error| error.to_string())?;
    if value.is_null() {
        Ok(Value::Mapping(Default::default()))
    } else if value.is_mapping() {
        Ok(value)
    } else {
        Err("Invalid custom mapping".into())
    }
}

fn encode(content: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(content.len() * 2);
    for byte in content.bytes() {
        write!(encoded, "{byte:02x}").unwrap();
    }
    encoded
}

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

fn remove_managed(content: &str) -> Result<String, String> {
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

fn is_root_entry(line: &str, indent: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    line.len() - trimmed.len() <= indent
        && !trimmed.trim().is_empty()
        && !trimmed.starts_with(['\t', '#', '%'])
        && !trimmed.starts_with("- ")
        && trimmed.trim() != "-"
        && trimmed.trim() != "---"
}

fn insert_managed(original: &str, start: usize, end: usize, directive: &str) -> String {
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let joined = if start > 0 && !original[..start].ends_with('\n') {
        newline.len()
    } else {
        0
    };
    let separator = if joined == 0 { "" } else { newline };
    let managed = format!(
        "{separator}{directive}{MARKER}{joined}:{}{newline}",
        encode(&original[start..end])
    );
    let mut updated = original.to_owned();
    updated.replace_range(start..end, &managed);
    updated
}

fn patch_flow_root(
    original: &str,
    expected: &Value,
    directive: &str,
    has_patch: bool,
) -> Option<String> {
    let first_value = original
        .lines()
        .map(|line| line.trim_start_matches(['\u{feff}', ' ', '\t']))
        .find(|line| {
            !line.trim().is_empty() && !line.starts_with(['#', '%']) && line.trim() != "---"
        });
    if !first_value.is_some_and(|line| line.strip_prefix("--- ").unwrap_or(line).starts_with('{')) {
        return None;
    }
    if has_patch {
        for key in ["__patch:", "\"__patch\":", "'__patch':"] {
            for (start, _) in original.match_indices(key) {
                for (relative_end, _) in original[start + key.len()..].match_indices([',', '}']) {
                    let end = start + key.len() + relative_end;
                    let updated = insert_managed(original, start, end, &format!("  {directive}"));
                    if parse_custom(&updated).ok().as_ref() == Some(expected) {
                        return Some(updated);
                    }
                }
            }
        }
    } else {
        for (end, _) in original.rmatch_indices('}') {
            for separator in [", ", ""] {
                let updated =
                    insert_managed(original, end, end, &format!("  {separator}{directive}"));
                if parse_custom(&updated).ok().as_ref() == Some(expected) {
                    return Some(updated);
                }
            }
        }
    }
    None
}

pub(crate) fn patch_candidate_page_size(content: &str, size: Option<u8>) -> Result<String, String> {
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    let original = remove_managed(content)?;
    let Some(size) = size else {
        return Ok(original);
    };
    let mut expected = parse_custom(&original)?;
    if expected
        .get("patch")
        .is_some_and(|patch| !patch.is_mapping() && !patch.is_null())
    {
        return Err("Invalid custom patch mapping".into());
    }
    let literal = format!("{{patch/+: {{menu/page_size: {size}}}}}");
    let value: Value = serde_yaml::from_str(&literal).map_err(|error| error.to_string())?;
    let previous = expected.as_mapping_mut().unwrap().remove("__patch");
    let has_patch = previous.is_some();
    let mut patches = match previous.clone() {
        Some(Value::Sequence(patches)) => patches,
        Some(previous) => vec![previous],
        None => Vec::new(),
    };
    patches.push(value);
    expected.as_mapping_mut().unwrap().insert(
        Value::String("__patch".into()),
        Value::Sequence(patches.clone()),
    );
    patches.pop();
    let existing = serde_json::to_string(&patches).map_err(|error| error.to_string())?;
    let existing = existing.strip_suffix(']').unwrap();
    let separator = if patches.is_empty() { "" } else { ", " };
    let flow_directive = format!("__patch: {existing}{separator}{literal}]");
    if let Some(updated) = patch_flow_root(&original, &expected, &flow_directive, has_patch) {
        return Ok(updated);
    }
    let root_indent = original
        .lines()
        .find_map(|line| {
            let line = line.strip_prefix('\u{feff}').unwrap_or(line);
            let trimmed = line.trim_start_matches(' ');
            (!trimmed.trim().is_empty()
                && !trimmed.starts_with(['#', '%'])
                && !trimmed.starts_with("---"))
            .then_some(line.len() - trimmed.len())
        })
        .unwrap_or(0);
    let mut offset = 0;
    let mut header = None;
    let mut document_end = original.len();
    let mut lines = original.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line
            .strip_prefix("...")
            .is_some_and(|tail| tail.trim().is_empty() || tail.trim_start().starts_with('#'))
        {
            document_end = offset;
        }
        let normalized = line.strip_prefix('\u{feff}').unwrap_or(line);
        let trimmed = normalized.trim_start_matches(' ');
        if normalized.len() - trimmed.len() == root_indent
            && ["__patch:", "\"__patch\":", "'__patch':"]
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        {
            let start = offset;
            offset += line.len();
            let mut end = offset;
            while let Some(next) = lines.peek() {
                if is_root_entry(next, root_indent) {
                    break;
                }
                end += next.len();
                lines.next();
            }
            header = Some((start, offset, end, line));
            offset = end;
        } else {
            offset += line.len();
        }
    }

    let (start, end, directive) = match (previous, header) {
        (None, _) => (
            document_end,
            document_end,
            format!("{}{flow_directive}", " ".repeat(root_indent)),
        ),
        (Some(Value::Sequence(_)), Some((_, after_header, end, line)))
            if line.split_once(':').is_some_and(|(_, tail)| {
                tail.trim().is_empty() || tail.trim_start().starts_with('#')
            }) =>
        {
            let indent = original[after_header..end]
                .lines()
                .find(|line| line.trim_start().starts_with('-'))
                .map(|line| line.len() - line.trim_start_matches(' ').len())
                .ok_or("Cannot locate custom patch list")?;
            (end, end, format!("{}- {literal}", " ".repeat(indent)))
        }
        (Some(_), Some((start, _, end, _))) => (
            start,
            end,
            format!("{}{flow_directive}", " ".repeat(root_indent)),
        ),
        _ => return Err("Cannot safely locate custom __patch directive".into()),
    };
    let updated = insert_managed(&original, start, end, &directive);
    // The compiler applies this last directive after user patches without editing them.
    if parse_custom(&updated)? != expected {
        return Err("Cannot safely insert candidate page size override".into());
    }
    Ok(updated)
}

pub(crate) fn apply_candidate_page_size(root: &Path, size: Option<u8>) -> Result<bool, String> {
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    let mut changed = false;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, "Skipped candidate page size directory entry");
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name.strip_suffix(".schema.yaml") else {
            continue;
        };
        let result = (|| -> Result<(), String> {
            let schema_path = crate::packages::safe_path(root, name)?;
            if !schema_path.is_file() {
                return Ok(());
            }
            let path = crate::packages::safe_path(root, &format!("{id}.custom.yaml"))?;
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            let updated = patch_candidate_page_size(&content, size)?;
            if updated != content {
                crate::scheme_files::write_file_atomic(&path, updated.as_bytes())
                    .map_err(|error| error.to_string())?;
                changed = true;
            }
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
