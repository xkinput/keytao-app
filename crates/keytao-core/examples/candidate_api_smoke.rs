//! Manual smoke test for the candidate, paging and commit APIs against a real
//! deployment.
//!
//! Requires a user directory whose schemas are already deployed:
//!
//! ```text
//! candidate_api_smoke <user-dir> <shared-dir> <input>
//! ```
//!
//! It walks selection, highlighting, paging, commit, discard and the sensitive
//! input policy on the schema the user actually runs, and checks that the
//! cursor offsets are Unicode scalars. What it cannot decide is whether those
//! calls went through librime's own entry points or through the fallback key
//! strokes: on a schema that publishes select keys and imports the `-`/`=`
//! paging bindings both produce the same result, which is why this run prints
//! the capability report and says so. The deterministic proof lives in
//! `tests/candidate_api.rs`, whose fixture has no select keys, no paging
//! bindings and more candidates per page than there are fallback keys.

use keytao_core::{
    EngineCapabilities, ImeRuntime, ImeRuntimeSession, ImeState, InputContextPolicy,
    DEFAULT_SELECT_KEYS,
};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> String {
    "usage: candidate_api_smoke <user-dir> <shared-dir> <input>".into()
}

fn type_input(session: &ImeRuntimeSession, input: &str) -> Result<ImeState, String> {
    session.reset().ok_or("reset failed")?;
    let mut state = ImeState::empty();
    for character in input.chars() {
        let result = session
            .process_key_result(character as u32, 0)
            .ok_or_else(|| format!("Rime rejected input character {character:?}"))?;
        state = result.state;
    }
    if state.candidates.is_empty() {
        return Err(format!("input {input:?} produced no candidates"));
    }
    Ok(state)
}

fn check_offsets(state: &ImeState, stage: &str) -> Result<(), String> {
    let chars = state.preedit.chars().count();
    if state.cursor > chars || state.sel_start > chars || state.sel_end > chars {
        return Err(format!(
            "{stage}: offsets are not scalar offsets into {:?}: cursor={} sel=({}, {})",
            state.preedit, state.cursor, state.sel_start, state.sel_end
        ));
    }
    if state.sel_start > state.sel_end {
        return Err(format!(
            "{stage}: inverted selection ({}, {})",
            state.sel_start, state.sel_end
        ));
    }
    Ok(())
}

