pub use keytao_core::parse_schema_list;

pub fn write_file_atomic(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    crate::package_fs::retry_windows(|| {
        #[cfg(target_os = "windows")]
        crate::package_fs::clear_readonly(path)?;
        keytao_core::write_file_atomic(path, content)
    })
    .map_err(|error| crate::package_fs::context("写入文件", path, error))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn atomic_write_replaces_readonly_target_with_writable_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.schema.yaml");
        fs::write(&path, b"previous").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();

        write_file_atomic(&path, b"replacement").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert!(!fs::metadata(&path).unwrap().permissions().readonly());
    }

    #[test]
    fn atomic_write_error_includes_operation_and_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing/example.schema.yaml");
        let error = write_file_atomic(&path, b"replacement").unwrap_err();
        assert!(error.to_string().contains("写入文件"));
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }
}
