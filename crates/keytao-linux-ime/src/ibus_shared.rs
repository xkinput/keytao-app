//! What the two IBus frontends have in common.
//!
//! `gnome_ibus_engine` serves `org.freedesktop.IBus.Engine` objects inside
//! GNOME's ibus-daemon; `ibus_backend` serves `org.freedesktop.IBus.InputContext`
//! objects from a shim that stands in for the daemon in Chromium/CEF sessions.
//! The two interfaces differ, but the decisions behind a key event do not:
//! which keys a password field swallows, when a Shift release is only a mode
//! switch, when an empty composition hands the key back to the client, and when
//! `Return` belongs to librime.  Each frontend used to carry its own copy of
//! that state machine, so a fix landed in one of them and the same keystroke
//! behaved differently under GNOME than under the shim.
//!
//! The decision machine and the IBus value builders live here.  The frontends
//! only publish the outcome over their own interface.

use crate::engine::{ibus_content_type_policy, ImeSession};
use keytao_core::{key_policy, ImeState, InputContextPolicy, KeyProcessResult, RIME_RELEASE_MASK};
use keytao_theme::{
    CandidateOptionModel, CandidatePanelInput, CandidatePanelModel, ThemeCandidate, ThemeResolver,
    UiCapabilities,
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use zbus::zvariant;

// ── Key state machine ─────────────────────────────────────────────────────────

/// `XK_Shift_L` and `XK_Shift_R`, the keys librime's `ascii_composer` switches
/// the conversion mode on.
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;

pub fn is_shift_key(sym: u32) -> bool {
    matches!(sym, XK_SHIFT_L | XK_SHIFT_R)
}

/// What a key event is before the current composition is looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPhase {
    /// A password or PIN field: nothing reaches librime and nothing is drawn.
    Sensitive,
    /// A Shift release, which `ascii_composer` may turn into a mode switch.
    ShiftRelease,
    /// Any other release; librime is only interested in Shift releases.
    IgnoredRelease,
    /// A press, still to be routed against the composition librime holds.
    Press,
}

pub fn key_phase(keyval: u32, state: u32, policy: InputContextPolicy) -> KeyPhase {
    if !policy.composing {
        return KeyPhase::Sensitive;
    }
    if state & RIME_RELEASE_MASK == 0 {
        return KeyPhase::Press;
    }
    if is_shift_key(keyval) {
        KeyPhase::ShiftRelease
    } else {
        KeyPhase::IgnoredRelease
    }
}

/// Where a press goes once the composition librime holds is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressRoute {
    /// Nothing is being composed and the key belongs to the client.
    BypassEmptyComposition,
    /// `Return` over a live composition: only the schema knows whether it
    /// confirms the highlighted candidate or commits the raw code.
    Enter,
    /// Everything else, digits and select keys included, goes to librime.
    Rime,
}

pub fn press_route(keyval: u32, mods: u32, before: &ImeState) -> PressRoute {
    if key_policy::should_bypass_empty_composition(keyval, mods, before) {
        return PressRoute::BypassEmptyComposition;
    }
    if key_policy::is_enter_key(keyval)
        && key_policy::enter_action(before) == key_policy::EnterAction::ForwardToRime
    {
        return PressRoute::Enter;
    }
    PressRoute::Rime
}

/// What a frontend has to publish once the machine has run.
#[derive(Debug)]
pub enum KeyOutcome {
    /// Nothing to publish; the client keeps the key.
    Forward,
    /// Tear the preedit and candidate UI down, then the client keeps the key.
    ClearUi,
    /// Only the conversion mode moved.
    ModeChanged { ascii_mode: bool, accepted: bool },
    /// Publish the new IME state.
    Publish {
        state: Box<ImeState>,
        accepted: bool,
    },
}

impl KeyOutcome {
    /// What `ProcessKeyEvent` answers the client.
    pub fn accepted(&self) -> bool {
        match self {
            KeyOutcome::Forward | KeyOutcome::ClearUi => false,
            KeyOutcome::ModeChanged { accepted, .. } | KeyOutcome::Publish { accepted, .. } => {
                *accepted
            }
        }
    }

