//! Exercise real edit-session callbacks against a deterministic in-memory host.

use super::{apply_ime_state, flush_pending_commit_retry, queue_lost_commit_once};
use crate::state::{
    flush_pending_ime_ui, new_shared_state, reset_input_for_focus_change, terminate_input_now,
    SharedState, WeakState,
};
use keytao_core::ImeState;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    core::{implement, IUnknown, Result, GUID, HRESULT, PCWSTR, PWSTR},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_NOTIMPL, S_OK},
        System::Com::IDataObject,
        UI::TextServices::*,
    },
};

macro_rules! unsupported {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> { Err(E_NOTIMPL.into()) })*
    };
}

#[derive(Default)]
struct Host {
    queued: RefCell<Vec<ITfEditSession>>,
    session_flags: RefCell<Vec<TF_CONTEXT_EDIT_CONTEXT_FLAGS>>,
    text: RefCell<String>,
    insert_queries: Cell<usize>,
    starts: Cell<usize>,
    fail_start: Cell<Option<usize>>,
    active_compositions: Rc<Cell<usize>>,
    reject_overlapping_compositions: Cell<bool>,
    selections: Cell<usize>,
    selection_anchors: RefCell<Vec<TfActiveSelEnd>>,
    collapses: RefCell<Vec<TfAnchor>>,
    fail_selection: Cell<Option<usize>>,
    selection_hook: RefCell<Option<Box<dyn FnOnce()>>>,
    range_hook: Rc<RefCell<Option<Box<dyn FnOnce()>>>>,
    writes: RefCell<Vec<String>>,
    fail_text: Cell<bool>,
    inline_async: Cell<bool>,
    request_depth: Cell<usize>,
    reentered_request: Cell<bool>,
    refuse_sync_once: Cell<bool>,
    fail_async_request: Cell<bool>,
    end_count: Rc<Cell<usize>>,
    fail_end: Rc<Cell<bool>>,
    end_hook: Rc<RefCell<Option<Box<dyn FnOnce()>>>>,
    fail_terminate: Cell<bool>,
    terminate_count: Cell<usize>,
    last_sink: RefCell<Option<ITfCompositionSink>>,
    last_composition: RefCell<Option<ITfComposition>>,
    state: RefCell<Option<WeakState>>,
}

impl Host {
    fn context(self: &Rc<Self>) -> ITfContext {
        Context { host: self.clone() }.into()
    }
    fn range(self: &Rc<Self>) -> ITfRange {
        Range { host: self.clone() }.into()
    }
    fn run_queued(&self) {
        loop {
            let queued = self.queued.take();
            if queued.is_empty() {
                break;
            }
            for session in queued {
                unsafe { session.DoEditSession(1).unwrap() };
            }
        }
    }
}

#[implement(
    ITfContext,
    ITfInsertAtSelection,
    ITfContextComposition,
    ITfContextOwnerCompositionServices
)]
struct Context {
    host: Rc<Host>,
}

