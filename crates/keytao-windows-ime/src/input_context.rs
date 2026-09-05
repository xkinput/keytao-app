//! Detects input contexts a keyboard TIP must keep its hands off.
//!
//! TSF expresses this through compartments: `GUID_COMPARTMENT_KEYBOARD_DISABLED`
//! is set by hosts such as Chromium for password fields, `GUID_COMPARTMENT_EMPTYCONTEXT`
//! marks a context without a text store, and `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE`
//! carries the system-wide on/off state (Ctrl+Space, the input indicator).
//! Password, PIN and private fields that leave the compartments alone are still
//! recognised through their input scope.
//!
//! While any of these hold, keys are passed through unaltered and the session
//! runs with `InputContextPolicy::sensitive()` so nothing reaches the user
//! dictionary.
//!
//! Probe outcomes distinguish a declared restriction, a clear answer, an
//! invalid/unknown answer, and a refused password edit session. TSF reports "no restriction" and "cannot answer
//! right now" through the same shapes — a refused synchronous read session, a
//! failed `GetSelection`, a property in an unexpected form — and reading any of
//! them as "no restriction" would hand a password field straight to Rime. A
//! Most failed probes therefore count as restricted and are retried from the
//! key path. A refused password-probe edit session is distinct: the caller can
//! retain a known answer for the same context, or allow one key-path retry when
//! there is no prior answer.

use std::{cell::Cell, rc::Rc};

use windows::{
    core::{Interface, GUID, VARIANT},
    Win32::{
        System::Com::CoTaskMemFree,
        UI::TextServices::{
            ITfCompartmentMgr, ITfContext, ITfDocumentMgr, ITfInputScope, ITfReadOnlyProperty,
            ITfThreadMgr, InputScope, GUID_COMPARTMENT_EMPTYCONTEXT,
            GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
            GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, GUID_PROP_INPUTSCOPE, IS_ALPHANUMERIC_PIN,
            IS_ALPHANUMERIC_PIN_SET, IS_NUMERIC_PASSWORD, IS_NUMERIC_PIN, IS_PASSWORD, IS_PRIVATE,
            TF_CONVERSIONMODE_NATIVE, TF_DEFAULT_SELECTION, TF_SELECTION,
        },
    },
};

use crate::{
    edit_session::{take_selection_range, with_read_session},
    state::append_diagnostic,
};

/// The compartments that live on the *context* rather than on the thread
/// manager, and that a host flips to take the keyboard away mid-session.
///
/// They have to be watched with a sink advised on the focused context: a host
/// that disables an already-composing context reports it nowhere else, and the
/// key path alone would leave the existing preedit and candidate window on
/// screen.
pub(crate) const CONTEXT_SENSITIVITY_COMPARTMENTS: [GUID; 2] = [
    GUID_COMPARTMENT_KEYBOARD_DISABLED,
    GUID_COMPARTMENT_EMPTYCONTEXT,
];

/// Input scopes that mean "compose nothing here, remember nothing".
///
/// `IS_PASSWORD` on its own leaves holes: lock screens and payment dialogs
/// declare a PIN scope, and hosts that only want the input method to forget the
/// field declare `IS_PRIVATE`. The shared contract puts all three in the same
/// bucket — the IBus backend already treats `purpose PASSWORD/PIN` and
/// `hint PRIVATE` alike — so TSF has to as well.
const SENSITIVE_INPUT_SCOPES: [InputScope; 6] = [
    IS_PASSWORD,
    IS_NUMERIC_PASSWORD,
    IS_NUMERIC_PIN,
    IS_ALPHANUMERIC_PIN,
    IS_ALPHANUMERIC_PIN_SET,
    IS_PRIVATE,
];

fn scopes_declare_sensitive(scopes: &[InputScope]) -> bool {
    scopes
        .iter()
        .any(|scope| SENSITIVE_INPUT_SCOPES.contains(scope))
}

