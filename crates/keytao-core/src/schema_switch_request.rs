use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

pub const SCHEMA_SWITCH_REQUEST_FILE: &str = ".keytao-schema-switches.json";
pub const SCHEMA_SWITCH_APPLIED_FILE: &str = ".keytao-schema-switches.applied";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaSwitchRequest {
    pub generation: u64,
    pub options: BTreeMap<String, bool>,
}

pub fn read_schema_switch_request(root: &Path) -> Result<SchemaSwitchRequest, String> {
    match std::fs::read(root.join(SCHEMA_SWITCH_REQUEST_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("read schema switch request: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SchemaSwitchRequest::default())
        }
        Err(error) => Err(format!("read schema switch request: {error}")),
    }
}

pub fn schema_switch_applied_generation(root: &Path) -> Result<u64, String> {
    match std::fs::read_to_string(root.join(SCHEMA_SWITCH_APPLIED_FILE)) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|error| format!("read applied schema switch generation: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(format!("read applied schema switch generation: {error}")),
    }
}

pub(crate) fn pending(root: &Path) -> Result<Option<SchemaSwitchRequest>, String> {
    let request = read_schema_switch_request(root)?;
    Ok((request.generation > schema_switch_applied_generation(root)?).then_some(request))
}

#[derive(Default)]
struct ProcessSwitchState {
    generations: BTreeMap<PathBuf, u64>,
}

// Keep each host's baseline across librime rebuilds; another host's ack is not ours.
static PROCESS_SWITCH_STATE: LazyLock<Mutex<ProcessSwitchState>> =
    LazyLock::new(|| Mutex::new(ProcessSwitchState::default()));

pub(crate) fn pending_for_process(root: &Path) -> Result<Option<SchemaSwitchRequest>, String> {
    crate::lock_ignore_poison(&PROCESS_SWITCH_STATE).pending(root)
}

pub(crate) fn acknowledge(root: &Path, generation: u64) -> Result<(), String> {
    crate::lock_ignore_poison(&PROCESS_SWITCH_STATE).acknowledge(root, generation)
}

impl ProcessSwitchState {
    fn pending(&mut self, root: &Path) -> Result<Option<SchemaSwitchRequest>, String> {
        let request = read_schema_switch_request(root)?;
        let applied = match self.generations.entry(root.to_path_buf()) {
            std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                *entry.insert(schema_switch_applied_generation(root)?)
            }
        };
        Ok((request.generation > applied).then_some(request))
    }

    fn acknowledge(&mut self, root: &Path, generation: u64) -> Result<(), String> {
        write_file_atomic(
            &root.join(SCHEMA_SWITCH_APPLIED_FILE),
            generation.to_string().as_bytes(),
        )
        .map_err(|error| format!("save applied schema switch generation: {error}"))?;
        let applied = self.generations.entry(root.to_path_buf()).or_default();
        *applied = (*applied).max(generation);
        Ok(())
    }
}

pub(crate) fn merge(
    root: &Path,
    values: impl IntoIterator<Item = (String, bool)>,
) -> Result<(), String> {
    let mut request = read_schema_switch_request(root)?;
    request.generation = request
        .generation
        .max(schema_switch_applied_generation(root)?)
        .checked_add(1)
        .ok_or("schema switch generation exhausted")?;
    request.options.extend(values);
    let bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    write_file_atomic(&root.join(SCHEMA_SWITCH_REQUEST_FILE), &bytes)
        .map_err(|error| format!("save schema switch request: {error}"))
}

/// Replace a shared file without exposing partially written contents.
/// Keep the temporary file beside the destination so rename is atomic.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_processes_each_apply_a_shared_generation_once() {
        let root =
            std::env::temp_dir().join(format!("keytao-switch-processes-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut first = ProcessSwitchState::default();
        let mut second = ProcessSwitchState::default();

        merge(&root, [("mars".into(), true)]).unwrap();
        assert_eq!(first.pending(&root).unwrap().unwrap().generation, 1);
        assert_eq!(second.pending(&root).unwrap().unwrap().generation, 1);
        first.acknowledge(&root, 1).unwrap();
        second.acknowledge(&root, 1).unwrap();
        let mut idle = ProcessSwitchState::default();
        assert!(idle.pending(&root).unwrap().is_none());

        merge(&root, [("mars".into(), false)]).unwrap();
        let request = first.pending(&root).unwrap().unwrap();
        assert_eq!(request.generation, 2);
        assert!(!request.options["mars"]);
        first.acknowledge(&root, request.generation).unwrap();
        assert_eq!(schema_switch_applied_generation(&root).unwrap(), 2);

        // Another host's acknowledgment must not hide this process's update.
        let request = second.pending(&root).unwrap().unwrap();
        assert_eq!(request.generation, 2);
        assert!(!request.options["mars"]);
        second.acknowledge(&root, request.generation).unwrap();
        assert_eq!(idle.pending(&root).unwrap().unwrap().generation, 2);
        assert!(first.pending(&root).unwrap().is_none());
        assert!(second.pending(&root).unwrap().is_none());

        let mut fresh = ProcessSwitchState::default();
        assert!(fresh.pending(&root).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_baselines_are_per_root_and_recorded_before_the_first_request() {
        let root = std::env::temp_dir().join(format!("keytao-switch-roots-{}", std::process::id()));
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let mut process = ProcessSwitchState::default();
        assert!(process.pending(&root).unwrap().is_none());
        assert!(process.pending(&other).unwrap().is_none());

        merge(&root, [("mars".into(), false)]).unwrap();
        merge(&other, [("mars".into(), true)]).unwrap();
        process.acknowledge(&root, 1).unwrap();
        ProcessSwitchState::default()
            .acknowledge(&other, 1)
            .unwrap();
        let request = process.pending(&other).unwrap().unwrap();
        assert_eq!(request.generation, 1);
        assert!(request.options["mars"]);
        assert!(process.pending(&root).unwrap().is_none());

        process.acknowledge(&other, 1).unwrap();
        assert!(process.pending(&other).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
