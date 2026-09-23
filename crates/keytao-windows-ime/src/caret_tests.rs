//! Exercise the actual COM probe against a text store with no adjacent text.
//! Some hosts expose a valid collapsed caret even when both shifts return zero.

use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    rc::Rc,
};

use keytao_core::ImeState;
use windows::{
    core::{implement, IUnknown, Result, GUID, HRESULT, PCWSTR, PWSTR},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_NOTIMPL, HWND, POINT, RECT},
        System::Com::IDataObject,
        UI::TextServices::*,
    },
};

// Keep unexercised COM methods explicit; accidentally using one fails the probe.
macro_rules! unimplemented_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> {
            Err(E_NOTIMPL.into())
        })*
    };
}

struct TextStore {
    collapsed_rect: RECT,
    clipped: bool,
    extent_fails: bool,
    has_adjacent_text: bool,
    viewport: Option<RECT>,
    viewport_queries: Cell<usize>,
    extent_queries: RefCell<Vec<bool>>,
    shift_requests: RefCell<Vec<i32>>,
}

impl TextStore {
    fn empty() -> Self {
        Self {
            collapsed_rect: RECT {
                left: 120,
                top: 200,
                right: 120,
                bottom: 220,
            },
            clipped: false,
            extent_fails: false,
            has_adjacent_text: false,
            viewport: None,
            viewport_queries: Cell::new(0),
            extent_queries: RefCell::new(Vec::new()),
            shift_requests: RefCell::new(Vec::new()),
        }
    }

    fn range(self: &Rc<Self>, collapsed: bool) -> ITfRange {
        MockRange {
            store: self.clone(),
            collapsed: Cell::new(collapsed),
        }
        .into()
    }

    fn context(self: &Rc<Self>) -> ITfContext {
        MockContext {
            range: self.range(true),
            view: MockView {
                store: self.clone(),
            }
            .into(),
        }
        .into()
    }
}

#[implement(ITfRange)]
struct MockRange {
    store: Rc<TextStore>,
    collapsed: Cell<bool>,
}

impl MockRange_Impl {
    fn shift(&self, requested: i32, shifted: *mut i32) -> Result<()> {
        self.store.shift_requests.borrow_mut().push(requested);
        let count = if self.store.has_adjacent_text {
            requested
        } else {
            0
        };
        unsafe {
            *shifted = count;
        }
        if count != 0 {
            self.collapsed.set(false);
        }
        Ok(())
    }
}

impl ITfRange_Impl for MockRange_Impl {
    fn ShiftStart(
        &self,
        _ec: u32,
        requested: i32,
        shifted: *mut i32,
        _halt: *const TF_HALTCOND,
    ) -> Result<()> {
        self.shift(requested, shifted)
    }

    fn ShiftEnd(
        &self,
        _ec: u32,
        requested: i32,
        shifted: *mut i32,
        _halt: *const TF_HALTCOND,
    ) -> Result<()> {
        self.shift(requested, shifted)
    }

    fn IsEmpty(&self, _ec: u32) -> Result<BOOL> {
        Ok(BOOL::from(self.collapsed.get()))
    }

    fn Collapse(&self, _ec: u32, _anchor: TfAnchor) -> Result<()> {
        self.collapsed.set(true);
        Ok(())
    }

    fn Clone(&self) -> Result<ITfRange> {
        Ok(self.store.range(self.collapsed.get()))
    }

