use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use keytao_core::LINUX_UPGRADE_DEPLOY_SUBCOMMAND as SUBCOMMAND;
const VERSION_STAMP: &str = "keytao-librime.version";

#[derive(Debug, PartialEq, Eq)]
enum UpgradeDecision {
    Current,
    RecordVersion,
    Deploy,
}

fn upgrade_decision(
    recorded_version: Option<&str>,
    runtime_version: &str,
    has_deployed_data: bool,
) -> UpgradeDecision {
    if recorded_version.map(str::trim) == Some(runtime_version) {
        UpgradeDecision::Current
    } else if has_deployed_data {
        UpgradeDecision::Deploy
    } else {
        UpgradeDecision::RecordVersion
    }
}

fn upgrade_context() -> Result<(PathBuf, String), String> {
    let user_dir = keytao_core::default_user_data_dir()
        .ok_or("cannot determine the librime user data directory")?;
    let version = keytao_core::librime_runtime_version()
        .ok_or("cannot determine the librime runtime version")?;
    Ok((user_dir, version))
}

fn decision_for(user_dir: &Path, version: &str) -> Result<UpgradeDecision, String> {
    let dirs = keytao_core::RuntimeDirs::resolve(user_dir, user_dir);
    let recorded_version = match std::fs::read_to_string(user_dir.join(VERSION_STAMP)) {
        Ok(version) => Some(version),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read librime version stamp: {error}")),
    };
    let has_schemes = [
        dirs.user_data_dir.join("default.custom.yaml"),
        dirs.user_data_dir.join("default-custom.yaml"),
        dirs.staging_dir.join("default.yaml"),
        dirs.user_data_dir.join("default.yaml"),
    ]
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .map(|content| keytao_core::parse_schema_list(&content))
    .any(|schemas| !schemas.is_empty());
    let has_deployed_data = has_schemes && keytao_core::has_schemas(&dirs.staging_dir);
    Ok(upgrade_decision(
        recorded_version.as_deref(),
        version,
        has_deployed_data,
    ))
}

fn record_version(user_dir: &Path, version: &str) -> Result<(), String> {
    std::fs::write(user_dir.join(VERSION_STAMP), format!("{version}\n"))
        .map_err(|error| format!("cannot write librime version stamp: {error}"))
}

pub struct UpgradeDeploy {
    lock: File,
}

/// Hold the lock through startup so concurrent backend processes launch one child.
pub fn prepare() -> Result<Option<UpgradeDeploy>, String> {
    let (user_dir, version) = upgrade_context()?;
    let Some(lock) = keytao_core::linux_deploy_lock(&user_dir, std::time::Duration::ZERO)? else {
        return Ok(None);
    };
    match decision_for(&user_dir, &version)? {
        UpgradeDecision::Current => Ok(None),
        UpgradeDecision::RecordVersion => {
            record_version(&user_dir, &version)?;
            Ok(None)
        }
        UpgradeDecision::Deploy => Ok(Some(UpgradeDeploy { lock })),
    }
}

impl UpgradeDeploy {
    /// Called only after the daemon has installed its reload stamp watcher.
    pub fn spawn(self) {
        if let Err(error) = std::thread::Builder::new()
            .name("librime-upgrade".into())
            .spawn(move || {
                let result = (|| {
                    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
                    let mut command = Command::new(exe);
                    command
                        .arg(SUBCOMMAND)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null());
                    keytao_core::deploy_child::inherit_linux_deploy_lock(&mut command, &self.lock);
                    let mut child = command.spawn().map_err(|error| error.to_string())?;
                    tracing::info!(pid = child.id(), "librime upgrade deployment started");
                    let status = child.wait().map_err(|error| error.to_string())?;
                    if status.success() {
                        tracing::info!("librime upgrade deployment finished");
                        Ok(())
                    } else {
                        Err(format!("librime upgrade child exited with {status}"))
                    }
                })();
                if let Err(error) = result {
                    tracing::warn!(
                        "librime upgrade deployment failed: {error}; will retry on next start"
                    );
                }
                drop(self.lock);
            })
        {
            tracing::warn!("cannot start librime upgrade deployment: {error}");
        }
    }
}

pub fn run_child() -> Result<(), String> {
    let _lock = keytao_core::deploy_child::inherited_linux_deploy_lock()?;
    let (user_dir, version) = upgrade_context()?;
    if decision_for(&user_dir, &version)? == UpgradeDecision::Deploy {
        keytao_core::deploy_for_upgrade(
            user_dir.to_string_lossy().into_owned(),
            keytao_core::default_shared_data_dir(),
        )?;
        keytao_core::ReloadStamp::write(&user_dir)?;
    }
    record_version(&user_dir, &version)
}

#[cfg(test)]
mod tests {
    use super::{upgrade_decision, UpgradeDecision};

    #[test]
    fn upgrade_decision_requires_changed_version_and_deployed_data() {
        for (recorded, deployed, expected) in [
            (Some("1.17.0\n"), true, UpgradeDecision::Current),
            (Some("1.10.0"), true, UpgradeDecision::Deploy),
            (Some("1.10.0"), false, UpgradeDecision::RecordVersion),
            (None, true, UpgradeDecision::Deploy),
            (None, false, UpgradeDecision::RecordVersion),
        ] {
            assert_eq!(upgrade_decision(recorded, "1.17.0", deployed), expected);
        }
    }
}