/// Why a passing run on this schema does not rule out the fallback paths.
fn inconclusive_notes(state: &ImeState, capabilities: EngineCapabilities) -> Vec<String> {
    let mut notes = Vec::new();
    let select_keys = state.select_keys.as_deref().unwrap_or_default();
    if !select_keys.is_empty() && state.candidates.len() <= select_keys.chars().count() {
        notes.push(format!(
            "every candidate has a select key ({select_keys:?}), so a synthesized select key \
             would have picked the same one"
        ));
    } else if select_keys.is_empty()
        && state.candidates.len() <= DEFAULT_SELECT_KEYS.chars().count()
    {
        notes.push(format!(
            "the page fits in the {DEFAULT_SELECT_KEYS:?} fallback, so a synthesized digit could \
             have produced this selection"
        ));
    }
    if state.is_last_page {
        notes.push("a single page never exercises paging".into());
    }
    if !capabilities.supports_native_paging() {
        notes.push("this librime pages through the `-`/`=` bindings".into());
    }
    if !capabilities.supports_candidate_selection() {
        notes.push("this librime selects through select keys".into());
    }
    notes
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let shared_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let input = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let runtime = ImeRuntime::with_dirs(&user_dir, shared_dir.to_string_lossy().into_owned());
    runtime.init_without_deploy()?;
    let capabilities = runtime.capabilities();
    println!("librime capabilities: {capabilities:?}");
    let session = runtime.create_session()?;

    let state = type_input(&session, &input)?;
    check_offsets(&state, "after typing")?;
    println!(
        "typed {input:?}: preedit={:?} candidates={} select_keys={:?} last_page={}",
        state.preedit,
        state.candidates.len(),
        state.select_keys,
        state.is_last_page
    );
    for note in inconclusive_notes(&state, capabilities) {
        println!("note: {note}");
    }

    // Highlighting must move the menu without producing a commit.
    let last = state.candidates.len() - 1;
    let highlighted = session
        .highlight_candidate_on_page(last)
        .ok_or("highlight_candidate_on_page failed")?;
    if highlighted.highlighted_candidate_index != last {
        return Err(format!(
            "highlight landed on {} instead of {last}",
            highlighted.highlighted_candidate_index
        ));
    }
    if highlighted.committed.is_some() {
        return Err("highlighting committed text".into());
    }

    // Paging must work without the `-`/`=` key_binder preset.
    if !state.is_last_page {
        let next = session.change_page(false).ok_or("change_page failed")?;
        if next.page != state.page + 1 {
            return Err(format!(
                "change_page(forward) went from page {} to {}",
                state.page, next.page
            ));
        }
        let back = session.change_page(true).ok_or("change_page failed")?;
        if back.page != state.page {
            return Err(format!(
                "change_page(backward) went from page {} to {}",
                next.page, back.page
            ));
        }
    }

    // The last candidate of the page must be selectable even when the schema
    // publishes no select keys and the page is longer than "1234567890".
    let expected = state.candidates[last].text.clone();
    let selected = session
        .select_candidate_on_page(last)
        .ok_or("select_candidate_on_page failed")?;
    let produced = selected.committed.clone().unwrap_or_default();
    if !produced.contains(&expected) && !selected.preedit.contains(&expected) {
        return Err(format!(
            "selecting candidate {last} ({expected:?}) produced committed={produced:?} preedit={:?}",
            selected.preedit
        ));
    }
    check_offsets(&selected, "after selecting")?;
    session.reset().ok_or("reset failed")?;

    // Discarding leaves nothing behind.
    type_input(&session, &input)?;
    let cleared = session.clear_composition().ok_or("clear failed")?;
    if !cleared.preedit.is_empty() || !cleared.candidates.is_empty() {
        return Err(format!(
            "clear_composition left preedit={:?} candidates={}",
            cleared.preedit,
            cleared.candidates.len()
        ));
    }

    // Committing the composition is the focus-loss path and must produce text.
    type_input(&session, &input)?;
    let committed = session
        .commit_composition()
        .ok_or("commit_composition failed")?;
    if committed.committed.is_none() {
        return Err("commit_composition produced no text".into());
    }
    if !committed.preedit.is_empty() {
        return Err(format!(
            "commit_composition left preedit {:?}",
            committed.preedit
        ));
    }

    // The raw input fallback commits the code the user typed.
    type_input(&session, &input)?;
    let raw = session.raw_input().unwrap_or_default();
    let raw_commit = session
        .commit_raw_input()
        .ok_or("commit_raw_input failed")?;
    let raw_committed = raw_commit.committed.clone().unwrap_or_default();
    if !raw_committed.ends_with(&raw) || raw.is_empty() {
        return Err(format!(
            "commit_raw_input committed {raw_committed:?} for raw input {raw:?}"
        ));
    }
    if !raw_commit.preedit.is_empty() {
        return Err(format!(
            "commit_raw_input left preedit {:?}",
            raw_commit.preedit
        ));
    }

    // Return goes through Rime and never drops the composition.
    type_input(&session, &input)?;
    let entered = session.process_enter().ok_or("process_enter failed")?;
    if !entered.accepted {
        return Err("process_enter neither let Rime handle Return nor committed".into());
    }
    if entered.state.committed.is_none() {
        return Err("process_enter produced no commit while composing".into());
    }
    let idle = session.state();
    if idle.committed.is_some() {
        return Err("state() reported an already delivered commit again".into());
    }

    // Return with nothing to compose belongs to the application.
    let idle_enter = session.process_enter().ok_or("process_enter failed")?;
    if idle_enter.accepted {
        return Err("process_enter swallowed Return without a composition".into());
    }

    // A sensitive input context discards the composition and stops composing.
    type_input(&session, &input)?;
    let sensitive = session
        .set_input_policy(InputContextPolicy::sensitive())
        .ok_or("set_input_policy failed")?;
    if !sensitive.preedit.is_empty() || !sensitive.candidates.is_empty() {
        return Err(format!(
            "sensitive context kept preedit={:?} candidates={}",
            sensitive.preedit,
            sensitive.candidates.len()
        ));
    }
    for character in input.chars() {
        let result = session
            .process_key_result(character as u32, 0)
            .ok_or("process_key_result failed in a sensitive context")?;
        if result.accepted {
            return Err(format!(
                "sensitive context accepted {character:?} instead of passing it through"
            ));
        }
        if !result.state.preedit.is_empty() || !result.state.candidates.is_empty() {
            return Err("sensitive context produced a composition".into());
        }
    }
    session
        .set_input_policy(InputContextPolicy::default())
        .ok_or("set_input_policy failed")?;
    let resumed = type_input(&session, &input)?;
    if resumed.candidates.is_empty() {
        return Err("composing did not resume after the sensitive context".into());
    }
    session.reset().ok_or("reset failed")?;

    println!("candidate api smoke ok: raw={raw:?}");
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
