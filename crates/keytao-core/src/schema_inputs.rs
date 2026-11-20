use crate::{valid_rime_build_id, write_file_atomic};
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

const RECEIPT_FILE: &str = ".keytao-schema-inputs.json";

/// Save source/output hashes only after the deployment has fully succeeded.
pub fn record_schema_input_fingerprints(root: &Path, deployed: &[String]) -> Result<(), String> {
    record_resolved_schema_input_fingerprints(root, deployed, None)
}

pub(crate) fn record_resolved_schema_input_fingerprints(
    root: &Path,
    deployed: &[String],
    package_shared: Option<&Path>,
) -> Result<(), String> {
    let staging_dir = crate::RuntimeDirs::resolve(root, root)?.staging_dir;
    let path = staging_dir.join(RECEIPT_FILE);
    if let Some(shared) = package_shared {
        let fingerprints: BTreeMap<_, _> = schema_closure(&staging_dir, deployed)
            .into_iter()
            .filter_map(|id| {
                let hash = fingerprint(root, &id, &mut BTreeSet::new(), Some(shared))?;
                Some((id, hash))
            })
            .collect();
        return write_receipt(&path, &fingerprints);
    }
    let raw = raw_fingerprints(root, deployed)?;
    // Android's bundled shared data lives below the user root.
    let shared = root.join("rime-data");
    let mut fingerprints = read_receipt(root);
    fingerprints.retain(|id, saved| {
        if let Some(id) = id.strip_prefix("android:") {
            fingerprint(root, id, &mut BTreeSet::new(), Some(&shared)) == Some(*saved)
        } else {
            raw.get(id) == Some(&Some(*saved))
        }
    });
    for id in schema_closure(&staging_dir, deployed) {
        if let Some(Some(value)) = raw.get(&id) {
            fingerprints.insert(id.clone(), *value);
        }
        if let Some(value) = fingerprint(root, &id, &mut BTreeSet::new(), Some(&shared)) {
            fingerprints.insert(format!("android:{id}"), value);
        }
    }
    write_receipt(&path, &fingerprints)
}