/// Outcome of a single TSF probe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ContextProbe {
    /// The context declared the restriction.
    Restricted,
    /// The context answered, and declared nothing.
    Clear,
    /// The read failed. Counts as restricted until a later probe answers.
    #[default]
    Unknown,
    /// TSF refused the synchronous password-probe edit session. Unlike an
    /// invalid property answer, this does not prove the field is sensitive.
    ProbeFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputBlockReason {
    KeyboardClosed,
    Sensitive,
    ContextDisabled,
    ProbeFailed,
}

impl InputBlockReason {
    pub(crate) fn diagnostic_name(self) -> &'static str {
        match self {
            Self::KeyboardClosed => "keyboard_closed",
            Self::Sensitive => "sensitive",
            Self::ContextDisabled => "context_disabled",
            Self::ProbeFailed => "probe_failed",
        }
    }
}

impl ContextProbe {
    fn declared(set: bool) -> Self {
        if set {
            Self::Restricted
        } else {
            Self::Clear
        }
    }

    fn blocks_input(self) -> bool {
        matches!(self, Self::Restricted | Self::Unknown)
    }

    pub(crate) fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Restricted => "restricted",
            Self::Clear => "clear",
            Self::Unknown => "unknown",
            Self::ProbeFailed => "probe_failed",
        }
    }

    fn is_known(self) -> bool {
        matches!(self, Self::Restricted | Self::Clear)
    }
}

/// What the current input context allows, as declared by TSF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContextInputState {
    /// `GUID_COMPARTMENT_KEYBOARD_DISABLED` is set on the context.
    pub(crate) keyboard_disabled: ContextProbe,
    /// `GUID_COMPARTMENT_EMPTYCONTEXT` is set on the context.
    pub(crate) empty_context: ContextProbe,
    /// The context declares an `IS_PASSWORD` input scope.
    pub(crate) password: ContextProbe,
    /// The most recent password-scope edit session was refused. The effective
    /// `password` value may still be the last known answer for this context.
    pub(crate) password_probe_failed: bool,
}

impl ContextInputState {
    /// Every probe answered, none of them declared a restriction.
    #[cfg(test)]
    pub(crate) fn unrestricted() -> Self {
        Self {
            keyboard_disabled: ContextProbe::Clear,
            empty_context: ContextProbe::Clear,
            password: ContextProbe::Clear,
            password_probe_failed: false,
        }
    }

    /// No composition, no candidates, no user-dictionary learning.
    pub(crate) fn is_sensitive(&self) -> bool {
        self.keyboard_disabled.blocks_input()
            || self.empty_context.blocks_input()
            || self.password.blocks_input()
    }

    pub(crate) fn block_reason(&self) -> Option<InputBlockReason> {
        if matches!(
            (self.keyboard_disabled, self.empty_context),
            (ContextProbe::Restricted, _) | (_, ContextProbe::Restricted)
        ) {
            return Some(InputBlockReason::ContextDisabled);
        }
        if self.password == ContextProbe::Restricted {
            return Some(InputBlockReason::Sensitive);
        }
        if [self.keyboard_disabled, self.empty_context, self.password]
            .into_iter()
            .any(|probe| probe == ContextProbe::Unknown)
        {
            return Some(InputBlockReason::ProbeFailed);
        }
        None
    }

    /// Take fresh compartment answers, keep the input-scope one.
    ///
    /// The input scope is the expensive probe and the compartments cannot
    /// change it; dropping it here would let a password field go back to
    /// composing the moment its host touched an unrelated compartment.
    fn with_compartments(
        self,
        keyboard_disabled: ContextProbe,
        empty_context: ContextProbe,
    ) -> Self {
        Self {
            keyboard_disabled,
            empty_context,
            password: self.password,
            password_probe_failed: self.password_probe_failed,
        }
    }

    /// A probe could not answer, so the state has to be inspected again before
    /// the context can go back to composing.
    pub(crate) fn needs_retry(&self) -> bool {
        self.password_probe_failed
            || [self.keyboard_disabled, self.empty_context, self.password]
                .into_iter()
                .any(|probe| matches!(probe, ContextProbe::Unknown | ContextProbe::ProbeFailed))
    }
}

