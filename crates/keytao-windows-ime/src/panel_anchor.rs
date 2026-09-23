//! A native, empty composition used only to obtain host caret geometry.
//!
//! Unlike an inline preedit this never owns any text that we may erase. The
//! host can terminate an empty composition at any point; that ends this lease,
//! not the Rime composition displayed in our candidate panel.

use std::{cell::Cell, rc::Rc};

use windows::{
    core::{implement, Interface, Result},
    Win32::{
        Foundation::{E_FAIL, E_UNEXPECTED},
        UI::TextServices::*,
    },
};

use crate::{edit_session::take_selection_range, globals::DllActivityGuard};

#[derive(Clone)]
pub(crate) struct PanelAnchor {
    context: ITfContext,
    composition: ITfComposition,
    terminated: Rc<Cell<bool>>,
    ending: Rc<Cell<bool>>,
}

impl PanelAnchor {
    pub(crate) fn context(&self) -> &ITfContext {
        &self.context
    }

    pub(crate) fn composition(&self) -> Option<&ITfComposition> {
        (!self.terminated.get() && !self.ending.get()).then_some(&self.composition)
    }

    /// End the native lease without clearing text or changing the selection.
    /// The caller must supply a write cookie for `context()`.
    pub(crate) fn end(&self, ec: u32) -> Result<()> {
        if self.terminated.get() {
            return Ok(());
        }
        if self.ending.replace(true) {
            return Err(E_UNEXPECTED.into());
        }
        struct EndingGuard<'a>(&'a Cell<bool>);
        impl Drop for EndingGuard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _ending = EndingGuard(&self.ending);
        let result = unsafe { self.composition.EndComposition(ec) };
        if result.is_ok() {
            self.terminated.set(true);
        }
        // A failed lock request is retryable. A synchronous termination
        // notification, however, remains authoritative even on an error path.
        if self.terminated.get() {
            Ok(())
        } else {
            result
        }
    }
}

/// Start at the selection's active end without changing the original range.
///
/// The selection may contain text. Clone+Collapse changes only our range, not
/// the host selection; committing later still replaces the original selection.
/// `S_OK` with a null composition is a normal host refusal, returned as `None`.
pub(crate) fn start_panel_anchor(ec: u32, context: &ITfContext) -> Result<Option<PanelAnchor>> {
    unsafe {
        let mut selections = [TF_SELECTION::default()];
        let mut count = 0;
        let selection_result =
            context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selections, &mut count);
        let active_end = selections[0].style.ase;
        let selection = take_selection_range(&mut selections[0]);
        selection_result?;
        if count != 1 {
            return Err(E_FAIL.into());
        }
        let range = selection
            .ok_or_else(|| windows::core::Error::from(E_FAIL))?
            .Clone()?;
        range.Collapse(
            ec,
            if active_end == TF_AE_START {
                TF_ANCHOR_START
            } else {
                TF_ANCHOR_END
            },
        )?;
        if !range.IsEmpty(ec)?.as_bool() {
            return Err(E_FAIL.into());
        }

        let terminated = Rc::new(Cell::new(false));
        let sink: ITfCompositionSink = PanelAnchorSink {
            terminated: Rc::clone(&terminated),
            _dll_guard: DllActivityGuard::new(),
        }
        .into();
        let owner: ITfContextComposition = context.cast()?;
        let mut raw_composition = std::ptr::null_mut();
        let result = (owner.vtable().StartComposition)(
            owner.as_raw(),
            ec,
            range.as_raw(),
            sink.as_raw(),
            &mut raw_composition,
        );
        let composition =
            (!raw_composition.is_null()).then(|| ITfComposition::from_raw(raw_composition));
        result.ok()?;
        let Some(composition) = composition else {
            return Ok(None);
        };
        // Some hosts end an empty composition before StartComposition returns.
        // Never publish that dead object back into the shared input state.
        if terminated.get() {
            return Ok(None);
        }
        Ok(Some(PanelAnchor {
            context: context.clone(),
            composition,
            terminated,
            ending: Rc::new(Cell::new(false)),
        }))
    }
}

#[implement(ITfCompositionSink)]
struct PanelAnchorSink {
    terminated: Rc<Cell<bool>>,
    _dll_guard: DllActivityGuard,
}

impl ITfCompositionSink_Impl for PanelAnchorSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _composition: Option<&ITfComposition>,
    ) -> Result<()> {
        // No TsfState borrow, Rime reset, document write, or window operation.
        // This callback is allowed to run synchronously inside any host call.
        self.terminated.set(true);
        Ok(())
    }
}

#[cfg(test)]
#[path = "panel_anchor_tests.rs"]
mod tests;