    unimplemented_methods! {
        fn GetText(&self, ec: u32, flags: u32, text: PWSTR, max: u32, count: *mut u32) -> ();
        fn SetText(&self, ec: u32, flags: u32, text: &PCWSTR, count: i32) -> ();
        fn GetFormattedText(&self, ec: u32) -> IDataObject;
        fn GetEmbedded(&self, ec: u32, service: *const GUID, iid: *const GUID) -> IUnknown;
        fn InsertEmbedded(&self, ec: u32, flags: u32, data: Option<&IDataObject>) -> ();
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

#[implement(ITfContextView)]
struct MockView {
    store: Rc<TextStore>,
}

impl ITfContextView_Impl for MockView_Impl {
    fn GetTextExt(
        &self,
        ec: u32,
        range: Option<&ITfRange>,
        rect: *mut RECT,
        clipped: *mut BOOL,
    ) -> Result<()> {
        let collapsed = unsafe { range.unwrap().IsEmpty(ec)?.as_bool() };
        self.store.extent_queries.borrow_mut().push(collapsed);
        unsafe {
            *rect = if collapsed {
                self.store.collapsed_rect
            } else {
                RECT {
                    left: 300,
                    top: 400,
                    right: 310,
                    bottom: 420,
                }
            };
            *clipped = BOOL::from(self.store.clipped);
        }
        if self.store.extent_fails {
            Err(E_FAIL.into())
        } else {
            Ok(())
        }
    }

    fn GetWnd(&self) -> Result<HWND> {
        // A non-window sentinel avoids consulting the user's focused window.
        // MonitorFromWindow's nearest-monitor fallback uses the primary display.
        Ok(HWND(1usize as *mut _))
    }

    fn GetScreenExt(&self) -> Result<RECT> {
        self.store
            .viewport_queries
            .set(self.store.viewport_queries.get() + 1);
        self.store.viewport.ok_or_else(|| E_NOTIMPL.into())
    }

    unimplemented_methods! {
        fn GetRangeFromPoint(&self, ec: u32, point: *const POINT, flags: u32) -> ITfRange;
    }
}

#[implement(ITfContext)]
struct MockContext {
    range: ITfRange,
    view: ITfContextView,
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
        assert!(count > 0);
        unsafe {
            *selection = TF_SELECTION {
                range: ManuallyDrop::new(Some(self.range.clone())),
                style: TF_SELECTIONSTYLE {
                    ase: TF_AE_END,
                    fInterimChar: BOOL(0),
                },
            };
            *fetched = 1;
        }
        Ok(())
    }

    fn GetActiveView(&self) -> Result<ITfContextView> {
        Ok(self.view.clone())
    }

    unimplemented_methods! {
        fn RequestEditSession(&self, tid: u32, session: Option<&ITfEditSession>, flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS) -> HRESULT;
        fn InWriteSession(&self, tid: u32) -> BOOL;
        fn SetSelection(&self, ec: u32, count: u32, selection: *const TF_SELECTION) -> ();
        fn GetStart(&self, ec: u32) -> ITfRange;
        fn GetEnd(&self, ec: u32) -> ITfRange;
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

fn probe(store: &Rc<TextStore>) -> Option<(i32, i32)> {
    super::probe_caret(0, &store.context(), None, &ImeState::empty(), false).position
}

#[test]
fn empty_editor_uses_collapsed_caret_when_adjacent_shifts_return_zero() {
    let store = Rc::new(TextStore::empty());

    assert_eq!(probe(&store), Some((120, 220)));
    assert_eq!(*store.shift_requests.borrow(), [1, -1]);
    assert_eq!(*store.extent_queries.borrow(), [true]);
}

#[test]
fn empty_editor_rejects_zero_height_caret() {
    let mut store = TextStore::empty();
    store.collapsed_rect.bottom = store.collapsed_rect.top;
    let store = Rc::new(store);

    assert_eq!(probe(&store), None);
    assert_eq!(*store.extent_queries.borrow(), [true]);
}

#[test]
fn empty_editor_rejects_clipped_caret() {
    let store = Rc::new(TextStore {
        clipped: true,
        ..TextStore::empty()
    });

    assert_eq!(probe(&store), None);
    assert_eq!(*store.extent_queries.borrow(), [true]);
}

#[test]
fn empty_editor_rejects_coordinates_returned_with_failed_extent() {
    let store = Rc::new(TextStore {
        extent_fails: true,
        ..TextStore::empty()
    });

    assert_eq!(probe(&store), None);
    assert_eq!(*store.extent_queries.borrow(), [true]);
}

#[test]
fn adjacent_character_extent_remains_preferred_over_collapsed_caret() {
    let store = Rc::new(TextStore {
        has_adjacent_text: true,
        ..TextStore::empty()
    });

    assert_eq!(probe(&store), Some((300, 420)));
    assert_eq!(*store.shift_requests.borrow(), [1]);
    assert_eq!(*store.extent_queries.borrow(), [false]);
}

#[test]
fn unusable_text_extents_keep_a_viewport_without_inventing_a_caret() {
    let viewport = RECT {
        left: 100,
        top: 200,
        right: 900,
        bottom: 800,
    };
    let store = Rc::new(TextStore {
        collapsed_rect: RECT {
            left: 5119,
            top: 2783,
            right: 5120,
            bottom: 2783,
        },
        viewport: Some(viewport),
        ..TextStore::empty()
    });
    let probe = super::probe_caret(0, &store.context(), None, &ImeState::empty(), false);
    assert_eq!(
        probe.position, None,
        "WPS zero-height sentinel is not a caret"
    );
    assert_eq!(probe.viewport, Some(viewport));
    assert_eq!(store.viewport_queries.get(), 1);
}

#[test]
fn fresh_caret_does_not_query_or_choose_a_fallback_viewport() {
    let store = Rc::new(TextStore {
        viewport: Some(RECT {
            left: 100,
            top: 200,
            right: 900,
            bottom: 800,
        }),
        ..TextStore::empty()
    });
    let probe = super::probe_caret(0, &store.context(), None, &ImeState::empty(), false);
    assert_eq!(probe.position, Some((120, 220)));
    assert_eq!(probe.viewport, None);
    assert_eq!(store.viewport_queries.get(), 0);
}

#[test]
fn failed_or_malformed_viewports_are_not_retained() {
    for viewport in [
        None,
        Some(RECT::default()),
        Some(RECT {
            left: 100,
            top: 200,
            right: 100,
            bottom: 800,
        }),
        Some(RECT {
            left: 100,
            top: 200,
            right: 900,
            bottom: 200,
        }),
        Some(RECT {
            left: 900,
            top: 200,
            right: 100,
            bottom: 800,
        }),
    ] {
        let store = Rc::new(TextStore {
            extent_fails: true,
            viewport,
            ..TextStore::empty()
        });
        let probe = super::probe_caret(0, &store.context(), None, &ImeState::empty(), false);
        assert_eq!(probe.position, None);
        assert_eq!(probe.viewport, None);
    }
}

#[test]
fn viewport_anchor_uses_only_the_visible_owner_intersection() {
    let client = RECT {
        left: 100,
        top: 200,
        right: 900,
        bottom: 800,
    };
    let viewport = RECT {
        left: 50,
        top: 300,
        right: 500,
        bottom: 1200,
    };
    assert_eq!(
        super::panel_fallback_position(Some(viewport), Some(client), true),
        Some(((108, 308), super::CaretSource::View))
    );
    let inside = RECT {
        left: 300,
        top: 400,
        right: 500,
        bottom: 600,
    };
    assert_eq!(
        super::panel_fallback_position(Some(inside), Some(client), true),
        Some(((308, 408), super::CaretSource::View))
    );
}

#[test]
fn rejected_viewport_falls_back_to_the_validated_window_client() {
    let client = RECT {
        left: 100,
        top: 200,
        right: 900,
        bottom: 800,
    };
    for viewport in [
        None,
        Some(RECT::default()),
        Some(RECT {
            left: 950,
            top: 200,
            right: 1200,
            bottom: 800,
        }),
        Some(RECT {
            left: 900,
            top: 200,
            right: 1200,
            bottom: 800,
        }),
        Some(RECT {
            left: 200,
            top: 500,
            right: 100,
            bottom: 600,
        }),
    ] {
        assert_eq!(
            super::panel_fallback_position(viewport, Some(client), true),
            Some(((108, 208), super::CaretSource::Window))
        );
    }
}

#[test]
fn viewports_cannot_anchor_mode_hints_or_bypass_missing_validated_owner() {
    let viewport = RECT {
        left: 100,
        top: 200,
        right: 900,
        bottom: 800,
    };
    assert_eq!(
        super::panel_fallback_position(Some(viewport), Some(viewport), false),
        None
    );
    for client in [None, Some(RECT::default())] {
        assert_eq!(
            super::panel_fallback_position(Some(viewport), client, true),
            None
        );
    }
}

#[test]
fn fallback_inset_stays_inside_small_negative_and_extreme_rectangles() {
    for area in [
        RECT {
            left: 100,
            top: 200,
            right: 101,
            bottom: 201,
        },
        RECT {
            left: -1900,
            top: -900,
            right: -100,
            bottom: -100,
        },
        RECT {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        },
    ] {
        let ((x, y), source) = super::panel_fallback_position(None, Some(area), true).unwrap();
        assert_eq!(source, super::CaretSource::Window);
        assert!(x >= area.left && x < area.right);
        assert!(y >= area.top && y < area.bottom);
    }
}

#[test]
fn fallback_sources_keep_the_bounded_caret_retry_active() {
    for source in [
        super::CaretSource::Accessibility,
        super::CaretSource::View,
        super::CaretSource::Window,
    ] {
        assert!(super::should_arm_caret_reprobe(Some(source)));
    }
    assert!(!super::should_arm_caret_reprobe(Some(
        super::CaretSource::Probe
    )));
}

#[test]
fn synchronous_focus_changes_cannot_revive_an_old_window_update() {
    assert!(super::window_update_context_matches(true, 1, Some(1), None));
    assert!(super::window_update_context_matches(true, 1, None, Some(1)));
    assert!(!super::window_update_context_matches(
        false,
        1,
        Some(1),
        Some(1)
    ));
    assert!(!super::window_update_context_matches(true, 1, None, None));
    assert!(!super::window_update_context_matches(
        true,
        1,
        Some(2),
        Some(1)
    ));
    assert!(!super::window_update_context_matches(
        true,
        1,
        None,
        Some(2)
    ));
}

#[test]
fn a_consumed_reentrant_update_invalidates_the_older_same_context_snapshot() {
    let older = super::next_window_update_generation();
    assert!(super::window_update_generation_is_current(older));
    // The provider synchronously flushes a newer update, so its pending slot
    // may already be empty when the older accessibility query returns.
    let newer = super::next_window_update_generation();
    assert!(super::window_update_generation_is_current(newer));
    assert!(!super::window_update_generation_is_current(older));
    // Further freshness checks themselves do not advance the generation.
    assert!(super::window_update_generation_is_current(newer));
}

#[test]
fn returning_to_the_same_context_after_focus_reset_does_not_revive_old_ui() {
    let store = Rc::new(TextStore::empty());
    let context = store.context();
    let state = crate::state::new_shared_state();
    {
        let mut st = state.borrow_mut();
        st.key_context = Some(context.clone());
        st.ime_state = Some(ImeState::empty());
    }
    let update = super::PendingWindowUpdate {
        context: context.clone(),
        input_generation: state.borrow().input_generation,
        ime_state: ImeState::empty(),
        caret: super::CaretProbe {
            owner_hwnd: HWND::default(),
            position: None,
            viewport: None,
        },
        document_mgr: None,
        show_mode_hint: false,
        embedded: false,
        reposition_only: false,
        key_started: None,
    };
    let generation = super::next_window_update_generation();
    assert!(super::window_update_is_current(&state, &update, generation));
    crate::state::reset_input_for_focus_change(&state);
    {
        let mut st = state.borrow_mut();
        st.key_context = Some(context);
        st.ime_state = Some(ImeState::empty());
    }
    assert!(!super::window_update_is_current(
        &state, &update, generation
    ));
}