pub(crate) fn merge_probe_failed_state(
    previous_same_context: Option<ContextInputState>,
    mut inspected: ContextInputState,
) -> ContextInputState {
    if inspected.password == ContextProbe::ProbeFailed {
        if let Some(previous) = previous_same_context.filter(|state| state.password.is_known()) {
            inspected.password = previous.password;
        }
        inspected.password_probe_failed = true;
    }
    inspected
}

fn compartment_probe(manager: &ITfCompartmentMgr, guid: &GUID) -> ContextProbe {
    unsafe {
        let Ok(compartment) = manager.GetCompartment(guid) else {
            return ContextProbe::Unknown;
        };
        let Ok(value) = compartment.GetValue() else {
            return ContextProbe::Unknown;
        };
        // An undeclared compartment reads back empty; that is a real "not set".
        if value.is_empty() {
            return ContextProbe::Clear;
        }
        match i32::try_from(&value) {
            Ok(flag) => ContextProbe::declared(flag != 0),
            Err(_) => ContextProbe::Unknown,
        }
    }
}

fn focus_context(thread_mgr: Option<&ITfThreadMgr>) -> Option<ITfContext> {
    let thread_mgr = thread_mgr?;
    let document_mgr: ITfDocumentMgr = unsafe { thread_mgr.GetFocus() }.ok()?;
    unsafe { document_mgr.GetTop() }.ok()
}

/// The context a caller means: the one it was handed, or the focused one.
pub(crate) fn resolve_context(
    thread_mgr: Option<&ITfThreadMgr>,
    context: Option<&ITfContext>,
) -> Option<ITfContext> {
    match context {
        Some(context) => Some(context.clone()),
        None => focus_context(thread_mgr),
    }
}

/// True when TSF told us to stop processing keys for this context.
///
/// Cheap enough for the key callbacks: two compartment reads, and only when the
/// caller could not supply a context does it ask the thread manager for the
/// focus document.
pub(crate) fn context_block_reason(
    thread_mgr: Option<&ITfThreadMgr>,
    context: Option<&ITfContext>,
) -> Option<InputBlockReason> {
    let owned;
    let context = match context {
        Some(context) => context,
        None => match focus_context(thread_mgr) {
            Some(context) => {
                owned = context;
                &owned
            }
            // No focus document at all: nothing may be composed into it.
            None => return Some(InputBlockReason::ProbeFailed),
        },
    };
    let Ok(manager) = context.cast::<ITfCompartmentMgr>() else {
        return Some(InputBlockReason::ProbeFailed);
    };
    let keyboard_disabled = compartment_probe(&manager, &GUID_COMPARTMENT_KEYBOARD_DISABLED);
    let empty_context = compartment_probe(&manager, &GUID_COMPARTMENT_EMPTYCONTEXT);
    if matches!(
        (keyboard_disabled, empty_context),
        (ContextProbe::Restricted, _) | (_, ContextProbe::Restricted)
    ) {
        Some(InputBlockReason::ContextDisabled)
    } else if matches!(
        (keyboard_disabled, empty_context),
        (ContextProbe::Unknown, _) | (_, ContextProbe::Unknown)
    ) {
        Some(InputBlockReason::ProbeFailed)
    } else {
        None
    }
}

/// The system on/off state. A closed keyboard passes every key through.
pub(crate) fn keyboard_is_open(thread_mgr: Option<&ITfThreadMgr>) -> bool {
    let Some(thread_mgr) = thread_mgr else {
        return true;
    };
    let Ok(manager) = thread_mgr.cast::<ITfCompartmentMgr>() else {
        return true;
    };
    unsafe {
        let Ok(compartment) = manager.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) else {
            return true;
        };
        let Ok(value) = compartment.GetValue() else {
            return true;
        };
        // An unset compartment means "not declared yet", not "closed".
        if value.is_empty() {
            return true;
        }
        i32::try_from(&value).map(|flag| flag != 0).unwrap_or(true)
    }
}

pub(crate) fn set_keyboard_open(thread_mgr: Option<&ITfThreadMgr>, client_id: u32, open: bool) {
    set_thread_compartment(
        thread_mgr,
        client_id,
        &GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
        i32::from(open),
    );
}