    /// A session without a usable engine must not swallow the key: the client
    /// keeps it rather than losing it to an input method that cannot compose.
    fn from_result(result: Option<KeyProcessResult>) -> Self {
        match result {
            Some(result) => KeyOutcome::Publish {
                state: Box::new(result.state),
                accepted: result.accepted,
            },
            None => KeyOutcome::Forward,
        }
    }
}

/// Run one IBus `ProcessKeyEvent` against `session`.
pub fn process_key_event(session: &ImeSession, keyval: u32, state: u32) -> KeyOutcome {
    match key_phase(keyval, state, session.input_policy()) {
        KeyPhase::Sensitive | KeyPhase::IgnoredRelease => KeyOutcome::Forward,
        KeyPhase::ShiftRelease => match session.process_key_result(keyval, RIME_RELEASE_MASK) {
            Some(result) => {
                tracing::debug!(
                    "IBus mode after Shift release: ascii_mode={}",
                    result.state.ascii_mode
                );
                KeyOutcome::ModeChanged {
                    ascii_mode: result.state.ascii_mode,
                    accepted: result.accepted,
                }
            }
            None => KeyOutcome::Forward,
        },
        KeyPhase::Press => {
            tracing::trace!("IBus ProcessKeyEvent keyval={keyval:#x} state={state:#x}");
            let before = session.state();
            match press_route(keyval, state, &before) {
                PressRoute::BypassEmptyComposition => KeyOutcome::ClearUi,
                PressRoute::Enter => KeyOutcome::from_result(session.process_enter()),
                PressRoute::Rime => {
                    KeyOutcome::from_result(session.process_key_result(keyval, state))
                }
            }
        }
    }
}

/// A gesture on the candidate list, which reaches the engine through the
/// desktop panel rather than through a key event.
///
/// These go through librime's own candidate and paging entry points, never
/// through a synthesised digit or `-`/`=`, so a schema without select keys or
/// paging bindings behaves the same as one that has them.  `None` means librime
/// had nothing to act on and the frontend leaves what is on screen alone.
pub fn select_candidate(session: &ImeSession, index: usize) -> Option<ImeState> {
    session.select_candidate_on_page(index)
}

pub fn change_page(session: &ImeSession, backward: bool) -> Option<ImeState> {
    session.change_page(backward)
}

/// A cursor-up/down button on the panel.  The keysym goes in unmodified, and a
/// key librime did not take leaves the composition as it is: the panel has no
/// client to hand the key back to.
pub fn navigation_key(session: &ImeSession, keyval: u32) -> Option<ImeState> {
    let result = session.process_key_result(keyval, 0)?;
    result.accepted.then_some(result.state)
}

// ── Content type ──────────────────────────────────────────────────────────────

/// The IBus `content-type` a client last set on its context.
///
/// It is what decides whether the context is a password field, and IBus also
/// exposes it as a readable property, so both frontends have to keep it and
/// both have to map it to a session policy the same way.
#[derive(Debug, Default)]
pub struct ContentTypeState {
    purpose: AtomicU32,
    hints: AtomicU32,
}

/// What a new content type asks the frontend to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentTypeChange {
    /// The policy to push into the session.
    pub policy: InputContextPolicy,
    /// The context just turned into a password or PIN field: whatever preedit
    /// and candidate list is still on screen has to go, because no further key
    /// will reach librime to update it.
    pub turned_sensitive: bool,
}

impl ContentTypeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pair the `ContentType` property reports.
    pub fn get(&self) -> (u32, u32) {
        (
            self.purpose.load(Ordering::Relaxed),
            self.hints.load(Ordering::Relaxed),
        )
    }

    /// Record a content type and say what it changes.  The previous policy is
    /// derived from the previous content type rather than read back from the
    /// session, because the content type is the only thing that sets it on
    /// these two frontends.
    pub fn set(&self, purpose: u32, hints: u32) -> ContentTypeChange {
        let (previous_purpose, previous_hints) = self.get();
        self.purpose.store(purpose, Ordering::Relaxed);
        self.hints.store(hints, Ordering::Relaxed);
        let previous = ibus_content_type_policy(previous_purpose, previous_hints);
        let policy = ibus_content_type_policy(purpose, hints);
        ContentTypeChange {
            policy,
            turned_sensitive: previous.composing && !policy.composing,
        }
    }
}

