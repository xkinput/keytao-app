use std::path::Path;

/// Replace a file's contents in a single step so a concurrent reader never
/// sees a half-written file. The input methods (notably the iOS keyboard
/// extension, which shares these files through the App Group container) poll
/// the shared configuration while the app rewrites it, and a plain
/// `fs::write` truncates before it writes.
///
/// The temporary file is created in the destination directory, so the final
/// `rename` stays inside one filesystem and is therefore atomic.
pub fn write_file_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("keytao");
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let outcome = std::fs::File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(content)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if outcome.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    outcome
}

pub fn yaml_child_mapping<'a>(
    mapping: &'a mut serde_yaml::Mapping,
    key: &str,
    error: &str,
) -> Result<&'a mut serde_yaml::Mapping, String> {
    let value = mapping
        .entry(serde_yaml::Value::String(key.into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    if !matches!(value, serde_yaml::Value::Mapping(_)) {
        *value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    value.as_mapping_mut().ok_or_else(|| error.to_string())
}

pub fn parse_schema_list(content: &str) -> Vec<String> {
    let mut schemas = Vec::new();
    let mut in_list = false;
    for line in content.lines() {
        let t = line.trim();
        if t.contains("schema_list:") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(rest) = t.strip_prefix("- schema:") {
                let s = clean_yaml_scalar(rest);
                if !s.is_empty() {
                    schemas.push(s);
                }
            } else if !t.is_empty() && !t.starts_with('#') && !t.starts_with('-') {
                in_list = false;
            }
        }
    }
    schemas
}

pub fn clean_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        let quote = trimmed.chars().next().unwrap();
        return trimmed[1..]
            .find(quote)
            .map(|end| trimmed[1..1 + end].to_string())
            .unwrap_or_else(|| trimmed[1..].to_string());
    }
    trimmed
        .split_once('#')
        .map_or(trimmed, |(head, _)| head)
        .trim()
        .to_string()
}
