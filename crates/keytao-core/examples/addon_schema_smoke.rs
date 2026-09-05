//! Functional smoke test for a deployed KeyTao scheme plus Easy English.
//!
//! Run against a scratch user directory whose `default.custom.yaml` initially
//! contains only `keytao`, but whose Easy English source files are present. The
//! example deploys KeyTao, adds `easy_en` to the list, measures that incremental
//! deploy, verifies a `hel* ` candidate, then switches back to `keytao` and
//! verifies a Chinese candidate.

use keytao_core::{ImeRuntime, ImeState};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

fn usage() -> String {
    "usage: addon_schema_smoke <scratch-user-dir> <shared-dir>".into()
}

fn type_text(
    session: &keytao_core::ImeRuntimeSession,
    text: &str,
) -> Result<ImeState, String> {
    let mut state = session.state();
    for character in text.chars() {
        state = session
            .process_key_result(character as u32, 0)
            .ok_or_else(|| format!("Rime session rejected {character:?}"))?
            .state;
    }
    Ok(state)
}

fn candidate_texts(state: &ImeState) -> Vec<String> {
    state
        .candidates
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect()
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let shared_dir = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let runtime = ImeRuntime::with_dirs(&user_dir, shared_dir);
    let base_deploy_started = Instant::now();
    runtime.init()?;
    let base_deploy_elapsed = base_deploy_started.elapsed();

    let default_custom = user_dir.join("default.custom.yaml");
    fs::write(
        &default_custom,
        "patch:\n  schema_list:\n    - schema: keytao\n    - schema: easy_en\n",
    )
    .map_err(|error| format!("write {}: {error}", default_custom.display()))?;
    let addon_deploy_started = Instant::now();
    runtime.reload()?;
    let addon_deploy_elapsed = addon_deploy_started.elapsed();

    let session = runtime.create_session()?;
    let schemas = session
        .list_schemas()
        .ok_or("Rime session did not return schemas")?;
    let schema_ids: Vec<&str> = schemas.iter().map(|schema| schema.id.as_str()).collect();
    if !schema_ids.contains(&"keytao") || !schema_ids.contains(&"easy_en") {
        return Err(format!("deployed schema list mismatch: {schema_ids:?}"));
    }

    let selected_english = session.select_schema("easy_en")?;
    if selected_english.schema_name != "Easy English" {
        return Err(format!(
            "easy_en schema name mismatch: {:?}",
            selected_english.schema_name
        ));
    }
    let english = type_text(&session, "hel")?;
    let english_candidates = candidate_texts(&english);
    if !english_candidates
        .iter()
        .any(|candidate| candidate.starts_with("hel") && candidate.ends_with(' '))
    {
        return Err(format!(
            "easy_en did not return a hel* candidate with trailing space: {english_candidates:?}"
        ));
    }

    session.clear_composition();
    let selected_chinese = session.select_schema("keytao")?;
    if selected_chinese.schema_name != "键道6" {
        return Err(format!(
            "keytao schema name mismatch: {:?}",
            selected_chinese.schema_name
        ));
    }
    let chinese = type_text(&session, "n")?;
    let chinese_candidates = candidate_texts(&chinese);
    if !chinese_candidates.iter().any(|candidate| candidate == "你") {
        return Err(format!(
            "keytao did not return expected Chinese candidate: {chinese_candidates:?}"
        ));
    }

    println!(
        "base_keytao_deploy_elapsed_ms={}",
        base_deploy_elapsed.as_millis()
    );
    println!(
        "easy_en_incremental_deploy_elapsed_ms={}",
        addon_deploy_elapsed.as_millis()
    );
    println!("deployed_schema_ids={schema_ids:?}");
    println!(
        "easy_en schema={:?} input=hel candidates={english_candidates:?}",
        english.schema_name
    );
    println!(
        "keytao schema={:?} input=n candidates={chinese_candidates:?}",
        chinese.schema_name
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