// ── Conversion-mode indicator ─────────────────────────────────────────────────

/// The conversion mode a frontend last announced to the desktop.
///
/// The GNOME engine publishes it as an IBus property and the shim as a Kimpanel
/// signal, but both decide when to publish in the same way, and an indicator
/// that drifts out of step with librime is quiet enough to stay unnoticed.
#[derive(Debug)]
pub struct ModeIndicator {
    ascii_mode: AtomicBool,
}

/// A conversion mode that has to be announced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeChange {
    pub ascii_mode: bool,
    /// Also draw the transient on-screen hint.  A candidate list already shows
    /// which mode is on, so the hint is suppressed while one is up rather than
    /// drawn on top of it.
    pub hint: bool,
}

impl Default for ModeIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl ModeIndicator {
    /// librime starts a session in conversion mode.
    pub fn new() -> Self {
        Self {
            ascii_mode: AtomicBool::new(false),
        }
    }

    /// Adopt the mode librime reports without asking for an announcement; the
    /// caller is about to publish it over its own interface anyway.
    pub fn adopt(&self, ascii_mode: bool) {
        self.ascii_mode.store(ascii_mode, Ordering::Relaxed);
    }

    /// `None` when the desktop already shows `ascii_mode`.
    pub fn update(&self, ascii_mode: bool, hint: bool) -> Option<ModeChange> {
        if self.ascii_mode.swap(ascii_mode, Ordering::Relaxed) == ascii_mode {
            return None;
        }
        Some(ModeChange { ascii_mode, hint })
    }
}

// ── IBus value builders ───────────────────────────────────────────────────────

pub const IBUS_ORIENTATION_SYSTEM: i32 = 2;

/// Build an IBusText structure: `("IBusText", a{sv}, s, v)` where the trailing
/// variant is an empty `("IBusAttrList", a{sv}, av)`.
///
/// The structure is returned unwrapped so that a caller passing it as a `v`
/// signal argument gets `v(sa{sv}sv)`; `av` elements need the extra wrapper
/// [`ibus_text_as_variant`] adds.
pub fn ibus_text_variant(text: &str) -> zvariant::Value<'static> {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict1 = Dict::new(sig_s.clone(), sig_v.clone());
    let empty_array = Array::new(sig_v.clone());
    let attr_list = StructureBuilder::new()
        .add_field("IBusAttrList".to_owned())
        .append_field(Value::Dict(empty_dict1))
        .append_field(Value::Array(empty_array))
        .build();
    let attr_list_variant = Value::Value(Box::new(Value::Structure(attr_list)));

    let empty_dict2 = Dict::new(sig_s, sig_v);
    let ibus_text = StructureBuilder::new()
        .add_field("IBusText".to_owned())
        .append_field(Value::Dict(empty_dict2))
        .add_field(text.to_owned())
        .append_field(attr_list_variant)
        .build();
    Value::Structure(ibus_text)
}

/// Wrap an IBusText inside a variant, the shape `av` array elements need.
pub fn ibus_text_as_variant(text: &str) -> zvariant::Value<'static> {
    zvariant::Value::Value(Box::new(ibus_text_variant(text)))
}

pub fn ibus_text_value(text: &str) -> zvariant::OwnedValue {
    zvariant::OwnedValue::try_from(ibus_text_variant(text)).expect("ibus_text_value")
}

pub fn candidate_display_text(candidate: &CandidateOptionModel) -> String {
    match candidate.comment.as_deref() {
        Some(comment) => format!("{} {}", candidate.text, comment),
        None => candidate.text.clone(),
    }
}

