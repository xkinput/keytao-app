//! Manual smoke test for the reload contract.
//!
//! Requires a user directory whose schemas are already deployed:
//!
//! ```text
//! reload_smoke <user-dir> <shared-dir> <input> <candidate>
//! ```
//!
//! It asserts that a session handed out before a reload keeps working after
//! librime was finalized and initialized again, that the mode the user was in
//! survives the rebuild, and that a commit is delivered exactly once.

use keytao_core::{ImeRuntime, ImeRuntimeSession, ImeState};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> String {
    "usage: reload_smoke <user-dir> <shared-dir> <input> <candidate>".into()
}

fn type_input(session: &ImeRuntimeSession, input: &str) -> Result<ImeState, String> {
    let mut state = ImeState::empty();
    for character in input.chars() {
        let result = session
            .process_key_result(character as u32, 0)
            .ok_or_else(|| format!("Rime rejected input character {character:?}"))?;
        state = result.state;
    }
    Ok(state)
}

fn require_candidate(state: &ImeState, expected: &str, stage: &str) -> Result<(), String> {
    if state
        .candidates
        .iter()
        .any(|candidate| candidate.text == expected)
    {
        return Ok(());
    }
    Err(format!(
        "{stage}: candidate {expected:?} not found in {:?}",
        state
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>()
    ))
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let shared_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let input = args.next().ok_or_else(usage)?;
    let expected_candidate = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let runtime = ImeRuntime::with_dirs(&user_dir, shared_dir.to_string_lossy().into_owned());
    runtime.init_without_deploy()?;
    let session = runtime.create_session()?;

    let state = type_input(&session, &input)?;
    require_candidate(&state, &expected_candidate, "before reload")?;
    session.reset().ok_or("reset failed before reload")?;

    // The mode the user is in must survive the engine rebuild.
    session
        .set_ascii_mode(true)
        .ok_or("set_ascii_mode failed")?;
    runtime.reload_without_deploy()?;
    if !session.is_ascii_mode() {
        return Err("ascii_mode was lost while the session was rebuilt".into());
    }
    session
        .set_ascii_mode(false)
        .ok_or("set_ascii_mode failed after reload")?;

    // The pre-reload session handle must keep working on the new engine.
    let state = type_input(&session, &input)?;
    require_candidate(&state, &expected_candidate, "after reload")?;

    // A commit is reported once, and a read-only query never swallows one.
    let committed = session
        .select_candidate_global(0)
        .ok_or("select_candidate_global failed")?
        .committed
        .ok_or("selecting a candidate produced no commit")?;
    let after = session.state();
    if after.committed.is_some() {
        return Err("state() reported an already delivered commit again".into());
    }
    if !after.preedit.is_empty() {
        return Err(format!(
            "composition survived the commit: {}",
            after.preedit
        ));
    }

    println!("reload smoke ok: input={input} committed={committed}");
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
