use super::*;

/// Use librime's snapshot APIs with the app engine idle. The callback may
/// lock and stage closed databases before passing the private copies to levers.
pub fn with_userdb_migration<T>(
    action: impl FnOnce(&UserdbMigration) -> Result<T, String>,
) -> Result<T, String> {
    let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
    let _rime = rime_api_lock();
    if *lock_ignore_poison(&RIME_INITIALIZED) {
        return Err("Close the in-process Rime engine before migrating user dictionaries".into());
    }
    unsafe {
        let api = rime_get_api();
        let finalize =
            rime_api_member!(api, finalize).ok_or("librime finalize API is unavailable")?;
        let initialize = rime_api_member!(api, deployer_initialize)
            .ok_or("librime deployer initialization API is unavailable")?;
        require_levers_module();
        if let Some(setup) = rime_api_member!(api, setup) {
            setup(std::ptr::null_mut());
        }
        let migration = UserdbMigration {
            api,
            initialize,
            finalize,
            configured: Cell::new(false),
        };
        action(&migration)
    }
}

pub struct UserdbMigration {
    api: *const RimeApi,
    initialize: unsafe extern "C" fn(*mut librime_sys::RimeTraits),
    finalize: unsafe extern "C" fn(),
    configured: Cell<bool>,
}

impl Drop for UserdbMigration {
    fn drop(&mut self) {
        if self.configured.get() {
            unsafe { (self.finalize)() };
        }
    }
}

impl UserdbMigration {
    fn set_runtime(&self, dirs: &RuntimeDirs) {
        // librime components cache resource resolvers for the previous dirs.
        if self.configured.replace(true) {
            unsafe { (self.finalize)() };
        }
        dirs.with_rime_traits(|mut traits| unsafe { (self.initialize)(&mut traits) });
    }

    pub fn dictionary_names(
        &self,
        dirs: &RuntimeDirs,
        schemas: &[String],
    ) -> Result<HashSet<String>, String> {
        self.set_runtime(dirs);
        let mut names = HashSet::new();
        let deploy = unsafe { rime_api_member!(self.api, deploy_config_file) }
            .ok_or("librime config deployment API is unavailable")?;
        for id in schemas {
            let schema_file = format!("{id}.schema.yaml");
            let filename = CString::new(schema_file.as_str()).map_err(|error| error.to_string())?;
            if unsafe { deploy(filename.as_ptr(), c"schema/version".as_ptr()) } == 0
                || !dirs.staging_dir.join(&schema_file).is_file()
            {
                return Err(format!(
                    "Could not resolve user dictionaries for schema {id}"
                ));
            }
            names.extend(schema_user_dictionary_names_locked(id));
        }
        Ok(names)
    }

    pub fn backup(&self, source: &Path, sync: &Path, name: &str) -> Result<PathBuf, String> {
        self.set_runtime(&RuntimeDirs::flat(source, source));
        let dictionary = CString::new(name).map_err(|error| error.to_string())?;
        let sync_path = sync.to_str().ok_or("Invalid private snapshot directory")?;
        unsafe {
            let run =
                rime_api_member!(self.api, run_task).ok_or("librime task API is unavailable")?;
            let get_sync = rime_api_member!(self.api, get_sync_dir)
                .ok_or("librime sync directory API is unavailable")?;
            if run(c"installation_update".as_ptr()) == 0
                || cstr_to_str(get_sync()).as_deref() != Some(sync_path)
            {
                return Err("Could not verify librime's private snapshot directory".into());
            }
            let levers = user_dictionary_api_locked()?;
            let backup = (*levers)
                .backup_user_dict
                .ok_or("librime user dictionary backup API is unavailable")?;
            if backup(dictionary.as_ptr()) == 0 {
                return Err(format!("Could not back up user dictionary {name}"));
            }
        }
        let snapshot = sync
            .join("keytao-migration")
            .join(format!("{name}.userdb.txt"));
        if !snapshot.is_file() {
            return Err(format!(
                "Missing private snapshot for user dictionary {name}"
            ));
        }
        Ok(snapshot)
    }

    pub fn restore(&self, target: &Path, name: &str, snapshot: &Path) -> Result<(), String> {
        let file = std::fs::File::open(snapshot).map_err(|error| error.to_string())?;
        use std::io::BufRead;
        let expected = format!("{name}.userdb");
        let mut matched = false;
        for line in std::io::BufReader::new(file).lines() {
            let line = line.map_err(|error| error.to_string())?;
            if let Some(db_name) = line.strip_prefix("#@/db_name\t") {
                matched = db_name == expected || db_name == name;
                break;
            }
            if !line.starts_with('#') && !line.is_empty() {
                break;
            }
        }
        if !matched {
            return Err(format!(
                "Snapshot contains an unexpected dictionary name for {name}"
            ));
        }
        let snapshot = CString::new(snapshot.to_str().ok_or("Invalid private snapshot path")?)
            .map_err(|error| error.to_string())?;
        self.set_runtime(&RuntimeDirs::flat(target, target));
        unsafe {
            let levers = user_dictionary_api_locked()?;
            let restore = (*levers)
                .restore_user_dict
                .ok_or("librime user dictionary restore API is unavailable")?;
            if restore(snapshot.as_ptr()) == 0 {
                return Err(format!("Could not restore user dictionary {name}"));
            }
        }
        Ok(())
    }
}
