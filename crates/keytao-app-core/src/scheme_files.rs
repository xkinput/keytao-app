pub use keytao_core::{parse_schema_list, write_file_atomic};

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