fn write_receipt(path: &Path, fingerprints: &BTreeMap<String, [u8; 32]>) -> Result<(), String> {
    let bytes = serde_json::to_vec(fingerprints).map_err(|error| error.to_string())?;
    if fs::read(path).ok().as_deref() != Some(&bytes) {
        write_file_atomic(path, &bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn invalidate_changed_deploy_schemas(
    root: &Path,
    selected: &[String],
) -> Result<(), String> {
    let trusted = read_receipt(root);
    let raw = raw_fingerprints(root, selected)?;
    // A new source/custom patch can introduce dependencies absent from old compiled YAML.
    let changed = raw
        .into_iter()
        .filter(|(id, value)| {
            value
                .as_ref()
                .is_some_and(|value| trusted.get(id) != Some(value))
        })
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    crate::invalidate_rime_build_artifacts(root, &changed, &[])?;
    Ok(())
}

fn schema_closure(staging_dir: &Path, selected: &[String]) -> BTreeSet<String> {
    let mut pending: BTreeSet<_> = selected.iter().cloned().collect();
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop_first() {
        if !valid_rime_build_id(&id) || !visited.insert(id.clone()) {
            continue;
        }
        let compiled = fs::read(staging_dir.join(format!("{id}.schema.yaml")))
            .ok()
            .and_then(|bytes| serde_yaml::from_slice::<Value>(&bytes).ok());
        if let Some(dependencies) = compiled
            .as_ref()
            .and_then(|value| value["schema"]["dependencies"].as_sequence())
        {
            pending.extend(
                dependencies
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    visited
}

/// Fingerprint each compiled schema's source closure; `None` requires rebuilding.
pub fn schema_input_fingerprints(
    root: &Path,
    schema_ids: &[String],
) -> Result<BTreeMap<String, Option<[u8; 32]>>, String> {
    let trusted = read_receipt(root);
    Ok(raw_fingerprints(root, schema_ids)?
        .into_iter()
        .map(|(id, value)| {
            let value = value.filter(|value| trusted.get(&id) == Some(value));
            (id, value)
        })
        .collect())
}

fn read_receipt(root: &Path) -> BTreeMap<String, [u8; 32]> {
    let Ok(dirs) = crate::RuntimeDirs::resolve(root, root) else {
        return BTreeMap::new();
    };
    fs::read(dirs.staging_dir.join(RECEIPT_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn schema_input_is_current(root: &Path, id: &str, shared: &Path) -> bool {
    fingerprint(root, id, &mut BTreeSet::new(), Some(shared))
        .is_some_and(|value| read_receipt(root).get(&format!("android:{id}")) == Some(&value))
}

fn raw_fingerprints(
    root: &Path,
    schema_ids: &[String],
) -> Result<BTreeMap<String, Option<[u8; 32]>>, String> {
    let mut ids: BTreeSet<_> = schema_ids.iter().cloned().collect();
    match fs::read_dir(crate::RuntimeDirs::resolve(root, root)?.staging_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".schema.yaml"))
                {
                    ids.insert(id.to_owned());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(ids
        .into_iter()
        .map(|id| {
            let fingerprint = fingerprint(root, &id, &mut BTreeSet::new(), None);
            (id, fingerprint)
        })
        .collect())
}

fn fingerprint(
    root: &Path,
    id: &str,
    visited: &mut BTreeSet<String>,
    shared: Option<&Path>,
) -> Option<[u8; 32]> {
    if !valid_rime_build_id(id) || !visited.insert(id.into()) {
        return None;
    }
    let compiled_path = crate::RuntimeDirs::resolve(root, shared.unwrap_or(root))
        .ok()?
        .staging_dir
        .join(format!("{id}.schema.yaml"));
    let compiled_metadata = fs::symlink_metadata(&compiled_path).ok()?;
    if !compiled_metadata.is_file() {
        return None;
    }
    let compiled_bytes = fs::read(&compiled_path).ok()?;
    let compiled: Value = serde_yaml::from_slice(&compiled_bytes).ok()?;
    if compiled["schema"]["schema_id"].as_str()? != id {
        return None;
    }
    let timestamps = compiled["__build_info"]["timestamps"].as_mapping()?;
    if !timestamps.contains_key(Value::String(format!("{id}.schema"))) {
        return None;
    }
    let mut paths = BTreeSet::from([format!("{id}.schema.yaml"), format!("{id}.custom.yaml")]);
    for (name, stamp) in timestamps {
        let name = name.as_str()?;
        if !valid_rime_build_id(name) || stamp.as_u64().is_none() {
            return None;
        }
        if stamp.as_u64()? != 0
            && !root.join(format!("{name}.yaml")).is_file()
            && !shared.is_some_and(|shared| shared.join(format!("{name}.yaml")).is_file())
        {
            return None;
        }
        paths.insert(format!("{name}.yaml"));
        if name == "default" || name == "default.custom" {
            paths.insert("default.custom.yaml".into());
            paths.insert("default-custom.yaml".into());
        }
    }
    let mut hash = Sha256::new();
    if shared.is_some() {
        // Keep Android deploy trust separate from the installer's source comparison.
        paths.extend([
            "default.yaml".into(),
            "default.custom.yaml".into(),
            "default-custom.yaml".into(),
        ]);
        hash.update(b"keytao-android-schema-inputs/v1");
        hash.update(crate::librime_runtime_version()?.as_bytes());
    } else {
        hash.update(b"keytao-schema-inputs/v1");
    }
    hash.update((compiled_bytes.len() as u64).to_le_bytes());
    hash.update(compiled_bytes);
    for name in paths {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        let mut path = root.join(&name);
        if let Some(shared) = shared {
            if fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                path = shared.join(&name);
            }
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => return None,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        match fs::read(path) {
            Ok(bytes) => {
                hash.update([1]);
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
            Err(_) => return None,
        }
    }
    if !compiled["schema"]["dependencies"].is_null() {
        for dependency in compiled["schema"]["dependencies"].as_sequence()? {
            hash.update(fingerprint(root, dependency.as_str()?, visited, shared)?);
        }
    }
    visited.remove(id);
    Some(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    const RECEIPT: &str = "build/.keytao-schema-inputs.json";
    use super::*;
    use crate::{invalidate_rime_build_artifacts, DeployProcess};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "keytao-schema-inputs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("build")).unwrap();
            fs::write(root.join("shared.yaml"), b"value: 1\n").unwrap();
            for id in ["one", "two"] {
                fs::write(
                    root.join(format!("{id}.schema.yaml")),
                    format!("schema: {{schema_id: {id}}}\n"),
                )
                .unwrap();
                fs::write(root.join(format!("build/{id}.schema.yaml")), format!("__build_info:\n  timestamps: {{{id}.schema: 1, shared: 1, default.custom: 0}}\nschema: {{schema_id: {id}}}\n")).unwrap();
                fs::write(root.join(format!("build/{id}.prism.bin")), b"prism").unwrap();
            }
            fs::write(root.join("one.user.dict.yaml"), b"keep words").unwrap();
            fs::create_dir_all(root.join("one.userdb")).unwrap();
            fs::write(root.join("one.userdb/data"), b"keep database").unwrap();
            record_schema_input_fingerprints(&root, &["one".into(), "two".into()]).unwrap();
            Self(root)
        }

        fn changed(&self, before: &BTreeMap<String, Option<[u8; 32]>>) -> Vec<String> {
            let after = schema_input_fingerprints(&self.0, &[]).unwrap();
            before
                .iter()
                .filter(|(id, value)| value.is_none() || after.get(*id) != Some(*value))
                .map(|(id, _)| id.clone())
                .collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn user_dictionary_cache_only_skips_exiting_deploy_processes() {
        assert!(DeployProcess::InProcess.needs_user_dictionary_cache());
        for process in [
            DeployProcess::DeployChild,
            DeployProcess::AndroidDeployer,
            DeployProcess::PublicDeploy,
        ] {
            assert!(!process.needs_user_dictionary_cache());
        }
    }

    #[test]
    fn changed_schema_invalidates_exactly_its_build_and_preserves_user_data() {
        let fixture = Fixture::new();
        let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        assert!(fixture.changed(&before).is_empty());
        fs::write(
            fixture.0.join("one.schema.yaml"),
            b"schema: {schema_id: one, version: 2}\n",
        )
        .unwrap();
        let changed = fixture.changed(&before);
        assert_eq!(changed, ["one"]);
        assert_eq!(
            invalidate_rime_build_artifacts(&fixture.0, &changed, &[]).unwrap(),
            ["build/one.prism.bin", "build/one.schema.yaml"]
        );
        assert!(fixture.0.join("build/two.prism.bin").exists());
        assert_eq!(
            fs::read(fixture.0.join("one.user.dict.yaml")).unwrap(),
            b"keep words"
        );
        assert_eq!(
            fs::read(fixture.0.join("one.userdb/data")).unwrap(),
            b"keep database"
        );
    }

    #[test]
    fn includes_default_custom_and_new_schema_custom_invalidate_their_consumers() {
        for (name, expected) in [
            ("shared.yaml", vec!["one", "two"]),
            ("default.custom.yaml", vec!["one", "two"]),
            ("default-custom.yaml", vec!["one", "two"]),
            ("one.custom.yaml", vec!["one"]),
        ] {
            let fixture = Fixture::new();
            let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
            fs::write(fixture.0.join(name), b"patch: {value: 2}\n").unwrap();
            assert_eq!(fixture.changed(&before), expected, "{name}");
        }
    }

    #[test]
    fn listed_schema_reuse_requires_a_matching_receipt_and_resolved_inputs() {
        let fixture = Fixture::new();
        assert!(schema_input_is_current(&fixture.0, "one", &fixture.0));
        fs::write(fixture.0.join("shared.yaml"), b"value: 2\n").unwrap();
        assert!(!schema_input_is_current(&fixture.0, "one", &fixture.0));
        record_schema_input_fingerprints(&fixture.0, &["one".into()]).unwrap();
        assert!(schema_input_is_current(&fixture.0, "one", &fixture.0));
        fs::remove_file(fixture.0.join("shared.yaml")).unwrap();
        assert!(!schema_input_is_current(&fixture.0, "one", &fixture.0));
        assert!(!schema_input_is_current(&fixture.0, "missing", &fixture.0));
        assert!(!schema_input_is_current(
            &fixture.0,
            "../outside",
            &fixture.0
        ));
        let fixture = Fixture::new();
        fs::remove_file(fixture.0.join(RECEIPT)).unwrap();
        assert!(!schema_input_is_current(&fixture.0, "one", &fixture.0));
    }

    #[test]
    fn android_defaults_do_not_change_installer_fingerprints_or_trust_legacy_receipts() {
        let fixture = Fixture::new();
        let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        fs::write(fixture.0.join("default.yaml"), b"menu: {page_size: 9}\n").unwrap();
        assert_eq!(schema_input_fingerprints(&fixture.0, &[]).unwrap(), before);
        assert!(!schema_input_is_current(&fixture.0, "one", &fixture.0));
        record_schema_input_fingerprints(&fixture.0, &["one".into()]).unwrap();
        assert!(schema_input_is_current(&fixture.0, "one", &fixture.0));
        assert!(!schema_input_is_current(&fixture.0, "two", &fixture.0));
        let mut legacy = read_receipt(&fixture.0);
        legacy.retain(|id, _| !id.starts_with("android:"));
        fs::write(
            fixture.0.join(RECEIPT),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        assert_eq!(schema_input_fingerprints(&fixture.0, &[]).unwrap(), before);
        assert!(!schema_input_is_current(&fixture.0, "one", &fixture.0));
    }

    #[test]
    fn changed_dependency_invalidates_parent_and_unknown_build_requires_rebuild() {
        let fixture = Fixture::new();
        let compiled = fixture.0.join("build/one.schema.yaml");
        fs::write(&compiled, b"__build_info: {timestamps: {one.schema: 1}}\nschema: {schema_id: one, dependencies: [two]}\n").unwrap();
        record_schema_input_fingerprints(&fixture.0, &["one".into()]).unwrap();
        let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        assert!(before["two"].is_some());
        fs::write(fixture.0.join("two.custom.yaml"), b"patch: {value: 2}\n").unwrap();
        assert_eq!(fixture.changed(&before), ["one", "two"]);
        let fixture = Fixture::new();
        let compiled = fixture.0.join("build/one.schema.yaml");
        fs::write(&compiled, b"schema: {schema_id: one}\n").unwrap();
        let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        assert_eq!(fixture.changed(&before), ["one"]);
    }

    #[test]
    fn prior_same_mtime_custom_edit_cannot_reuse_a_later_touched_build() {
        let fixture = Fixture::new();
        let custom = fixture.0.join("one.custom.yaml");
        fs::write(&custom, b"patch: {value: 1}\n").unwrap();
        record_schema_input_fingerprints(&fixture.0, &["one".into(), "two".into()]).unwrap();
        let modified = fs::metadata(&custom).unwrap().modified().unwrap();
        fs::File::options()
            .write(true)
            .open(fixture.0.join("build/one.schema.yaml"))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(modified + std::time::Duration::from_secs(60)),
            )
            .unwrap();
        fs::write(&custom, b"patch: {value: 2}\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&custom)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        assert_eq!(fixture.changed(&before), ["one"]);
        assert!(before["one"].is_none());
        assert!(before["two"].is_some());
    }

    #[test]
    fn missing_receipt_nonzero_missing_include_or_modified_output_require_rebuild() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.0.join(RECEIPT)).unwrap();
        assert!(schema_input_fingerprints(&fixture.0, &[])
            .unwrap()
            .values()
            .all(Option::is_none));
        record_schema_input_fingerprints(&fixture.0, &["one".into(), "two".into()]).unwrap();
        fs::write(fixture.0.join(RECEIPT), b"invalid receipt").unwrap();
        assert!(schema_input_fingerprints(&fixture.0, &[])
            .unwrap()
            .values()
            .all(Option::is_none));
        record_schema_input_fingerprints(&fixture.0, &["one".into(), "two".into()]).unwrap();
        let path = fixture.0.join("build/one.schema.yaml");
        let original = fs::read(&path).unwrap();
        fs::write(
            &path,
            [original.as_slice(), b"# changed compiled output\n"].concat(),
        )
        .unwrap();
        assert!(schema_input_fingerprints(&fixture.0, &[]).unwrap()["one"].is_none());
        fs::write(&path, b"__build_info: {timestamps: {one.schema: 1, absent_include: 1}}\nschema: {schema_id: one}\n").unwrap();
        record_schema_input_fingerprints(&fixture.0, &["one".into(), "two".into()]).unwrap();
        let fingerprints = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        assert!(fingerprints["one"].is_none());
        assert!(fingerprints["two"].is_some());
    }

    #[test]
    fn deploying_one_schema_never_certifies_changed_inactive_builds() {
        let fixture = Fixture::new();
        record_schema_input_fingerprints(&fixture.0, &["one".into()]).unwrap();
        assert!(schema_input_fingerprints(&fixture.0, &[]).unwrap()["two"].is_some());
        fs::write(fixture.0.join("two.custom.yaml"), b"patch: {value: 2}\n").unwrap();
        record_schema_input_fingerprints(&fixture.0, &["one".into()]).unwrap();
        let before = schema_input_fingerprints(&fixture.0, &[]).unwrap();
        assert!(before["one"].is_some());
        assert!(before["two"].is_none());
        assert!(schema_input_is_current(&fixture.0, "one", &fixture.0));
        assert!(!schema_input_is_current(&fixture.0, "two", &fixture.0));
        assert_eq!(fixture.changed(&before), ["two"]);
        fs::remove_file(fixture.0.join(RECEIPT)).unwrap();
        fs::create_dir(fixture.0.join(RECEIPT)).unwrap();
        assert!(record_schema_input_fingerprints(&fixture.0, &["one".into()]).is_err());
        assert!(schema_input_fingerprints(&fixture.0, &[])
            .unwrap()
            .values()
            .all(Option::is_none));
        assert!(fixture.0.join("build/one.prism.bin").exists());
        assert!(fixture.0.join("build/two.prism.bin").exists());
    }

    #[test]
    fn deploy_preflight_rebuilds_untrusted_local_inputs_without_touching_trusted_or_unresolved_builds(
    ) {
        let fixture = Fixture::new();
        invalidate_changed_deploy_schemas(&fixture.0, &["one".into()]).unwrap();
        assert!(fixture.0.join("build/one.schema.yaml").exists());
        assert!(fixture.0.join("build/two.schema.yaml").exists());
        fs::write(fixture.0.join("two.custom.yaml"), b"patch: {value: 2}\n").unwrap();
        invalidate_changed_deploy_schemas(&fixture.0, &["one".into()]).unwrap();
        assert!(fixture.0.join("build/one.schema.yaml").exists());
        assert!(!fixture.0.join("build/two.schema.yaml").exists());
        fs::write(
            fixture.0.join("one.custom.yaml"),
            b"patch: {schema/dependencies: [two]}\n",
        )
        .unwrap();
        invalidate_changed_deploy_schemas(&fixture.0, &["one".into()]).unwrap();
        assert!(!fixture.0.join("build/one.schema.yaml").exists());
        assert!(!fixture.0.join("build/one.prism.bin").exists());
        assert!(!fixture.0.join("build/two.schema.yaml").exists());

        let fixture = Fixture::new();
        fs::remove_file(fixture.0.join(RECEIPT)).unwrap();
        invalidate_changed_deploy_schemas(&fixture.0, &["one".into()]).unwrap();
        assert!(!fixture.0.join("build/one.schema.yaml").exists());
        assert!(!fixture.0.join("build/two.schema.yaml").exists());
        fs::write(fixture.0.join("build/one.schema.yaml"), b"__build_info: {timestamps: {one.schema: 1, shared_missing: 1}}\nschema: {schema_id: one}\n").unwrap();
        invalidate_changed_deploy_schemas(&fixture.0, &["one".into()]).unwrap();
        assert!(fixture.0.join("build/one.schema.yaml").exists());
        record_schema_input_fingerprints(&fixture.0, &["one".into()]).unwrap();
        assert!(schema_input_fingerprints(&fixture.0, &[]).unwrap()["one"].is_none());
    }
}
