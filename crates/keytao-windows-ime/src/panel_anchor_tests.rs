//! In-memory TSF COM objects: no native windows, registration, or host input.

use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    rc::Rc,
};

use windows::{
    core::{
        implement, AsImpl, Error, IUnknown, IUnknownImpl, Interface, Result, GUID, HRESULT, PCWSTR,
        PWSTR,
    },
    Win32::{
        Foundation::{BOOL, E_FAIL, E_NOTIMPL, E_UNEXPECTED, S_OK},
        System::Com::IDataObject,
        UI::TextServices::*,
    },
};

use super::start_panel_anchor;

macro_rules! unimplemented_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> {
            Err(E_NOTIMPL.into())
        })*
    };
}

#[derive(Default)]
struct Operations {
    text_writes: Cell<usize>,
    selection_writes: Cell<usize>,
    composition_starts: RefCell<Vec<(i32, i32)>>,
    composition_ends: Cell<usize>,
    composition_ranges: Cell<usize>,
    range_clone_fails: Cell<bool>,
    range_collapse_fails: Cell<bool>,
    range_collapse_ineffective: Cell<bool>,
}

#[derive(Clone, Copy, Default)]
enum StartBehavior {
    #[default]
    Live,
    Refuse,
    Fail,
    Terminate,
}

struct Store {
    operations: Rc<Operations>,
    // A nonempty document lets tests detect accidental changes on cancellation.
    document: RefCell<String>,
    selection: Rc<Cell<(i32, i32)>>,
    active_end: TfActiveSelEnd,
    selection_hr: Cell<HRESULT>,
    selection_count: Cell<u32>,
    selection_missing: Cell<bool>,
    start_behavior: Cell<StartBehavior>,
    last_sink: RefCell<Option<ITfCompositionSink>>,
    end_hr: Cell<HRESULT>,
    terminate_on_end: Cell<bool>,
    terminate_on_get_range: Cell<bool>,
    end_hook: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Store {
    fn new(selection: (i32, i32), active_end: TfActiveSelEnd) -> Rc<Self> {
        Rc::new(Self {
            operations: Rc::new(Operations::default()),
            document: RefCell::new("original selected text".to_owned()),
            selection: Rc::new(Cell::new(selection)),
            active_end,
            selection_hr: Cell::new(S_OK),
            selection_count: Cell::new(1),
            selection_missing: Cell::new(false),
            start_behavior: Cell::new(StartBehavior::Live),
            last_sink: RefCell::new(None),
            end_hr: Cell::new(S_OK),
            terminate_on_end: Cell::new(false),
            terminate_on_get_range: Cell::new(false),
            end_hook: RefCell::new(None),
        })
    }

    fn context(self: &Rc<Self>) -> ITfContext {
        MockContext {
            store: Rc::clone(self),
        }
        .into()
    }

    fn assert_untouched(&self, expected_selection: (i32, i32)) {
        assert_eq!(&*self.document.borrow(), "original selected text");
        assert_eq!(self.selection.get(), expected_selection);
        assert_eq!(self.operations.text_writes.get(), 0);
        assert_eq!(self.operations.selection_writes.get(), 0);
    }
}

#[implement(ITfRange)]
struct MockRange {
    operations: Rc<Operations>,
    span: Rc<Cell<(i32, i32)>>,
}

impl ITfRange_Impl for MockRange_Impl {
    fn Clone(&self) -> Result<ITfRange> {
        if self.operations.range_clone_fails.get() {
            return Err(E_FAIL.into());
        }
        Ok(MockRange {
            operations: Rc::clone(&self.operations),
            span: Rc::new(Cell::new(self.span.get())),
        }
        .into())
    }

    fn Collapse(&self, _ec: u32, anchor: TfAnchor) -> Result<()> {
        if self.operations.range_collapse_fails.get() {
            return Err(E_FAIL.into());
        }
        if !self.operations.range_collapse_ineffective.get() {
            let (start, end) = self.span.get();
            let position = if anchor == TF_ANCHOR_START {
                start
            } else {
                end
            };
            self.span.set((position, position));
        }
        Ok(())
    }

    fn IsEmpty(&self, _ec: u32) -> Result<BOOL> {
        let (start, end) = self.span.get();
        Ok(BOOL::from(start == end))
    }