fn set_thread_compartment(
    thread_mgr: Option<&ITfThreadMgr>,
    client_id: u32,
    guid: &GUID,
    value: i32,
) {
    let Some(thread_mgr) = thread_mgr else {
        return;
    };
    let Ok(manager) = thread_mgr.cast::<ITfCompartmentMgr>() else {
        return;
    };
    unsafe {
        let Ok(compartment) = manager.GetCompartment(guid) else {
            return;
        };
        let value = VARIANT::from(value);
        let _ = compartment.SetValue(client_id, &value);
    }
}

/// The system conversion mode, translated to KeyTao's `ascii_mode`.
///
/// `None` while the compartment is undeclared — then the language bar value is
/// authoritative and nothing needs to be pushed into Rime.
pub(crate) fn conversion_mode_is_ascii(thread_mgr: Option<&ITfThreadMgr>) -> Option<bool> {
    let mode =
        thread_compartment_value(thread_mgr, &GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)?;
    Some(mode & TF_CONVERSIONMODE_NATIVE as i32 == 0)
}

fn thread_compartment_value(thread_mgr: Option<&ITfThreadMgr>, guid: &GUID) -> Option<i32> {
    let thread_mgr = thread_mgr?;
    let manager = thread_mgr.cast::<ITfCompartmentMgr>().ok()?;
    unsafe {
        let compartment = manager.GetCompartment(guid).ok()?;
        let value = compartment.GetValue().ok()?;
        if value.is_empty() {
            return None;
        }
        i32::try_from(&value).ok()
    }
}

/// Ask the context for its input scopes. Requires an edit cookie, so it runs in
/// a synchronous read session; hosts refuse one while the document is locked and
/// that answer is `ProbeFailed`, distinct from an invalid property answer.
fn context_password_probe(context: &ITfContext, client_id: u32) -> ContextProbe {
    if client_id == 0 {
        return ContextProbe::Unknown;
    }
    let probe = Rc::new(Cell::new(ContextProbe::Unknown));
    let probe_for_session = Rc::clone(&probe);
    let result = with_read_session(context, client_id, move |ec, ctx| {
        probe_for_session.set(read_password_scope(ec, ctx));
        Ok(())
    });
    if let Err(error) = result {
        append_diagnostic(format!(
            "password probe session hr=0x{:08X}",
            error.code().0 as u32
        ));
        return ContextProbe::ProbeFailed;
    }
    probe.get()
}

fn read_password_scope(ec: u32, context: &ITfContext) -> ContextProbe {
    unsafe {
        let Ok(property) = context.GetAppProperty(&GUID_PROP_INPUTSCOPE) else {
            return ContextProbe::Unknown;
        };
        let mut selections = [TF_SELECTION::default()];
        let mut count = 0u32;
        let fetched = context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selections, &mut count);
        // The range is a `ManuallyDrop`, so it has to be taken even when the call
        // failed after filling the entry.
        let range = take_selection_range(&mut selections[0]);
        if fetched.is_err() || count == 0 {
            return ContextProbe::Unknown;
        }
        let Some(range) = range else {
            return ContextProbe::Unknown;
        };
        input_scopes_probe(&property, ec, &range)
    }
}

unsafe fn input_scopes_probe(
    property: &ITfReadOnlyProperty,
    ec: u32,
    range: &windows::Win32::UI::TextServices::ITfRange,
) -> ContextProbe {
    let Ok(value) = property.GetValue(ec, range) else {
        return ContextProbe::Unknown;
    };
    // No input scope attached to this range: an ordinary, unrestricted field.
    if value.is_empty() {
        return ContextProbe::Clear;
    }
    let Ok(unknown) = windows::core::IUnknown::try_from(&value) else {
        return ContextProbe::Unknown;
    };
    let Ok(scopes) = unknown.cast::<ITfInputScope>() else {
        return ContextProbe::Unknown;
    };
    let mut buffer: *mut InputScope = std::ptr::null_mut();
    let mut count = 0u32;
    if scopes.GetInputScopes(&mut buffer, &mut count).is_err() {
        return ContextProbe::Unknown;
    }
    if buffer.is_null() {
        // A successful call with no buffer only makes sense for an empty list.
        return if count == 0 {
            ContextProbe::Clear
        } else {
            ContextProbe::Unknown
        };
    }
    let found = scopes_declare_sensitive(std::slice::from_raw_parts(buffer, count as usize));
    CoTaskMemFree(Some(buffer.cast()));
    ContextProbe::declared(found)
}

