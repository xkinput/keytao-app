use keytao_core::{ImeRuntime, ImeState};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> String {
    "usage: scheme_smoke <user-dir> <shared-dir> <input> <schema-name> <candidate>".into()
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let shared_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let input = args.next().ok_or_else(usage)?;
    let expected_schema_name = args.next().ok_or_else(usage)?;
    let expected_candidate = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let runtime = ImeRuntime::with_dirs(user_dir, shared_dir.to_string_lossy().into_owned());
    runtime.init()?;
    let session = runtime.create_session()?;
    let mut state = session.state();
    for character in input.chars() {
        let result = session
            .process_key_result(character as u32, 0)
            .ok_or_else(|| format!("Rime rejected input character {character:?}"))?;
        state = result.state;
    }
    verify_state(&state, &expected_schema_name, &expected_candidate)?;
    println!(
        "schema={} input={} candidates={}",
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

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
