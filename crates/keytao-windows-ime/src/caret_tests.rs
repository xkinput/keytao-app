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

    unimplemented_methods! {
        fn GetRangeFromPoint(&self, ec: u32, point: *const POINT, flags: u32) -> ITfRange;
        fn GetScreenExt(&self) -> RECT;
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