/// The two context compartments. Plain reads: no edit cookie, no session, so
/// they are cheap enough for a notification callback.
fn context_compartment_probes(context: &ITfContext) -> (ContextProbe, ContextProbe) {
    match context.cast::<ITfCompartmentMgr>() {
        Ok(manager) => (
            compartment_probe(&manager, &GUID_COMPARTMENT_KEYBOARD_DISABLED),
            compartment_probe(&manager, &GUID_COMPARTMENT_EMPTYCONTEXT),
        ),
        // A context that will not answer is not one we may compose into.
        Err(_) => (ContextProbe::Unknown, ContextProbe::Unknown),
    }
}

/// Full inspection for a focus change: compartments plus input scope.
pub(crate) fn inspect_context(
    thread_mgr: Option<&ITfThreadMgr>,
    context: Option<&ITfContext>,
    client_id: u32,
) -> ContextInputState {
    let Some(context) = resolve_context(thread_mgr, context) else {
        // Nothing to inspect: no focus document, or the host refused to hand one
        // out. Both leave every probe `Unknown`, which keeps the session in
        // pass-through and marks the state for a retry on the next key.
        return ContextInputState::default();
    };
    let (keyboard_disabled, empty_context) = context_compartment_probes(&context);
    ContextInputState {
        keyboard_disabled,
        empty_context,
        password: context_password_probe(&context, client_id),
        password_probe_failed: false,
    }
}

/// Re-read the two compartments, keeping the input scope from the last full
/// inspection.
///
/// For the compartment sink. Neither compartment can change the input scope, so
/// re-running that probe would only spend a synchronous read session inside a
/// notification callback — where the document is most likely still locked, and
/// a refusal would turn a known-ordinary field into a spurious `Unknown`.
pub(crate) fn refresh_context_compartments(
    thread_mgr: Option<&ITfThreadMgr>,
    context: Option<&ITfContext>,
    previous: ContextInputState,
) -> ContextInputState {
    let Some(context) = resolve_context(thread_mgr, context) else {
        return ContextInputState::default();
    };
    let (keyboard_disabled, empty_context) = context_compartment_probes(&context);
    previous.with_compartments(keyboard_disabled, empty_context)
}

#[cfg(test)]
mod tests {
    use super::{
        merge_probe_failed_state, scopes_declare_sensitive, ContextInputState, ContextProbe,
    };
    use windows::Win32::UI::TextServices::{
        IS_ALPHANUMERIC_PIN, IS_ALPHANUMERIC_PIN_SET, IS_DEFAULT, IS_EMAIL_USERNAME,
        IS_NUMERIC_PASSWORD, IS_NUMERIC_PIN, IS_PASSWORD, IS_PRIVATE, IS_URL,
    };

    #[test]
    fn password_pin_and_private_scopes_all_block_composing() {
        // A lock screen or payment dialog declares a PIN scope, not
        // `IS_PASSWORD`; a host that only wants the field forgotten declares
        // `IS_PRIVATE`. The IBus backend maps purpose PASSWORD/PIN and hint
        // PRIVATE to the same sensitive policy, so TSF must not be narrower.
        for scope in [
            IS_PASSWORD,
            IS_NUMERIC_PASSWORD,
            IS_NUMERIC_PIN,
            IS_ALPHANUMERIC_PIN,
            IS_ALPHANUMERIC_PIN_SET,
            IS_PRIVATE,
        ] {
            assert!(scopes_declare_sensitive(&[scope]));
        }
        // Hosts attach several scopes to one range; one sensitive entry decides.
        assert!(scopes_declare_sensitive(&[IS_DEFAULT, IS_NUMERIC_PIN]));
    }