impl ITfContext_Impl for Context_Impl {
    fn RequestEditSession(
        &self,
        _id: u32,
        session: Option<&ITfEditSession>,
        flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> Result<HRESULT> {
        let session = session.unwrap();
        self.host.session_flags.borrow_mut().push(flags);
        let depth = self.host.request_depth.get();
        if depth != 0 {
            self.host.reentered_request.set(true);
        }
        if flags.0 & TF_ES_SYNC.0 != 0
            || (self.host.inline_async.get() && flags.0 & TF_ES_ASYNC.0 == 0)
        {
            if self.host.refuse_sync_once.replace(false) {
                return Err(E_FAIL.into());
            }
            self.host.request_depth.set(depth + 1);
            let result = unsafe { session.DoEditSession(1) }.map(|()| S_OK);
            self.host.request_depth.set(depth);
            result
        } else {
            if self.host.fail_async_request.get() {
                return Err(E_FAIL.into());
            }
            self.host.queued.borrow_mut().push(session.clone());
            Ok(TS_S_ASYNC)
        }
    }
    fn SetSelection(&self, _ec: u32, count: u32, selection: *const TF_SELECTION) -> Result<()> {
        assert_eq!(count, 1);
        self.host
            .selection_anchors
            .borrow_mut()
            .push(unsafe { (*selection).style.ase });
        let count = self.host.selections.get() + 1;
        self.host.selections.set(count);
        if let Some(hook) = self.host.selection_hook.take() {
            hook();
        }
        if self.host.fail_selection.get() == Some(count) {
            return Err(E_FAIL.into());
        }
        Ok(())
    }
    unsupported! {
        fn InWriteSession(&self, id: u32) -> BOOL;
        fn GetSelection(&self, ec: u32, index: u32, count: u32, selection: *mut TF_SELECTION, fetched: *mut u32) -> ();
        fn GetStart(&self, ec: u32) -> ITfRange;
        fn GetEnd(&self, ec: u32) -> ITfRange;
        fn GetActiveView(&self) -> ITfContextView;
        fn EnumViews(&self) -> IEnumTfContextViews;
        fn GetStatus(&self) -> TS_STATUS;
        fn GetProperty(&self, guid: *const GUID) -> ITfProperty;
        fn GetAppProperty(&self, guid: *const GUID) -> ITfReadOnlyProperty;
        fn TrackProperties(&self, props: *const *const GUID, count: u32, app_props: *const *const GUID, app_count: u32) -> ITfReadOnlyProperty;
        fn EnumProperties(&self) -> IEnumTfProperties;
        fn GetDocumentMgr(&self) -> ITfDocumentMgr;
        fn CreateRangeBackup(&self, ec: u32, range: Option<&ITfRange>) -> ITfRangeBackup;
    }
}

impl ITfInsertAtSelection_Impl for Context_Impl {
    fn InsertTextAtSelection(
        &self,
        _ec: u32,
        flags: INSERT_TEXT_AT_SELECTION_FLAGS,
        _text: &PCWSTR,
        _count: i32,
    ) -> Result<ITfRange> {
        assert_eq!(flags, TF_IAS_QUERYONLY);
        self.host
            .insert_queries
            .set(self.host.insert_queries.get() + 1);
        Ok(self.host.range())
    }
    unsupported! {
        fn InsertEmbeddedAtSelection(&self, ec: u32, flags: u32, object: Option<&IDataObject>) -> ITfRange;
    }
}

impl ITfContextComposition_Impl for Context_Impl {
    fn StartComposition(
        &self,
        _ec: u32,
        range: Option<&ITfRange>,
        sink: Option<&ITfCompositionSink>,
    ) -> Result<ITfComposition> {
        let count = self.host.starts.get() + 1;
        self.host.starts.set(count);
        if self.host.fail_start.get() == Some(count) {
            return Err(E_FAIL.into());
        }
        if self.host.reject_overlapping_compositions.get() {
            assert_eq!(self.host.active_compositions.get(), 0);
        }
        self.host
            .active_compositions
            .set(self.host.active_compositions.get() + 1);
        let composition: ITfComposition = Composition {
            range: range.unwrap().clone(),
            range_hook: self.host.range_hook.clone(),
            end_count: self.host.end_count.clone(),
            fail_end: self.host.fail_end.clone(),
            end_hook: self.host.end_hook.clone(),
            active_compositions: self.host.active_compositions.clone(),
        }
        .into();
        *self.host.last_sink.borrow_mut() = sink.cloned();
        *self.host.last_composition.borrow_mut() = Some(composition.clone());
        Ok(composition)
    }
    unsupported! {
        fn EnumCompositions(&self) -> IEnumITfCompositionView;
        fn FindComposition(&self, ec: u32, range: Option<&ITfRange>) -> IEnumITfCompositionView;
        fn TakeOwnership(&self, ec: u32, view: Option<&ITfCompositionView>, sink: Option<&ITfCompositionSink>) -> ITfComposition;
    }
}

impl ITfContextOwnerCompositionServices_Impl for Context_Impl {
    fn TerminateComposition(&self, _view: Option<&ITfCompositionView>) -> Result<()> {
        if let Some(state) = self
            .host
            .state
            .borrow()
            .as_ref()
            .and_then(|state| state.upgrade())
        {
            assert!(
                !state.borrow().ime_write_session_active,
                "TerminateComposition requires the document lock to be released"
            );
        }
        self.host
            .terminate_count
            .set(self.host.terminate_count.get() + 1);
        if self.host.fail_terminate.get() {
            return Err(E_FAIL.into());
        }
        let sink = self.host.last_sink.borrow().clone();
        let composition = self.host.last_composition.borrow().clone();
        if let (Some(sink), Some(composition)) = (sink, composition) {
            unsafe {
                sink.OnCompositionTerminated(1, &composition)?;
            }
        }
        Ok(())
    }
}

#[implement(ITfRange)]
struct Range {
    host: Rc<Host>,
}
impl ITfRange_Impl for Range_Impl {
    fn Clone(&self) -> Result<ITfRange> {
        Ok(self.host.range())
    }
    fn Collapse(&self, _ec: u32, anchor: TfAnchor) -> Result<()> {
        self.host.collapses.borrow_mut().push(anchor);
        Ok(())
    }
    fn ShiftEnd(
        &self,
        _ec: u32,
        requested: i32,
        shifted: *mut i32,
        _halt: *const TF_HALTCOND,
    ) -> Result<()> {
        unsafe { *shifted = requested };
        Ok(())
    }
    fn SetText(&self, _ec: u32, _flags: u32, text: &PCWSTR, count: i32) -> Result<()> {
        if self.host.fail_text.replace(false) {
            return Err(E_FAIL.into());
        }
        let text = if count == 0 {
            String::new()
        } else {
            unsafe {
                String::from_utf16(std::slice::from_raw_parts(text.0, count as usize)).unwrap()
            }
        };
        self.host.writes.borrow_mut().push(text.clone());
        *self.host.text.borrow_mut() = text;
        Ok(())
    }
    fn GetText(&self, _ec: u32, _flags: u32, text: PWSTR, max: u32, count: *mut u32) -> Result<()> {
        let wide: Vec<u16> = self.host.text.borrow().encode_utf16().collect();
        let copied = wide.len().min(max as usize);
        unsafe {
            if copied > 0 {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), text.0, copied);
            }
            *count = copied as u32;
        }
        Ok(())
    }
    unsupported! {
        fn GetFormattedText(&self, ec: u32) -> IDataObject;
        fn GetEmbedded(&self, ec: u32, service: *const GUID, iid: *const GUID) -> IUnknown;
        fn InsertEmbedded(&self, ec: u32, flags: u32, object: Option<&IDataObject>) -> ();
        fn ShiftStart(&self, ec: u32, requested: i32, shifted: *mut i32, halt: *const TF_HALTCOND) -> ();
        fn ShiftStartToRange(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> ();
        fn ShiftEndToRange(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> ();
        fn ShiftStartRegion(&self, ec: u32, direction: TfShiftDir) -> BOOL;
        fn ShiftEndRegion(&self, ec: u32, direction: TfShiftDir) -> BOOL;
        fn IsEmpty(&self, ec: u32) -> BOOL;
        fn IsEqualStart(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> BOOL;
        fn IsEqualEnd(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> BOOL;
        fn CompareStart(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> i32;
        fn CompareEnd(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> i32;
        fn AdjustForInsert(&self, ec: u32, count: u32) -> BOOL;
        fn GetGravity(&self, start: *mut TfGravity, end: *mut TfGravity) -> ();
        fn SetGravity(&self, ec: u32, start: TfGravity, end: TfGravity) -> ();
        fn GetContext(&self) -> ITfContext;
    }
}

#[implement(ITfComposition, ITfCompositionView)]
struct Composition {
    range: ITfRange,
    range_hook: Rc<RefCell<Option<Box<dyn FnOnce()>>>>,
    end_count: Rc<Cell<usize>>,
    fail_end: Rc<Cell<bool>>,
    end_hook: Rc<RefCell<Option<Box<dyn FnOnce()>>>>,
    active_compositions: Rc<Cell<usize>>,
}
impl ITfComposition_Impl for Composition_Impl {
    fn GetRange(&self) -> Result<ITfRange> {
        if let Some(hook) = self.range_hook.take() {
            hook();
        }
        Ok(self.range.clone())
    }
    fn EndComposition(&self, _ec: u32) -> Result<()> {
        if let Some(hook) = self.end_hook.take() {
            hook();
        }
        self.end_count.set(self.end_count.get() + 1);
        if self.fail_end.get() {
            return Err(E_FAIL.into());
        }
        self.active_compositions
            .set(self.active_compositions.get().saturating_sub(1));
        Ok(())
    }
    unsupported! {
        fn ShiftStart(&self, ec: u32, range: Option<&ITfRange>) -> ();
        fn ShiftEnd(&self, ec: u32, range: Option<&ITfRange>) -> ();
    }
}

impl ITfCompositionView_Impl for Composition_Impl {
    fn GetOwnerClsid(&self) -> Result<GUID> {
        Ok(GUID::zeroed())
    }
    fn GetRange(&self) -> Result<ITfRange> {
        Ok(self.range.clone())
    }
}

fn fixture() -> (Rc<Host>, ITfContext, SharedState) {
    let host = Rc::new(Host::default());
    let context = host.context();
    let state = new_shared_state();
    state.borrow_mut().client_id = 1;
    state.borrow_mut().key_context = Some(context.clone());
    *host.state.borrow_mut() = Some(Rc::downgrade(&state));
    (host, context, state)
}

fn queue(context: &ITfContext, state: &SharedState) {
    let generation = state.borrow().input_generation;
    queue_lost_commit_once(
        context,
        1,
        state,
        &ImeState {
            committed: Some("retry".into()),
            ..ImeState::empty()
        },
        &Cell::new(false),
        generation,
        &Cell::new(false),
    );
}

#[test]
fn a_commit_retry_queued_before_focus_reset_never_requests_a_write() {
    let (host, context, state) = fixture();
    queue(&context, &state);
    reset_input_for_focus_change(&state);
    flush_pending_commit_retry(&state);
    assert!(host.queued.borrow().is_empty());
    assert!(host.writes.borrow().is_empty());
}

#[test]
fn a_requested_retry_expires_before_its_async_callback_on_focus_or_deactivate() {
    for client_id in [None, Some(0), Some(2)] {
        let (host, context, state) = fixture();
        queue(&context, &state);
        flush_pending_commit_retry(&state);
        assert_eq!(host.queued.borrow().len(), 1);
        if let Some(client_id) = client_id {
            state.borrow_mut().client_id = client_id;
        } else {
            reset_input_for_focus_change(&state);
        }
        host.run_queued();
        assert_eq!(host.insert_queries.get(), 0);
        assert!(host.writes.borrow().is_empty());
    }
}

#[test]
fn a_current_retry_still_commits_once() {
    let (host, context, state) = fixture();
    queue(&context, &state);
    flush_pending_commit_retry(&state);
    host.run_queued();
    flush_pending_commit_retry(&state);
    assert_eq!(&*host.text.borrow(), "retry");
    assert_eq!(
        host.writes
            .borrow()
            .iter()
            .filter(|text| text.as_str() == "retry")
            .count(),
        1
    );
}

#[test]
fn multiline_commits_keep_the_full_utf16_text_and_end_in_the_same_session() {
    for (text, expected) in [
        ("甲😀尾\n乙末\n丙终", "甲😀尾\r\n乙末\r\n丙终"),
        ("甲😀尾\r\n乙末\r\n丙终", "甲😀尾\r\n乙末\r\n丙终"),
        ("甲😀尾\r乙末\r丙终", "甲😀尾\r乙末\r丙终"),
    ] {
        let (host, context, state) = fixture();
        let frame = ImeState {
            committed: Some(text.into()),
            ..ImeState::empty()
        };
        apply_ime_state(&context, 1, &state, frame, false, false, None).unwrap();
        assert_eq!(&*host.writes.borrow(), &[expected]);
        assert_eq!(&*host.collapses.borrow(), &[TF_ANCHOR_END]);
        assert_eq!(&*host.selection_anchors.borrow(), &[TF_AE_NONE]);
        assert_eq!(host.end_count.get(), 1);
        assert!(host.queued.borrow().is_empty());
        assert_eq!(
            &*host.session_flags.borrow(),
            &[TF_CONTEXT_EDIT_CONTEXT_FLAGS(
                TF_ES_SYNC.0 | TF_ES_READWRITE.0
            )]
        );
        assert_eq!(host.terminate_count.get(), 0);
        assert!(state.borrow().pending_composition_ends.is_empty());
        assert_eq!(&*host.text.borrow(), expected);
    }
}

#[test]
fn a_committed_composition_ends_before_new_input_is_cached() {
    let (host, context, state) = fixture();
    let frame = ImeState {
        committed: Some("甲\n乙😀".into()),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, frame, false, false, None).unwrap();
    assert_eq!(host.end_count.get(), 1);
    assert!(host.queued.borrow().is_empty());
    state.borrow_mut().ime_state = Some(ImeState {
        preedit: "new input".into(),
        ..ImeState::empty()
    });
    host.run_queued();
    assert_eq!(
        state.borrow().ime_state.as_ref().unwrap().preedit,
        "new input"
    );
    assert_eq!(&*host.text.borrow(), "甲\r\n乙😀");
    assert_eq!(host.writes.borrow().len(), 1);
}

#[test]
fn losing_thread_focus_preserves_the_synchronously_ended_commit() {
    let (host, context, state) = fixture();
    let frame = ImeState {
        committed: Some("甲\n乙😀".into()),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, frame, false, false, None).unwrap();
    terminate_input_now(&state);
    assert_eq!(host.terminate_count.get(), 0);
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert_eq!(&*host.text.borrow(), "甲\r\n乙😀");
    host.run_queued();
    assert_eq!(host.end_count.get(), 1);
    assert_eq!(host.writes.borrow().len(), 1);
}

#[test]
fn a_written_commit_survives_a_later_selection_failure_without_retrying() {
    let (host, context, state) = fixture();
    host.fail_selection.set(Some(1)); // Commit SetText succeeds; positioning its caret fails.
    let frame = ImeState {
        committed: Some("甲".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(&*host.text.borrow(), "甲");
    assert!(host.queued.borrow().is_empty());
    assert!(state.borrow().pending_commit_retry.is_none());
}

#[test]
fn a_committed_frame_is_not_replayed_when_its_following_preedit_cannot_start() {
    let (host, context, state) = fixture();
    state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
    host.fail_start.set(Some(2));
    let frame = ImeState {
        committed: Some("甲".into()),
        preedit: "a".into(),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(&*host.text.borrow(), "甲");
    assert_eq!(host.writes.borrow().len(), 1);
    assert!(host.queued.borrow().is_empty());
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert!(state.borrow().pending_commit_retry.is_none());
}

#[test]
fn a_following_preedit_starts_after_the_old_composition_ends() {
    let (host, context, state) = fixture();
    state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
    host.reject_overlapping_compositions.set(true);
    let ending_state = state.clone();
    *host.end_hook.borrow_mut() = Some(Box::new(move || {
        assert!(ending_state.borrow().ime_write_session_active);
    }));
    let frame = ImeState {
        committed: Some("甲\n乙😀".into()),
        preedit: "a".into(),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, frame, false, false, None).unwrap();
    assert_eq!(host.starts.get(), 2);
    assert_eq!(&*host.writes.borrow(), &["甲\r\n乙😀", "a"]);
    assert_eq!(host.end_count.get(), 1);
    assert_eq!(host.active_compositions.get(), 1);
    assert!(host.queued.borrow().is_empty());
    assert!(state.borrow().composition.is_some());
    assert_eq!(state.borrow().ime_state.as_ref().unwrap().preedit, "a");
}

#[test]
fn a_deferred_preedit_never_overwrites_a_newer_input_frame() {
    for run_pending_first in [false, true] {
        let (host, context, state) = fixture();
        state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
        host.refuse_sync_once.set(true);
        host.reject_overlapping_compositions.set(true);
        let frame = ImeState {
            committed: Some("甲".into()),
            preedit: "a".into(),
            ..ImeState::empty()
        };
        apply_ime_state(&context, 1, &state, frame, false, false, None).unwrap();
        assert!(host.writes.borrow().is_empty());
        if run_pending_first {
            host.run_queued();
        }
        let newer = ImeState {
            preedit: "ab".into(),
            ..ImeState::empty()
        };
        if !run_pending_first {
            assert!(
                apply_ime_state(&context, 1, &state, newer.clone(), false, false, None).is_err()
            );
            assert!(host.writes.borrow().is_empty());
            host.run_queued();
        }
        apply_ime_state(&context, 1, &state, newer, false, false, None).unwrap();
        host.run_queued();
        assert_eq!(state.borrow().ime_state.as_ref().unwrap().preedit, "ab");
        assert_eq!(&*host.writes.borrow(), &["甲", "a", "ab"]);
        assert_eq!(host.active_compositions.get(), 1);
    }
}

#[test]
fn a_commit_does_not_need_an_async_end_request() {
    let (host, context, state) = fixture();
    host.fail_async_request.set(true);
    let frame = ImeState {
        committed: Some("甲\n乙😀".into()),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, frame, false, false, None).unwrap();
    assert_eq!(host.end_count.get(), 1);
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert!(state.borrow().pending_commit_retry.is_none());
    assert_eq!(&*host.text.borrow(), "甲\r\n乙😀");
    assert_eq!(host.writes.borrow().len(), 1);
    assert!(host.queued.borrow().is_empty());
}

#[test]
fn a_queued_retry_does_not_survive_an_engine_reload() {
    let (host, context, state) = fixture();
    queue(&context, &state);
    flush_pending_commit_retry(&state);
    let stamp = std::env::temp_dir().join(format!(
        "keytao-retry-reload-{}-{}.stamp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&stamp, "reload").unwrap();
    state.borrow_mut().reload_stamp_path = Some(stamp.clone());
    assert!(state.borrow_mut().begin_reload_if_changed());
    host.run_queued();
    assert_eq!(host.insert_queries.get(), 0);
    std::fs::remove_file(stamp).unwrap();
}

#[test]
fn a_failed_request_still_retries_a_commit_that_never_reached_the_document() {
    let (host, context, state) = fixture();
    host.fail_start.set(Some(1));
    let frame = ImeState {
        committed: Some("甲".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(host.queued.borrow().len(), 1);
    host.run_queued();
    assert_eq!(&*host.text.borrow(), "甲");
    assert_eq!(&*host.writes.borrow(), &["甲"]);
    assert!(!host.reentered_request.get());
    assert!(state.borrow().pending_commit_retry.is_none());
}

#[test]
fn an_inline_async_failure_finishes_its_request_before_retrying_the_commit() {
    let (host, context, state) = fixture();
    host.inline_async.set(true);
    host.fail_start.set(Some(1));
    let frame = ImeState {
        committed: Some("甲".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, true, None).is_err());
    assert!(!host.reentered_request.get());
    assert_eq!(&*host.writes.borrow(), &["甲"]);
    assert!(host.queued.borrow().is_empty());
    assert!(state.borrow().pending_ime_apply.is_none());
    assert!(state.borrow().pending_commit_retry.is_none());
}

#[test]
fn an_error_after_reentrant_focus_change_does_not_clear_the_new_input_state() {
    let (host, context, state) = fixture();
    let reentered = state.clone();
    *host.selection_hook.borrow_mut() = Some(Box::new(move || {
        reset_input_for_focus_change(&reentered);
        reentered.borrow_mut().ime_state = Some(ImeState {
            preedit: "new input".into(),
            ..ImeState::empty()
        });
    }));
    host.fail_selection.set(Some(1));
    let frame = ImeState {
        committed: Some("old input".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(
        state.borrow().ime_state.as_ref().unwrap().preedit,
        "new input"
    );
    assert!(host.queued.borrow().is_empty());
    assert!(state.borrow().pending_commit_retry.is_none());
}

#[test]
fn a_reentrant_get_range_cannot_insert_a_commit_or_preedit_into_the_old_editor() {
    for preedit in [false, true] {
        let (host, context, state) = fixture();
        state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
        let reentered = state.clone();
        *host.range_hook.borrow_mut() = Some(Box::new(move || {
            reset_input_for_focus_change(&reentered);
            reentered.borrow_mut().ime_state = Some(ImeState {
                preedit: "new input".into(),
                ..ImeState::empty()
            });
        }));
        let frame = if preedit {
            ImeState {
                preedit: "old preedit".into(),
                ..ImeState::empty()
            }
        } else {
            ImeState {
                committed: Some("old commit".into()),
                ..ImeState::empty()
            }
        };
        assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
        assert!(host.writes.borrow().is_empty());
        assert_eq!(
            state.borrow().ime_state.as_ref().unwrap().preedit,
            "new input"
        );
        assert!(state.borrow().pending_commit_retry.is_none());
    }
}

#[test]
fn a_retry_detects_focus_changes_inside_get_range_before_writing() {
    let (host, context, state) = fixture();
    queue(&context, &state);
    flush_pending_commit_retry(&state);
    let reentered = state.clone();
    *host.range_hook.borrow_mut() =
        Some(Box::new(move || reset_input_for_focus_change(&reentered)));
    let session = host.queued.borrow_mut().pop().unwrap();
    assert!(unsafe { session.DoEditSession(1) }.is_err());
    assert!(host.writes.borrow().is_empty());
    assert!(state.borrow().pending_commit_retry.is_none());
}

#[test]
fn a_failed_first_text_write_does_not_erase_the_original_selection() {
    let (host, context, state) = fixture();
    *host.text.borrow_mut() = "original selection".into();
    host.fail_text.set(true);
    let frame = ImeState {
        committed: Some("new commit".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(&*host.text.borrow(), "original selection");
    assert!(host.writes.borrow().is_empty());
    assert_eq!(host.end_count.get(), 1);
    assert_eq!(host.queued.borrow().len(), 1);
    reset_input_for_focus_change(&state);
    host.run_queued();
    assert_eq!(&*host.text.borrow(), "original selection");
    assert!(host.writes.borrow().is_empty());
}

#[test]
fn a_failed_new_preedit_is_ended_and_cleared_instead_of_leaking_its_composition() {
    let (host, context, state) = fixture();
    state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
    host.fail_selection.set(Some(1));
    let frame = ImeState {
        preedit: "a".into(),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(host.starts.get(), 1);
    assert_eq!(host.end_count.get(), 1);
    assert_eq!(&*host.text.borrow(), "");
    assert!(state.borrow().composition.is_none());
}

#[test]
fn failed_end_and_terminate_keep_the_handle_until_a_due_end_retry_succeeds() {
    let (host, context, state) = fixture();
    host.fail_end.set(true);
    host.fail_terminate.set(true);
    let frame = ImeState {
        committed: Some("甲".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    assert_eq!(&*host.text.borrow(), "甲");
    assert_eq!(state.borrow().pending_composition_ends.len(), 1);
    assert!(host.terminate_count.get() >= 1);
    let composition = host.last_composition.borrow().clone().unwrap();
    assert_eq!(
        state.borrow().pending_composition_ends[0].composition,
        composition
    );
    state.borrow_mut().pending_composition_ends[0].retry_after =
        Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
    let requests = host.session_flags.borrow().len();
    flush_pending_ime_ui(&state);
    assert_eq!(host.session_flags.borrow().len(), requests);
    assert!(host.queued.borrow().is_empty());
    assert_eq!(
        state.borrow().pending_composition_ends[0].composition,
        composition
    );
    state.borrow_mut().ime_state = Some(ImeState {
        preedit: "new input".into(),
        ..ImeState::empty()
    });
    host.fail_end.set(false);
    state.borrow_mut().pending_composition_ends[0].retry_after = None;
    flush_pending_ime_ui(&state);
    assert_eq!(host.queued.borrow().len(), 1);
    assert_eq!(
        state.borrow().pending_composition_ends[0].composition,
        composition
    );
    host.run_queued();
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert_eq!(host.active_compositions.get(), 0);
    assert_eq!(
        state.borrow().ime_state.as_ref().unwrap().preedit,
        "new input"
    );
    assert_eq!(&*host.text.borrow(), "甲");
    assert_eq!(host.writes.borrow().len(), 1);
}

#[test]
fn a_late_termination_from_a_failed_old_end_does_not_clear_new_input() {
    let (host, context, state) = fixture();
    host.fail_end.set(true);
    host.fail_terminate.set(true);
    let frame = ImeState {
        committed: Some("甲".into()),
        ..ImeState::empty()
    };
    assert!(apply_ime_state(&context, 1, &state, frame, false, false, None).is_err());
    state.borrow_mut().ime_state = Some(ImeState {
        preedit: "new input".into(),
        ..ImeState::empty()
    });
    let sink = host.last_sink.borrow().clone().unwrap();
    let composition = host.last_composition.borrow().clone().unwrap();
    unsafe { sink.OnCompositionTerminated(1, &composition).unwrap() };
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert_eq!(
        state.borrow().ime_state.as_ref().unwrap().preedit,
        "new input"
    );
}

#[test]
fn a_retry_that_cannot_end_retains_its_composition_without_reinserting_text() {
    let (host, context, state) = fixture();
    host.fail_end.set(true);
    host.fail_terminate.set(true);
    queue(&context, &state);
    flush_pending_commit_retry(&state);
    let session = host.queued.borrow_mut().pop().unwrap();
    assert!(unsafe { session.DoEditSession(1) }.is_err());
    assert_eq!(state.borrow().pending_composition_ends.len(), 1);
    flush_pending_ime_ui(&state);
    assert_eq!(&*host.text.borrow(), "retry");
    assert_eq!(host.writes.borrow().len(), 1);
    assert_eq!(state.borrow().pending_composition_ends.len(), 1);
}

#[test]
fn refused_sync_composition_end_queues_async_immediately_without_timer_backoff() {
    let (host, context, state) = fixture();
    state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
    let preedit = ImeState {
        preedit: "y".into(),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, preedit, false, false, None).unwrap();
    assert_eq!(&*host.text.borrow(), "y");

    host.refuse_sync_once.set(true);
    reset_input_for_focus_change(&state);
    assert_eq!(host.queued.borrow().len(), 1);
    assert!(state.borrow().pending_composition_ends[0].retry_after.is_none());

    host.run_queued();
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert_eq!(&*host.text.borrow(), "");
}

#[test]
fn five_refused_ends_release_apply_and_clean_up_raw_preedit_on_next_write_session() {
    let (host, context, state) = fixture();
    state.borrow_mut().thread_mgr_flags = TF_TMF_UIELEMENTENABLEDONLY;
    let preedit = ImeState {
        preedit: "y".into(),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, preedit, false, false, None).unwrap();
    assert_eq!(&*host.text.borrow(), "y");

    host.fail_end.set(true);
    host.fail_async_request.set(true);
    reset_input_for_focus_change(&state);
    for _ in 0..4 {
        state.borrow_mut().pending_composition_ends[0].retry_after = None;
        flush_pending_ime_ui(&state);
    }
    assert_eq!(state.borrow().pending_composition_ends.len(), 1);
    assert!(state.borrow().pending_composition_ends[0].exhausted);
    assert!(!super::has_active_composition_ends(&state.borrow()));

    host.fail_end.set(false);
    host.fail_async_request.set(false);
    let next = ImeState {
        committed: Some("字".into()),
        ..ImeState::empty()
    };
    apply_ime_state(&context, 1, &state, next, false, false, None).unwrap();
    assert!(state.borrow().pending_composition_ends.is_empty());
    assert!(state.borrow().written_preedits.is_empty());
    assert_eq!(&*host.text.borrow(), "字");
}
