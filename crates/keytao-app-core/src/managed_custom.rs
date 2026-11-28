use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_yaml::Value;

const BEGIN: &str = "# keytao-generated begin v1 ";
const END: &str = "# keytao-generated end v1";

pub(crate) fn active_schema_ids_from_default(root: &Path) -> Result<BTreeSet<String>, String> {
    for relative in ["default.custom.yaml", "default-custom.yaml", "default.yaml"] {
        let path = crate::packages::safe_path(root, relative)?;
        match fs::read_to_string(path) {
            Ok(content) => {
                return Ok(crate::scheme_files::parse_schema_list(&content)
                    .into_iter()
                    .filter(|id| {
                        crate::packages::safe_path(root, &format!("{id}.schema.yaml"))
                            .is_ok_and(|path| path.is_file())
                    })
                    .collect());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(BTreeSet::new())
}

pub(crate) fn active_schema_ids(root: &Path) -> Result<BTreeSet<String>, String> {
    let configured = active_schema_ids_from_default(root)?;
    let marker = crate::packages::safe_path(root, keytao_core::ACTIVE_SCHEME_MANIFEST)?;
    match fs::read(marker) {
        Ok(bytes) => {
            let active: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let manifest = active["manifest"]
                .as_object()
                .ok_or("Invalid active package manifest")?;
            let mut schemas: BTreeSet<String> = manifest
                .keys()
                .filter_map(|path| path.strip_suffix(".schema.yaml").map(str::to_owned))
                .collect();
            schemas.extend(
                configured
                    .into_iter()
                    .filter(|id| keytao_core::is_addon_schema(id)),
            );
            Ok(schemas)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if ["default.custom.yaml", "default-custom.yaml", "default.yaml"]
                .iter()
                .any(|name| root.join(name).is_file())
            {
                return Ok(configured);
            }
            let entries = match fs::read_dir(root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(BTreeSet::new());
                }
                Err(error) => return Err(error.to_string()),
            };
            let mut schemas = BTreeSet::new();
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".schema.yaml"))
                {
                    if entry
                        .file_type()
                        .map_err(|error| error.to_string())?
                        .is_file()
                    {
                        schemas.insert(id.to_owned());
                    }
                }
            }
            Ok(schemas)
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn is_generated(relative: &str, bytes: &[u8]) -> bool {
    if relative == crate::remove_filter::SCRIPT_PATH {
        return bytes == crate::remove_filter::SCRIPT.as_bytes();
    }
    if relative == crate::grammar_hint::SCRIPT_PATH {
        return bytes == crate::grammar_hint::SCRIPT.as_bytes();
    }
    relative.ends_with(".custom.yaml")
        && std::str::from_utf8(bytes).is_ok_and(|content| {
            content.contains(BEGIN) || strip(content).is_ok_and(|original| original != content)
        })
}

pub(crate) fn generated_paths(root: &Path) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    collect_generated_customs(root, "", &mut paths)?;
    for (relative, expected) in [
        (
            crate::remove_filter::SCRIPT_PATH,
            crate::remove_filter::SCRIPT.as_bytes(),
        ),
        (
            crate::grammar_hint::SCRIPT_PATH,
            crate::grammar_hint::SCRIPT.as_bytes(),
        ),
    ] {
        let script = crate::packages::safe_path(root, relative)?;
        if fs::read(script).is_ok_and(|bytes| bytes == expected) {
            paths.push(relative.into());
        }
    }
    paths.sort();
    Ok(paths)
}

fn collect_generated_customs(
    root: &Path,
    relative: &str,
    paths: &mut Vec<String>,
) -> Result<(), String> {
    let directory = if relative.is_empty() {
        root.to_owned()
    } else {
        crate::packages::safe_path(root, relative)?
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink()
            || name.contains(".userdb")
            || name.starts_with(".keytao-")
            || matches!(name, "build" | "sync")
        {
            continue;
        }
        let relative = if relative.is_empty() {
            name.to_owned()
        } else {
            format!("{relative}/{name}")
        };
        if kind.is_dir() {
            collect_generated_customs(root, &relative, paths)?;
        } else if kind.is_file() && name.ends_with(".custom.yaml") {
            let path = crate::packages::safe_path(root, &relative)?;
            if is_generated(
                &relative,
                &fs::read(path).map_err(|error| error.to_string())?,
            ) {
                paths.push(relative);
            }
        }
    }
    Ok(())
}

pub(crate) fn write_custom(
    root: &Path,
    relative: &str,
    previous: &str,
    updated: &str,
) -> Result<bool, String> {
    if previous == updated {
        return Ok(false);
    }
    let path = crate::packages::safe_path(root, relative)?;
    if updated.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    } else {
        crate::scheme_files::write_file_atomic(&path, updated.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    Ok(true)
}

pub(crate) fn strip_file(root: &Path, relative: &str) -> Result<bool, String> {
    let path = crate::packages::safe_path(root, relative)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if relative == crate::remove_filter::SCRIPT_PATH {
        if bytes != crate::remove_filter::SCRIPT.as_bytes() {
            return Ok(false);
        }
        fs::remove_file(path).map_err(|error| error.to_string())?;
        return Ok(true);
    }
    if relative == crate::grammar_hint::SCRIPT_PATH {
        if bytes != crate::grammar_hint::SCRIPT.as_bytes() {
            return Ok(false);
        }
        fs::remove_file(path).map_err(|error| error.to_string())?;
        return Ok(true);
    }
    if !relative.ends_with(".custom.yaml") {
        return Ok(false);
    }
    let content = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    write_custom(root, relative, content, &strip(content)?)
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn regenerate(root: &Path, schemas: &BTreeSet<String>) -> Result<bool, String> {
    regenerate_with_options(root, schemas, None)
}

pub(crate) fn regenerate_with_options(
    root: &Path,
    schemas: &BTreeSet<String>,
    grammar_model: Option<&str>,
) -> Result<bool, String> {
    let mut changed = false;
    for path in generated_paths(root)? {
        if path
            .strip_suffix(".custom.yaml")
            .is_some_and(|id| !schemas.contains(id))
        {
            changed |= strip_file(root, &path)?;
        }
    }
    changed |= crate::remove_filter::ensure_for_schemas(root, schemas)?;
    changed |= crate::grammar_hint::ensure_grammar_model_for_schemas(
        root,
        schemas,
        grammar_model,
    )?;
    changed |= crate::grammar_hint::ensure_for_schemas(root, schemas)?;
    changed |= crate::candidate_page_size::apply_for_schemas(
        root,
        crate::ime_settings::read_candidate_page_size(root),
        schemas,
    )?;
    if schemas.contains(crate::addon::EASY_EN_ADDON_ID) {
        let relative = "easy_en.custom.yaml";
        let path = crate::packages::safe_path(root, relative)?;
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.to_string()),
        };
        let updated = set_patch(&content, "english", Some("{easy_en/split_sentence: false}"))?;
        changed |= write_custom(root, relative, &content, &updated)?;
    }
    Ok(changed)
}

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

fn take_section(content: &str) -> Result<(String, BTreeMap<String, String>), String> {
    let mut offset = 0;
    let mut section = None;
    for line in content.split_inclusive('\n') {
        if let Some(metadata) = line.trim_end_matches(['\r', '\n']).strip_prefix(BEGIN) {
            if section.is_some() {
                return Err("Multiple KeyTao generated sections".into());
            }
            let mut fields = metadata.splitn(3, ':');
            let joined: usize = fields
                .next()
                .ok_or("Invalid KeyTao section")?
                .parse()
                .map_err(|_| "Invalid KeyTao separator")?;
            if joined > 2
                || joined > offset
                || !content.is_char_boundary(offset - joined)
                || (joined > 0 && !content[..offset].ends_with('\n'))
            {
                return Err("Invalid KeyTao separator".into());
            }
            let original = decode(fields.next().ok_or("Invalid KeyTao original bytes")?)?;
            let patches =
                serde_json::from_str(&decode(fields.next().ok_or("Invalid KeyTao patches")?)?)
                    .map_err(|error| error.to_string())?;
            section = Some((offset - joined, original, patches));
        } else if line.trim_end_matches(['\r', '\n']) == END {
            let (start, original, patches) =
                section.take().ok_or("Unmatched KeyTao section end")?;
            let mut updated = content.to_owned();
            updated.replace_range(start..offset + line.len(), &original);
            if updated.contains(BEGIN) || updated.lines().any(|line| line == END) {
                return Err("Multiple KeyTao generated sections".into());
            }
            return Ok((updated, patches));
        }
        offset += line.len();
    }
    if section.is_some() {
        return Err("Unterminated KeyTao generated section".into());
    }
    Ok((content.to_owned(), BTreeMap::new()))
}

pub(crate) fn strip(content: &str) -> Result<String, String> {
    let (original, _) = take_section(content)?;
    let original = crate::candidate_page_size::strip_legacy(&original)?;
    let original = crate::remove_filter::strip_legacy(&original)?;
    Ok(crate::addon::strip_legacy_custom(&original))
}

pub(crate) fn strip_legacy_patch(content: &str, key: &str, literal: &str, value: Value) -> String {
    let Ok(mut expected) = parse_custom(content) else {
        return content.to_owned();
    };
    let Some(patch) = expected.get_mut("patch").and_then(Value::as_mapping_mut) else {
        return content.to_owned();
    };
    if patch.get(key) != Some(&value) {
        return content.to_owned();
    }
    patch.remove(key);
    let remove_header = patch.is_empty();
    if remove_header {
        expected.as_mapping_mut().unwrap().remove("patch");
    }

    let mut offset = 0;
    let lines = content
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, offset, line)
        })
        .collect::<Vec<_>>();
    let meaningful = |line: &str| {
        !line.trim().is_empty()
            && !line.trim_start().starts_with(['#', '%'])
            && !matches!(line.trim(), "---" | "...")
    };
    let indent = |line: &str| line.len() - line.trim_start_matches(' ').len();
    let root_indent = lines
        .iter()
        .filter(|(_, _, line)| meaningful(line))
        .map(|(_, _, line)| indent(line))
        .min()
        .unwrap_or(0);
    for (index, &(header_start, header_end, header)) in lines.iter().enumerate() {
        if indent(header) != root_indent
            || !header
                .trim_start_matches(' ')
                .strip_prefix("patch:")
                .is_some_and(|tail| tail.trim().is_empty() || tail.trim_start().starts_with('#'))
        {
            continue;
        }
        let children = lines[index + 1..]
            .iter()
            .take_while(|(_, _, line)| !meaningful(line) || indent(line) > root_indent)
            .collect::<Vec<_>>();
        let Some(child_indent) = children
            .iter()
            .filter(|(_, _, line)| meaningful(line))
            .map(|(_, _, line)| indent(line))
            .min()
        else {
            continue;
        };
        for &&(start, end, line) in &children {
            if indent(line) != child_indent || line.trim() != format!("{key}: {literal}") {
                continue;
            }
            let mut updated = content.to_owned();
            updated.replace_range(start..end, "");
            if remove_header {
                updated.replace_range(header_start..header_end, "");
            }
            if parse_custom(&updated).ok().as_ref() == Some(&expected) {
                return updated;
            }
        }
    }
    content.to_owned()
}

