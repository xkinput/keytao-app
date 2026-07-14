use keytao_core::{patch_windows_lua_compatibility, ImeRuntime, ImeState};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() -> String {
    concat!(
        "usage: scheme_switch_smoke <user-dir> <shared-dir> <replacement-dir> ",
        "<input> <schema-name> <candidate>"
    )
    .into()
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    let entries =
        fs::read_dir(source).map_err(|error| format!("read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else {
            let content = fs::read(&source_path)
                .map_err(|error| format!("read {}: {error}", source_path.display()))?;
            fs::write(&destination_path, content)
                .map_err(|error| format!("write {}: {error}", destination_path.display()))?;
        }
    }
    Ok(())
}

fn verify_state(
    state: &ImeState,
    expected_schema_name: &str,
    expected_candidate: &str,
) -> Result<(), String> {
    if state.schema_name != expected_schema_name {
        return Err(format!(
            "schema mismatch: expected {expected_schema_name:?}, got {:?}",
            state.schema_name
        ));
    }
    if !state
        .candidates
        .iter()
        .any(|candidate| candidate.text == expected_candidate)
    {
        return Err(format!(
            "candidate {expected_candidate:?} not found in {:?}",
            state
                .candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>()
        ));
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let shared_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let replacement_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let input = args.next().ok_or_else(usage)?;
    let expected_schema_name = args.next().ok_or_else(usage)?;
    let expected_candidate = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let runtime = ImeRuntime::with_dirs(&user_dir, shared_dir.to_string_lossy().into_owned());
    runtime.init()?;
    let session = runtime.create_session()?;

    copy_tree(&replacement_dir, &user_dir)?;
    let patched = patch_windows_lua_compatibility(&user_dir)?;
    if patched.is_empty() {
        return Err("replacement scheme did not require a Windows Lua compatibility patch".into());
    }
    runtime.reload()?;

    let mut state = session.state();
    for character in input.chars() {
        let result = session
            .process_key_result(character as u32, 0)
            .ok_or_else(|| format!("Rime rejected input character {character:?}"))?;
        state = result.state;
    }
    verify_state(&state, &expected_schema_name, &expected_candidate)?;
    println!(
        "patched={} schema={} input={} candidates={}",
        patched.join(","),
        state.schema_name,
        input,
        state
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>()
            .join(",")
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
