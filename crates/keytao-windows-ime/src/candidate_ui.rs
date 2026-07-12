//! TSF UIElement bridge for candidate lists.
//!
//! A host can ask the TIP not to draw its own window. In that case Windows
//! reads candidate data through ITfCandidateListUIElement and renders it in the
//! host UI. Desktop hosts that allow TIP UI continue to use CandidateWindow.

use std::{cell::RefCell, rc::Rc};

use keytao_core::ImeState;
use windows::{
    core::{implement, Interface, Result, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, S_FALSE},
        UI::TextServices::{
            ITfCandidateListUIElement, ITfCandidateListUIElement_Impl, ITfDocumentMgr,
            ITfThreadMgr, ITfUIElement, ITfUIElementMgr, ITfUIElement_Impl, TF_CLUIE_COUNT,
            TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION,
            TF_CLUIE_STRING,
        },
    },
};

use crate::globals::DllActivityGuard;

const CANDIDATE_UI_GUID: GUID = GUID {
    data1: 0x8d7f2864,
    data2: 0x69b8,
    data3: 0x4f8d,
    data4: [0x96, 0xeb, 0xc2, 0x58, 0x94, 0xc1, 0x44, 0x35],
};

const ALL_CANDIDATE_FLAGS: u32 = TF_CLUIE_COUNT
    | TF_CLUIE_DOCUMENTMGR
    | TF_CLUIE_SELECTION
    | TF_CLUIE_STRING
    | TF_CLUIE_PAGEINDEX
    | TF_CLUIE_CURRENTPAGE;

struct CandidateUiData {
    state: ImeState,
    document_mgr: Option<ITfDocumentMgr>,
    updated_flags: u32,
    shown: bool,
}

#[implement(ITfCandidateListUIElement)]
struct CandidateUiElement {
    data: Rc<RefCell<CandidateUiData>>,
    _dll_guard: DllActivityGuard,
}

impl ITfUIElement_Impl for CandidateUiElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("KeyTao candidates"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        Ok(CANDIDATE_UI_GUID)
    }

    fn Show(&self, show: BOOL) -> Result<()> {
        self.data.borrow_mut().shown = show.as_bool();
        Ok(())
    }

    fn IsShown(&self) -> Result<BOOL> {
        Ok(BOOL::from(self.data.borrow().shown))
    }
}

impl ITfCandidateListUIElement_Impl for CandidateUiElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        Ok(std::mem::take(&mut self.data.borrow_mut().updated_flags))
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.data
            .borrow()
            .document_mgr
            .clone()
            .ok_or_else(|| E_FAIL.into())
    }

    fn GetCount(&self) -> Result<u32> {
        Ok(self.data.borrow().state.candidates.len() as u32)
    }

    fn GetSelection(&self) -> Result<u32> {
        let data = self.data.borrow();
        let count = data.state.candidates.len();
        if count == 0 {
            return Err(windows::core::Error::from(S_FALSE));
        }
        Ok(data
            .state
            .highlighted_candidate_index
            .min(count.saturating_sub(1)) as u32)
    }

    fn GetString(&self, index: u32) -> Result<BSTR> {
        let data = self.data.borrow();
        let candidate = data
            .state
            .candidates
            .get(index as usize)
            .ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        Ok(BSTR::from(candidate.text.as_str()))
    }

    fn GetPageIndex(
        &self,
        page_indices: *mut u32,
        capacity: u32,
        page_count: *mut u32,
    ) -> Result<()> {
        if page_count.is_null() {
            return Err(E_INVALIDARG.into());
        }

        let has_candidates = !self.data.borrow().state.candidates.is_empty();
        unsafe {
            *page_count = u32::from(has_candidates);
            if has_candidates && capacity > 0 {
                if page_indices.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                *page_indices = 0;
            }
        }
        Ok(())
    }

    fn SetPageIndex(&self, page_indices: *const u32, page_count: u32) -> Result<()> {
        if page_count == 0 {
            return Ok(());
        }
        if page_indices.is_null() || unsafe { *page_indices } != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(())
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(0)
    }
}

pub(crate) struct CandidateUiManager {
    data: Rc<RefCell<CandidateUiData>>,
    element: ITfCandidateListUIElement,
    ui_element_mgr: Option<ITfUIElementMgr>,
    ui_element_id: Option<u32>,
    host_allows_window: bool,
}

impl CandidateUiManager {
    pub(crate) fn new() -> Self {
        let data = Rc::new(RefCell::new(CandidateUiData {
            state: ImeState::empty(),
            document_mgr: None,
            updated_flags: ALL_CANDIDATE_FLAGS,
            shown: false,
        }));
        let element: ITfCandidateListUIElement = CandidateUiElement {
            data: Rc::clone(&data),
            _dll_guard: DllActivityGuard::new(),
        }
        .into();
        Self {
            data,
            element,
            ui_element_mgr: None,
            ui_element_id: None,
            host_allows_window: true,
        }
    }

    pub(crate) fn update(
        &mut self,
        thread_mgr: Option<&ITfThreadMgr>,
        document_mgr: Option<&ITfDocumentMgr>,
        state: &ImeState,
        allow_fallback_window: bool,
    ) -> bool {
        let has_content = !state.preedit.is_empty() || !state.candidates.is_empty();
        {
            let mut data = self.data.borrow_mut();
            data.state = state.clone();
            data.document_mgr = document_mgr.cloned();
            data.updated_flags = ALL_CANDIDATE_FLAGS;
            if self.ui_element_id.is_none() || !has_content {
                data.shown = has_content;
            }
        }

        if !has_content {
            self.end();
            return false;
        }

        if self.ui_element_id.is_none() {
            let Some(thread_mgr) = thread_mgr else {
                return allow_fallback_window;
            };
            let Ok(ui_element_mgr) = thread_mgr.cast::<ITfUIElementMgr>() else {
                return allow_fallback_window;
            };
            let Ok(element) = self.element.cast::<ITfUIElement>() else {
                return allow_fallback_window;
            };

            let mut show = BOOL::from(true);
            let mut element_id = u32::MAX;
            if unsafe { ui_element_mgr.BeginUIElement(&element, &mut show, &mut element_id) }
                .is_err()
            {
                return allow_fallback_window;
            }

            self.host_allows_window = show.as_bool();
            self.ui_element_id = Some(element_id);
            self.ui_element_mgr = Some(ui_element_mgr);
        }

        if !self.host_allows_window {
            if let (Some(manager), Some(element_id)) = (&self.ui_element_mgr, self.ui_element_id) {
                unsafe {
                    let _ = manager.UpdateUIElement(element_id);
                }
            }
        }

        self.host_allows_window && self.data.borrow().shown
    }

    pub(crate) fn end(&mut self) {
        if let (Some(manager), Some(element_id)) = (&self.ui_element_mgr, self.ui_element_id) {
            unsafe {
                let _ = manager.EndUIElement(element_id);
            }
        }
        self.ui_element_mgr = None;
        self.ui_element_id = None;
        self.host_allows_window = true;
        let mut data = self.data.borrow_mut();
        data.document_mgr = None;
        data.updated_flags = ALL_CANDIDATE_FLAGS;
        data.shown = false;
    }
}

impl Drop for CandidateUiManager {
    fn drop(&mut self) {
        self.end();
    }
}