pub(crate) fn set_patch(content: &str, kind: &str, patch: Option<&str>) -> Result<String, String> {
    let (original, mut patches) = take_section(content)?;
    let original = strip(&original)?;
    if let Some(patch) = patch {
        patches.insert(kind.into(), patch.into());
    } else {
        patches.remove(kind);
    }
    if patches.is_empty() {
        return Ok(original);
    }
    render(&original, &patches)
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

fn insert_managed(
    original: &str,
    start: usize,
    end: usize,
    directive: &str,
    patches: &BTreeMap<String, String>,
) -> String {
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
        "{separator}{BEGIN}{joined}:{}:{}{newline}{directive}{newline}{END}{newline}",
        encode(&original[start..end]),
        encode(&serde_json::to_string(patches).unwrap()),
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
    patches: &BTreeMap<String, String>,
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
                    let updated =
                        insert_managed(original, start, end, &format!("  {directive}"), patches);
                    if parse_custom(&updated).ok().as_ref() == Some(expected) {
                        return Some(updated);
                    }
                }
            }
        }
    } else {
        for (end, _) in original.rmatch_indices('}') {
            for separator in [", ", ""] {
                let updated = insert_managed(
                    original,
                    end,
                    end,
                    &format!("  {separator}{directive}"),
                    patches,
                );
                if parse_custom(&updated).ok().as_ref() == Some(expected) {
                    return Some(updated);
                }
            }
        }
    }
    None
}