pub fn state_to_panel_model(
    state: &ImeState,
    theme_resolver: &ThemeResolver,
) -> CandidatePanelModel {
    let theme = theme_resolver.current();
    theme.candidate_panel_model(
        CandidatePanelInput {
            preedit: state.preedit.clone(),
            candidates: state
                .candidates
                .iter()
                .map(|candidate| ThemeCandidate {
                    text: candidate.text.clone(),
                    comment: candidate.comment.clone(),
                })
                .collect(),
            highlighted_candidate_index: state.highlighted_candidate_index,
            page: state.page,
            is_last_page: state.is_last_page,
            select_keys: state.select_keys.clone(),
        },
        &UiCapabilities::system_lookup_table(),
    )
}

/// Build an IBusLookupTable: `("IBusLookupTable", a{sv}, u, u, b, b, i, av, av)`.
pub fn ibus_lookup_table_value(model: &CandidatePanelModel) -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());

    let mut candidates = Array::new(sig_v.clone());
    for candidate in &model.candidates {
        candidates
            .append(ibus_text_as_variant(&candidate_display_text(candidate)))
            .expect("append IBus lookup candidate");
    }

    let mut labels = Array::new(sig_v);
    for candidate in &model.candidates {
        labels
            .append(ibus_text_as_variant(&candidate.label))
            .expect("append IBus lookup label");
    }

    let page_size = model.candidates.len().clamp(1, 16) as u32;
    let cursor_pos = model
        .candidates
        .iter()
        .position(|candidate| candidate.selected)
        .unwrap_or(0) as u32;

    let table = StructureBuilder::new()
        .add_field("IBusLookupTable".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field(page_size)
        .add_field(cursor_pos)
        .add_field(true) // cursor_visible
        .add_field(false) // round
        .add_field(IBUS_ORIENTATION_SYSTEM)
        .append_field(Value::Array(candidates))
        .append_field(Value::Array(labels))
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(table)).expect("ibus_lookup_table_value")
}

#[cfg(test)]
mod tests {
    use super::{
        ibus_lookup_table_value, ibus_text_variant, is_shift_key, key_phase, press_route,
        state_to_panel_model, ContentTypeState, KeyPhase, ModeChange, ModeIndicator, PressRoute,
    };
    use crate::engine::{IBUS_INPUT_HINT_PRIVATE, IBUS_INPUT_PURPOSE_PASSWORD};
    use keytao_core::{key_policy, Candidate, ImeState, InputContextPolicy, RIME_RELEASE_MASK};
    use keytao_theme::ThemeResolver;

    fn composing_state() -> ImeState {
        let mut state = ImeState::empty();
        state.preedit = "ni".to_owned();
        state.candidates = vec![Candidate {
            text: "你".to_owned(),
            comment: None,
        }];
        state
    }

    #[test]
    fn shift_keysyms_are_the_ones_ascii_composer_switches_on() {
        assert!(is_shift_key(0xffe1));
        assert!(is_shift_key(0xffe2));
        // Control and Alt are not mode switches.
        assert!(!is_shift_key(0xffe3));
        assert!(!is_shift_key(0xffe9));
    }

    #[test]
    fn a_sensitive_context_never_reaches_librime() {
        let sensitive = InputContextPolicy::sensitive();
        // Presses and releases alike, whatever the key.
        assert_eq!(key_phase(0x61, 0, sensitive), KeyPhase::Sensitive);
        assert_eq!(
            key_phase(0xffe1, RIME_RELEASE_MASK, sensitive),
            KeyPhase::Sensitive
        );
    }

    #[test]
    fn only_shift_releases_survive_the_release_filter() {
        let policy = InputContextPolicy::default();
        assert_eq!(
            key_phase(0xffe1, RIME_RELEASE_MASK, policy),
            KeyPhase::ShiftRelease
        );
        assert_eq!(
            key_phase(0x61, RIME_RELEASE_MASK, policy),
            KeyPhase::IgnoredRelease
        );
        assert_eq!(key_phase(0x61, 0, policy), KeyPhase::Press);
        assert_eq!(key_phase(0xffe1, 0, policy), KeyPhase::Press);
    }