    fn SetText(&self, _ec: u32, _flags: u32, _text: &PCWSTR, _count: i32) -> Result<()> {
        self.operations
            .text_writes
            .set(self.operations.text_writes.get() + 1);
        Err(E_NOTIMPL.into())
    }

    unimplemented_methods! {
        fn GetText(&self, ec: u32, flags: u32, text: PWSTR, max: u32, count: *mut u32) -> ();
        fn GetFormattedText(&self, ec: u32) -> IDataObject;
        fn GetEmbedded(&self, ec: u32, service: *const GUID, iid: *const GUID) -> IUnknown;
        fn InsertEmbedded(&self, ec: u32, flags: u32, data: Option<&IDataObject>) -> ();
        fn ShiftStart(&self, ec: u32, requested: i32, shifted: *mut i32, halt: *const TF_HALTCOND) -> ();
        fn ShiftEnd(&self, ec: u32, requested: i32, shifted: *mut i32, halt: *const TF_HALTCOND) -> ();
        fn ShiftStartToRange(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> ();
        fn ShiftEndToRange(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> ();
        fn ShiftStartRegion(&self, ec: u32, direction: TfShiftDir) -> BOOL;
        fn ShiftEndRegion(&self, ec: u32, direction: TfShiftDir) -> BOOL;
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

#[implement(ITfContext, ITfContextComposition)]
struct MockContext {
    store: Rc<Store>,
}

impl ITfContext_Impl for MockContext_Impl {
    fn GetSelection(
        &self,
        _ec: u32,
        index: u32,
        count: u32,
        selection: *mut TF_SELECTION,
        fetched: *mut u32,
    ) -> Result<()> {
        assert_eq!(index, TF_DEFAULT_SELECTION);
        assert_eq!(count, 1);
        let range = (!self.store.selection_missing.get()).then(|| {
            MockRange {
                operations: Rc::clone(&self.store.operations),
                span: Rc::clone(&self.store.selection),
            }
            .into()
        });
        unsafe {
            *selection = TF_SELECTION {
                range: ManuallyDrop::new(range),
                style: TF_SELECTIONSTYLE {
                    ase: self.store.active_end,
                    fInterimChar: BOOL(0),
                },
            };
            *fetched = self.store.selection_count.get();
        }
        self.store.selection_hr.get().ok()
    }

    fn SetSelection(&self, _ec: u32, _count: u32, _selection: *const TF_SELECTION) -> Result<()> {
        let operations = &self.store.operations;
        operations
            .selection_writes
            .set(operations.selection_writes.get() + 1);
        Err(E_NOTIMPL.into())
    }

    unimplemented_methods! {
        fn RequestEditSession(&self, tid: u32, session: Option<&ITfEditSession>, flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS) -> HRESULT;
        fn InWriteSession(&self, tid: u32) -> BOOL;
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

impl ITfContextComposition_Impl for MockContext_Impl {
    fn StartComposition(
        &self,
        ec: u32,
        range: Option<&ITfRange>,
        sink: Option<&ITfCompositionSink>,
    ) -> Result<ITfComposition> {
        let range = range.unwrap();
        let mock: &MockRange = unsafe { range.as_impl() };
        self.store
            .operations
            .composition_starts
            .borrow_mut()
            .push(mock.span.get());
        let sink = sink.unwrap().clone();
        *self.store.last_sink.borrow_mut() = Some(sink.clone());
        match self.store.start_behavior.get() {
            // windows' generated ABI maps Error::empty() back to S_OK without
            // filling ppComposition. This exercises a real S_OK/null refusal.
            StartBehavior::Refuse => return Err(Error::empty()),
            StartBehavior::Fail => return Err(E_FAIL.into()),
            _ => {}
        }
        let composition: ITfComposition = MockComposition {
            store: Rc::clone(&self.store),
            range: range.clone(),
            sink: sink.clone(),
        }
        .into();
        if matches!(self.store.start_behavior.get(), StartBehavior::Terminate) {
            unsafe {
                sink.OnCompositionTerminated(ec, &composition)?;
            }
        }
        Ok(composition)
    }

    unimplemented_methods! {
        fn EnumCompositions(&self) -> IEnumITfCompositionView;
        fn FindComposition(&self, ec: u32, range: Option<&ITfRange>) -> IEnumITfCompositionView;
        fn TakeOwnership(&self, ec: u32, composition: Option<&ITfCompositionView>, sink: Option<&ITfCompositionSink>) -> ITfComposition;
    }
}

#[implement(ITfComposition)]
struct MockComposition {
    store: Rc<Store>,
    range: ITfRange,
    sink: ITfCompositionSink,
}

impl ITfComposition_Impl for MockComposition_Impl {
    fn GetRange(&self) -> Result<ITfRange> {
        let operations = &self.store.operations;
        operations
            .composition_ranges
            .set(operations.composition_ranges.get() + 1);
        if self.store.terminate_on_get_range.get() {
            unsafe {
                self.sink
                    .OnCompositionTerminated(7, &self.to_interface::<ITfComposition>())?;
            }
        }
        Ok(self.range.clone())
    }

    fn EndComposition(&self, ec: u32) -> Result<()> {
        let operations = &self.store.operations;
        operations
            .composition_ends
            .set(operations.composition_ends.get() + 1);
        let hook = self.store.end_hook.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
        if self.store.terminate_on_end.get() {
            unsafe {
                self.sink
                    .OnCompositionTerminated(ec, &self.to_interface::<ITfComposition>())?;
            }
        }
        self.store.end_hr.get().ok()
    }

    unimplemented_methods! {
        fn ShiftStart(&self, ec: u32, start: Option<&ITfRange>) -> ();
        fn ShiftEnd(&self, ec: u32, end: Option<&ITfRange>) -> ();
    }
}

#[test]
fn collapsed_anchor_has_no_document_or_selection_writes() {
    let store = Store::new((8, 8), TF_AE_END);
    let context = store.context();
    let anchor = start_panel_anchor(7, &context).unwrap().unwrap();
    assert_eq!(anchor.context().as_raw(), context.as_raw());
    assert!(anchor.composition().is_some());
    assert_eq!(&*store.operations.composition_starts.borrow(), &[(8, 8)]);
    anchor.end(7).unwrap();
    assert!(anchor.composition().is_none());
    assert_eq!(store.operations.composition_ranges.get(), 0);
    store.assert_untouched((8, 8));
}

#[test]
fn forward_and_reverse_selections_keep_their_original_span_on_cancel() {
    for (active_end, expected) in [(TF_AE_START, 3), (TF_AE_END, 17)] {
        let store = Store::new((3, 17), active_end);
        let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
        assert_eq!(
            &*store.operations.composition_starts.borrow(),
            &[(expected, expected)]
        );
        store.assert_untouched((3, 17));
        anchor.end(7).unwrap();
        store.assert_untouched((3, 17));
    }
}

#[test]
fn host_refusal_is_none_without_modifying_the_document() {
    let store = Store::new((3, 17), TF_AE_END);
    store.start_behavior.set(StartBehavior::Refuse);
    assert!(start_panel_anchor(7, &store.context()).unwrap().is_none());
    store.assert_untouched((3, 17));
}

#[test]
fn host_start_error_keeps_text_and_selection_unchanged() {
    let store = Store::new((3, 17), TF_AE_END);
    store.start_behavior.set(StartBehavior::Fail);
    assert_eq!(
        start_panel_anchor(7, &store.context())
            .err()
            .unwrap()
            .code(),
        E_FAIL
    );
    store.assert_untouched((3, 17));
}

#[test]
fn bad_selections_never_start_a_composition() {
    for scenario in 0..4 {
        let store = Store::new((3, 17), TF_AE_END);
        match scenario {
            0 => store.selection_hr.set(TF_E_NOLOCK),
            1 => store.selection_count.set(0),
            2 => store.selection_count.set(2),
            _ => store.selection_missing.set(true),
        }
        assert!(start_panel_anchor(7, &store.context()).is_err());
        assert!(store.operations.composition_starts.borrow().is_empty());
        store.assert_untouched((3, 17));
    }
}

#[test]
fn failed_range_cloning_or_collapsing_never_uses_the_original_selection() {
    for scenario in 0..3 {
        let store = Store::new((3, 17), TF_AE_END);
        match scenario {
            0 => store.operations.range_clone_fails.set(true),
            1 => store.operations.range_collapse_fails.set(true),
            _ => store.operations.range_collapse_ineffective.set(true),
        }
        assert!(start_panel_anchor(7, &store.context()).is_err());
        assert!(store.operations.composition_starts.borrow().is_empty());
        store.assert_untouched((3, 17));
    }
}

#[test]
fn termination_inside_start_never_publishes_a_dead_composition() {
    let store = Store::new((8, 8), TF_AE_END);
    store.start_behavior.set(StartBehavior::Terminate);
    assert!(start_panel_anchor(7, &store.context()).unwrap().is_none());
    assert_eq!(store.operations.composition_ends.get(), 0);
    store.assert_untouched((8, 8));
}

#[test]
fn external_termination_invalidates_all_clones_without_cleanup_writes() {
    let store = Store::new((8, 8), TF_AE_END);
    let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
    let clone = anchor.clone();
    unsafe {
        store
            .last_sink
            .borrow()
            .as_ref()
            .unwrap()
            .OnCompositionTerminated(7, anchor.composition())
            .unwrap();
    }
    assert!(anchor.composition().is_none());
    assert!(clone.composition().is_none());
    anchor.end(7).unwrap();
    clone.end(7).unwrap();
    assert_eq!(store.operations.composition_ends.get(), 0);
    store.assert_untouched((8, 8));
}

#[test]
fn termination_inside_get_range_is_visible_after_the_host_call() {
    let store = Store::new((8, 8), TF_AE_END);
    let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
    store.terminate_on_get_range.set(true);
    unsafe {
        anchor.composition().unwrap().GetRange().unwrap();
    }
    assert!(anchor.composition().is_none());
    anchor.end(7).unwrap();
    assert_eq!(store.operations.composition_ends.get(), 0);
}

#[test]
fn successful_end_is_idempotent_even_without_a_host_callback() {
    let store = Store::new((8, 8), TF_AE_END);
    let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
    let clone = anchor.clone();
    anchor.end(7).unwrap();
    clone.end(7).unwrap();
    assert_eq!(store.operations.composition_ends.get(), 1);
    assert!(clone.composition().is_none());
}

#[test]
fn failed_end_can_be_retried_without_erasing_text() {
    let store = Store::new((3, 17), TF_AE_END);
    let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
    store.end_hr.set(TF_E_NOLOCK);
    assert_eq!(anchor.end(7).unwrap_err().code(), TF_E_NOLOCK);
    assert!(anchor.composition().is_some());
    store.end_hr.set(S_OK);
    anchor.end(7).unwrap();
    assert!(anchor.composition().is_none());
    assert_eq!(store.operations.composition_ends.get(), 2);
    store.assert_untouched((3, 17));
}

#[test]
fn termination_callback_wins_over_a_failed_end_result() {
    let store = Store::new((8, 8), TF_AE_END);
    let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
    store.end_hr.set(E_UNEXPECTED);
    store.terminate_on_end.set(true);
    anchor.end(7).unwrap();
    anchor.end(7).unwrap();
    assert!(anchor.composition().is_none());
    assert_eq!(store.operations.composition_ends.get(), 1);
}

#[test]
fn reentrant_end_is_rejected_without_a_second_native_operation() {
    let store = Store::new((8, 8), TF_AE_END);
    let anchor = start_panel_anchor(7, &store.context()).unwrap().unwrap();
    let clone = anchor.clone();
    *store.end_hook.borrow_mut() = Some(Box::new(move || {
        assert!(clone.composition().is_none());
        assert_eq!(clone.end(7).unwrap_err().code(), E_UNEXPECTED);
    }));
    anchor.end(7).unwrap();
    assert_eq!(store.operations.composition_ends.get(), 1);
}

#[test]
fn a_delayed_old_callback_cannot_terminate_the_replacement_anchor() {
    let store = Store::new((8, 8), TF_AE_END);
    let context = store.context();
    let old = start_panel_anchor(7, &context).unwrap().unwrap();
    let old_composition = old.composition().unwrap().clone();
    let old_sink = store.last_sink.borrow().as_ref().unwrap().clone();
    old.end(7).unwrap();
    let new = start_panel_anchor(7, &context).unwrap().unwrap();
    unsafe {
        old_sink
            .OnCompositionTerminated(7, &old_composition)
            .unwrap();
    }
    assert!(new.composition().is_some());
    new.end(7).unwrap();
    store.assert_untouched((8, 8));
}
