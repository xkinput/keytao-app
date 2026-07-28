//! A deterministic librime deployment for the candidate, paging and reload
//! smoke tests.
//!
//! Everything the fallback paths rely on is deliberately absent, so a run can
//! only succeed through librime's own entry points:
//!
//! * no `menu/select_keys`, and digits belong to the speller's alphabet, so a
//!   synthesized select key edits the code instead of picking a candidate;
//! * a page holds more candidates than `DEFAULT_SELECT_KEYS` has characters,
//!   so the last candidate of a page has no select key at all;
//! * no `key_binder/bindings`, so the `-`/`=` paging fallback does nothing but
//!   feed the speller;
//! * the dictionary is sorted `original` and every word shares one code, so
//!   candidate order, page count and page contents are fixed.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Must start with `keytao` for `schema_install_state` to treat it as ours.
pub const SCHEMA_ID: &str = "keytao_smoke";

/// The code every fixture word is filed under.
pub const CODE: &str = "aa";

/// Candidates per page; longer than `keytao_core::DEFAULT_SELECT_KEYS`.
pub const PAGE_SIZE: usize = 12;

/// One code, several pages, distinct single-character words.
pub const WORDS: [&str; 20] = [
    "甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸", "子", "丑", "寅", "卯", "辰", "巳",
    "午", "未", "申", "酉",
];

/// Write the fixture into `dir`, which doubles as user and shared data dir.
pub fn write(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("default.yaml"), default_yaml())?;
    std::fs::write(dir.join("default.custom.yaml"), default_custom_yaml())?;
    std::fs::write(dir.join(format!("{SCHEMA_ID}.schema.yaml")), schema_yaml())?;
    std::fs::write(dir.join(format!("{SCHEMA_ID}.dict.yaml")), dict_yaml())?;
    Ok(())
}

/// A fresh directory for one fixture deployment. librime keeps compiled tables
/// per directory, so every run gets its own.
pub fn scratch_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("keytao-{name}-{}-{unique}", std::process::id()))
}

fn default_yaml() -> String {
    format!(
        "config_version: \"1\"\n\
         schema_list:\n  \
         - schema: {SCHEMA_ID}\n"
    )
}

fn default_custom_yaml() -> String {
    format!(
        "patch:\n  \
         schema_list:\n    \
         - schema: {SCHEMA_ID}\n"
    )
}

fn schema_yaml() -> String {
    format!(
        "schema:\n  \
           schema_id: {SCHEMA_ID}\n  \
           name: KeyTao Smoke\n  \
           version: \"1\"\n\
         switches:\n  \
         - name: ascii_mode\n    \
           reset: 0\n\
         engine:\n  \
           processors:\n    \
           - ascii_composer\n    \
           - key_binder\n    \
           - speller\n    \
           - selector\n    \
           - navigator\n    \
           - express_editor\n  \
           segmentors:\n    \
           - ascii_segmentor\n    \
           - abc_segmentor\n    \
           - fallback_segmentor\n  \
           translators:\n    \
           - table_translator\n\
         ascii_composer:\n  \
           good_old_caps_lock: false\n  \
           switch_key:\n    \
             Caps_Lock: noop\n    \
             Shift_L: noop\n    \
             Shift_R: noop\n    \
             Control_L: noop\n    \
             Control_R: noop\n\
         speller:\n  \
           alphabet: 'abcdefghijklmnopqrstuvwxyz0123456789-='\n  \
           initials: 'abcdefghijklmnopqrstuvwxyz'\n\
         translator:\n  \
           dictionary: {SCHEMA_ID}\n  \
           enable_completion: false\n  \
           enable_encoder: false\n  \
           enable_sentence: false\n  \
           enable_user_dict: false\n\
         menu:\n  \
           page_size: {PAGE_SIZE}\n"
    )
}

fn dict_yaml() -> String {
    let mut content = format!(
        "---\n\
         name: {SCHEMA_ID}\n\
         version: \"1\"\n\
         sort: original\n\
         use_preset_vocabulary: false\n\
         ...\n"
    );
    for word in WORDS {
        content.push_str(&format!("{word}\t{CODE}\n"));
    }
    content
}