    #[test]
    fn an_empty_composition_hands_editing_keys_back_to_the_client() {
        let empty = ImeState::empty();
        assert_eq!(
            press_route(key_policy::XK_BACK_SPACE, 0x10, &empty),
            PressRoute::BypassEmptyComposition
        );
        assert_eq!(
            press_route(key_policy::XK_RETURN, 0, &empty),
            PressRoute::BypassEmptyComposition
        );
        // A letter still starts a composition.
        assert_eq!(press_route(0x61, 0, &empty), PressRoute::Rime);
    }

    #[test]
    fn enter_over_a_composition_belongs_to_librime() {
        let state = composing_state();
        assert_eq!(
            press_route(key_policy::XK_RETURN, 0, &state),
            PressRoute::Enter
        );
        assert_eq!(
            press_route(key_policy::XK_KP_ENTER, 0, &state),
            PressRoute::Enter
        );
    }

    #[test]
    fn digits_are_not_intercepted_as_select_keys() {
        // Only the schema knows whether a digit picks a candidate or spells a
        // code, so the frontend must hand it to librime either way.
        let state = composing_state();
        assert_eq!(press_route(0x31, 0, &state), PressRoute::Rime);
        assert_eq!(
            press_route(key_policy::XK_SPACE, 0, &state),
            PressRoute::Rime
        );
    }

    #[test]
    fn only_the_step_into_a_password_field_tears_the_ui_down() {
        let content_type = ContentTypeState::new();
        assert_eq!(content_type.get(), (0, 0));

        let change = content_type.set(IBUS_INPUT_PURPOSE_PASSWORD, 0);
        assert!(!change.policy.composing);
        assert!(change.turned_sensitive, "the preedit on screen has to go");
        assert_eq!(content_type.get(), (IBUS_INPUT_PURPOSE_PASSWORD, 0));

        // Clients re-set the same content type on every focus change; only the
        // first one has anything on screen to clear.
        let again = content_type.set(IBUS_INPUT_PURPOSE_PASSWORD, IBUS_INPUT_HINT_PRIVATE);
        assert!(!again.policy.composing);
        assert!(!again.turned_sensitive);

        let back = content_type.set(0, 0);
        assert!(back.policy.composing);
        assert!(!back.turned_sensitive);
    }

    #[test]
    fn the_mode_indicator_only_speaks_when_the_mode_moves() {
        let indicator = ModeIndicator::new();
        // A session starts in conversion mode, which is what both frontends
        // register at startup.
        assert_eq!(indicator.update(false, true), None);
        assert_eq!(
            indicator.update(true, true),
            Some(ModeChange {
                ascii_mode: true,
                hint: true
            })
        );
        assert_eq!(indicator.update(true, true), None);
        // A candidate list already shows the mode, so the hint is suppressed
        // while one is up — but the property still has to be published.
        assert_eq!(
            indicator.update(false, false),
            Some(ModeChange {
                ascii_mode: false,
                hint: false
            })
        );

        // Adopting what librime reports silences the next announcement, which
        // is how focus-in avoids re-publishing a mode that did not move.
        indicator.adopt(true);
        assert_eq!(indicator.update(true, true), None);
    }

    #[test]
    fn the_lookup_table_keeps_the_ibus_signature() {
        // ibus-daemon and the Kimpanel applet drop the whole table when a field
        // type drifts.
        let model = state_to_panel_model(&composing_state(), &ThemeResolver::new(None, None));
        assert!(!model.candidates.is_empty());
        let table = ibus_lookup_table_value(&model);
        assert_eq!(
            zbus::zvariant::Value::try_from(&table)
                .expect("lookup table value")
                .value_signature()
                .as_str(),
            "(sa{sv}uubbiavav)"
        );
        assert_eq!(
            ibus_text_variant("你").value_signature().as_str(),
            "(sa{sv}sv)"
        );
    }
}