    #[test]
    fn ordinary_scopes_leave_composing_alone() {
        assert!(!scopes_declare_sensitive(&[]));
        assert!(!scopes_declare_sensitive(&[IS_DEFAULT]));
        assert!(!scopes_declare_sensitive(&[IS_EMAIL_USERNAME, IS_URL]));
    }

    #[test]
    fn any_declared_restriction_makes_the_context_sensitive() {
        assert!(!ContextInputState::unrestricted().is_sensitive());
        for state in [
            ContextInputState {
                keyboard_disabled: ContextProbe::Restricted,
                ..ContextInputState::unrestricted()
            },
            ContextInputState {
                empty_context: ContextProbe::Restricted,
                ..ContextInputState::unrestricted()
            },
            ContextInputState {
                password: ContextProbe::Restricted,
                ..ContextInputState::unrestricted()
            },
        ] {
            assert!(state.is_sensitive());
        }
    }

    #[test]
    fn a_failed_probe_fails_closed_and_asks_for_a_retry() {
        for state in [
            ContextInputState {
                keyboard_disabled: ContextProbe::Unknown,
                ..ContextInputState::unrestricted()
            },
            ContextInputState {
                empty_context: ContextProbe::Unknown,
                ..ContextInputState::unrestricted()
            },
            ContextInputState {
                password: ContextProbe::Unknown,
                ..ContextInputState::unrestricted()
            },
        ] {
            assert!(state.is_sensitive());
            assert!(state.needs_retry());
        }
        assert!(!ContextInputState::unrestricted().needs_retry());
    }

    #[test]
    fn an_uninspected_context_starts_out_sensitive() {
        // `TsfState::new` and the "no focus document" branch both land here.
        let state = ContextInputState::default();
        assert!(state.is_sensitive());
        assert!(state.needs_retry());
    }

    #[test]
    fn a_compartment_refresh_keeps_the_input_scope_answer() {
        // The compartment sink only re-reads the two compartments. A password
        // field whose host toggles an unrelated compartment must not lose the
        // `IS_PASSWORD` answer and start composing again.
        let password_field = ContextInputState {
            password: ContextProbe::Restricted,
            ..ContextInputState::unrestricted()
        };
        let refreshed = password_field.with_compartments(ContextProbe::Clear, ContextProbe::Clear);
        assert_eq!(refreshed.password, ContextProbe::Restricted);
        assert!(refreshed.is_sensitive());

        // The compartments themselves are taken from the fresh read, so a
        // context the host just disabled becomes sensitive right away.
        let ordinary = ContextInputState::unrestricted();
        assert!(ordinary
            .with_compartments(ContextProbe::Restricted, ContextProbe::Clear)
            .is_sensitive());
        // And one the host re-enabled goes back to composing.
        let disabled = ContextInputState {
            keyboard_disabled: ContextProbe::Restricted,
            ..ContextInputState::unrestricted()
        };
        assert!(!disabled
            .with_compartments(ContextProbe::Clear, ContextProbe::Clear)
            .is_sensitive());
    }

    #[test]
    fn a_declared_restriction_needs_no_retry() {
        let state = ContextInputState {
            password: ContextProbe::Restricted,
            ..ContextInputState::unrestricted()
        };
        assert!(!state.needs_retry());
    }

    #[test]
    fn password_probe_failure_keeps_a_known_clear_answer_or_retries_open() {
        let failed = ContextInputState {
            password: ContextProbe::ProbeFailed,
            ..ContextInputState::unrestricted()
        };

        let kept = merge_probe_failed_state(Some(ContextInputState::unrestricted()), failed);
        assert_eq!(kept.password, ContextProbe::Clear);
        assert!(!kept.is_sensitive());
        assert!(kept.needs_retry());
        assert!(kept.password_probe_failed);

        let first_failure = merge_probe_failed_state(None, failed);
        assert_eq!(first_failure.password, ContextProbe::ProbeFailed);
        assert!(!first_failure.is_sensitive());
        assert!(first_failure.needs_retry());
        assert!(first_failure.password_probe_failed);
    }
}