fn render(original: &str, contributions: &BTreeMap<String, String>) -> Result<String, String> {
    let mut expected = parse_custom(&original)?;
    if expected
        .get("patch")
        .is_some_and(|patch| !patch.is_mapping() && !patch.is_null())
    {
        return Err("Invalid custom patch mapping".into());
    }
    let body = contributions
        .values()
        .map(|patch| {
            patch
                .trim()
                .trim_start_matches('{')
                .trim_end_matches('}')
                .trim()
        })
        .collect::<Vec<_>>()
        .join(", ");
    let literal = format!("{{patch/+: {{{body}}}}}");
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
    if let Some(updated) = patch_flow_root(
        original,
        &expected,
        &flow_directive,
        has_patch,
        contributions,
    ) {
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
    let updated = insert_managed(original, start, end, &directive, contributions);
    // The compiler applies this last directive after user patches without editing them.
    if parse_custom(&updated)? != expected {
        return Err("Cannot safely insert KeyTao override".into());
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_section_restores_package_bytes_after_independent_setting_changes() {
        for original in [
            "# Package bytes\r\npatch: {name: package}\r\n",
            "__patch: existing:/patch # Package directive\npatch: {}\n",
            "{__patch: existing:/patch, patch: {name: package}}",
            "__patch:\n  - existing:/patch\npatch: {}\n",
        ] {
            let page =
                set_patch(original, "candidate-page-size", Some("{menu/page_size: 6}")).unwrap();
            let both = set_patch(
                &page,
                "remove-filter",
                Some("{engine/filters/@next: lua_filter@*keytao_remove_filter}"),
            )
            .unwrap();
            assert_eq!(both.matches(BEGIN).count(), 1);
            assert!(both.contains("menu/page_size: 6"));
            assert!(both.contains("engine/filters/@next"));
            assert_eq!(strip(&both).unwrap().as_bytes(), original.as_bytes());
            let filter = set_patch(&both, "candidate-page-size", None).unwrap();
            assert!(!filter.contains("menu/page_size"));
            assert_eq!(set_patch(&filter, "remove-filter", None).unwrap(), original);
        }
    }

    #[test]
    fn legacy_candidate_filter_and_english_defaults_are_adopted() {
        let legacy = "patch: {name: package}\n__patch: [{patch/+: {menu/page_size: 6}}] # keytao-candidate-page-size v1 0:\n";
        assert_eq!(strip(legacy).unwrap(), "patch: {name: package}\n");
        let converted =
            set_patch(legacy, "candidate-page-size", Some("{menu/page_size: 3}")).unwrap();
        assert!(!converted.contains("keytao-candidate-page-size"));
        assert!(converted.contains(BEGIN));
        assert_eq!(
            strip("patch:\n  engine/filters/@next: lua_filter@*keytao_remove_filter\n").unwrap(),
            ""
        );
        assert_eq!(strip("# KeyTao add-on defaults: no external word-splitter on mobile/desktop.\npatch:\n  easy_en/split_sentence: false\n").unwrap(), "");
    }

    #[test]
    fn switching_active_sets_removes_stale_sections_and_restores_package_customs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for id in ["keytao", "local", "inactive"] {
            fs::write(
                root.join(format!("{id}.schema.yaml")),
                "engine: {filters: [lua_filter@*existing]}\n",
            )
            .unwrap();
        }
        let original = "# Package bytes\r\npatch: {name: package}\r\n";
        fs::write(root.join("keytao.custom.yaml"), original).unwrap();
        crate::ime_settings::set_candidate_page_size(root, Some(5)).unwrap();
        let a = BTreeSet::from(["keytao".to_owned()]);
        let b = BTreeSet::from(["local".to_owned()]);
        assert!(regenerate(root, &a).unwrap());
        let first = fs::read(root.join("keytao.custom.yaml")).unwrap();
        assert!(!regenerate(root, &a).unwrap());
        assert!(regenerate(root, &b).unwrap());
        assert_eq!(
            fs::read(root.join("keytao.custom.yaml")).unwrap(),
            original.as_bytes()
        );
        assert!(!root.join(crate::remove_filter::SCRIPT_PATH).exists());
        assert!(!root.join("inactive.custom.yaml").exists());
        assert!(regenerate(root, &a).unwrap());
        assert!(!root.join("local.custom.yaml").exists());
        assert_eq!(fs::read(root.join("keytao.custom.yaml")).unwrap(), first);
        assert_eq!(
            generated_paths(root).unwrap(),
            ["keytao.custom.yaml", crate::remove_filter::SCRIPT_PATH]
        );
        assert!(regenerate(root, &BTreeSet::new()).unwrap());
        assert_eq!(
            fs::read(root.join("keytao.custom.yaml")).unwrap(),
            original.as_bytes()
        );
        assert!(generated_paths(root).unwrap().is_empty());
    }

    #[test]
    fn active_manifest_excludes_stale_disk_schemas_but_includes_enabled_addons() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["active", "stale", "easy_en"] {
            fs::write(dir.path().join(format!("{id}.schema.yaml")), "schema: {}\n").unwrap();
        }
        fs::write(
            dir.path().join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: stale\n    - schema: easy_en\n",
        )
        .unwrap();
        fs::write(
            dir.path().join(keytao_core::ACTIVE_SCHEME_MANIFEST),
            r#"{"manifest":{"active.schema.yaml":"hash"}}"#,
        )
        .unwrap();
        assert_eq!(
            active_schema_ids(dir.path()).unwrap(),
            BTreeSet::from(["active".into(), "easy_en".into()])
        );
        assert_eq!(
            active_schema_ids_from_default(dir.path()).unwrap(),
            BTreeSet::from(["stale".into(), "easy_en".into()])
        );
    }

    #[test]
    fn legacy_removal_preserves_nested_mapping_headers_and_literal_scalars() {
        let retained = "extra:\n  patch:\n    user_value: kept\nliteral: |\n  patch:\n    engine/filters/@next: lua_filter@*keytao_remove_filter\n";
        let original =
            format!("patch:\n  engine/filters/@next: lua_filter@*keytao_remove_filter\n{retained}");
        assert_eq!(
            crate::remove_filter::strip_legacy(&original).unwrap(),
            retained
        );
        assert_eq!(
            crate::remove_filter::strip_legacy(retained).unwrap(),
            retained
        );
        let retained = "extra:\n  patch:\n    user_value: kept\nliteral: |\n  patch:\n    easy_en/split_sentence: false\n";
        let original = format!(
            "# KeyTao add-on defaults: no external word-splitter on mobile/desktop.\npatch:\n  easy_en/split_sentence: false\n{retained}"
        );
        assert_eq!(crate::addon::strip_legacy_custom(&original), retained);
        assert_eq!(crate::addon::strip_legacy_custom(retained), retained);
    }

    #[test]
    fn nested_schema_sections_are_reported_and_removed_on_switch() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(
            dir.path().join("nested/example.schema.yaml"),
            "schema: {}\n",
        )
        .unwrap();
        crate::ime_settings::set_candidate_page_size(dir.path(), Some(5)).unwrap();
        let active = BTreeSet::from(["nested/example".to_owned()]);
        assert!(regenerate(dir.path(), &active).unwrap());
        assert_eq!(
            generated_paths(dir.path()).unwrap(),
            ["nested/example.custom.yaml"]
        );
        assert!(regenerate(dir.path(), &BTreeSet::new()).unwrap());
        assert!(!dir.path().join("nested/example.custom.yaml").exists());
        assert!(generated_paths(dir.path()).unwrap().is_empty());
    }
}
