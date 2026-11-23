use keytao_core::{list_schema_switches, release_idle_rime_config, set_schema_switch};
use librime_sys::{rime_get_api, RimeTraits};
use std::ffi::{CStr, CString};
use std::path::Path;

fn write_config(dir: &Path) {
    std::fs::create_dir_all(dir.join("build")).unwrap();
    std::fs::write(
        dir.join("build/default.yaml"),
        "schema_list:\n  - schema: keytao_foreign_config\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("build/keytao_foreign_config.schema.yaml"),
        "schema:\n  schema_id: keytao_foreign_config\nswitches:\n  - name: mars\n    states: [普通, 火星文]\n",
    )
    .unwrap();
}

#[test]
fn switch_settings_do_not_take_over_a_foreign_librime_initializer() {
    let root = std::env::temp_dir().join(format!(
        "keytao-foreign-initializer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let app = root.join("用户");
    let foreign = root.join("foreign");
    let other = root.join("other");
    write_config(&app);
    write_config(&other);
    std::fs::create_dir_all(&foreign).unwrap();
    assert_eq!(list_schema_switches(&app).unwrap().len(), 1);
    assert_eq!(list_schema_switches(&app).unwrap().len(), 1);

    unsafe {
        let api = &*rime_get_api();
        let foreign_path = CString::new(foreign.to_str().unwrap()).unwrap();
        librime_sys::rime_struct!(traits: RimeTraits);
        traits.user_data_dir = foreign_path.as_ptr();
        traits.shared_data_dir = foreign_path.as_ptr();
        api.initialize.unwrap()(&mut traits);
        let foreign_bytes = CStr::from_ptr(api.get_user_data_dir.unwrap()())
            .to_bytes()
            .to_vec();

        let error = "librime is initialized for another user directory";
        assert_eq!(list_schema_switches(&app).unwrap_err(), error);
        assert_eq!(
            set_schema_switch(&app, "keytao_foreign_config", "mars", 1).unwrap_err(),
            error
        );
        assert!(!app.join(keytao_core::SCHEMA_SWITCH_REQUEST_FILE).exists());
        assert_eq!(list_schema_switches(&other).unwrap_err(), error);
        assert_eq!(release_idle_rime_config(&app).unwrap_err(), error);
        assert_eq!(
            CStr::from_ptr(api.get_user_data_dir.unwrap()()).to_bytes(),
            foreign_bytes
        );
        api.finalize.unwrap()();
    }
    std::fs::remove_dir_all(root).unwrap();
}
