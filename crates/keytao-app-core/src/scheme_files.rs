pub use keytao_core::write_file_atomic;

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
