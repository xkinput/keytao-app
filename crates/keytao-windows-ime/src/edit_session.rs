//! `ITfEditSession` plumbing shared by the key path and the context probes.
//!
//! TSF only hands out an edit cookie inside a session, so every document read
//! or write goes through one of the helpers below.

use std::cell::RefCell;

use windows::{
    core::{implement, Result},
    Win32::{
        Foundation::{E_FAIL, E_UNEXPECTED},
        UI::TextServices::{
            ITfContext, ITfEditSession, ITfEditSession_Impl, ITfRange,
            TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_ASYNC, TF_ES_ASYNCDONTCARE, TF_ES_READ,
            TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
        },
    },
};

use crate::{globals::DllActivityGuard, guard};

type EditFn = Box<dyn FnOnce(u32, &ITfContext) -> Result<()>>;

#[implement(ITfEditSession)]
struct EditSession {
    context: ITfContext,
    f: RefCell<Option<EditFn>>,
    _dll_guard: DllActivityGuard,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guard(|| {
            let mut slot = self
                .f
                .try_borrow_mut()
                .map_err(|_| windows::core::Error::from(E_UNEXPECTED))?;
            match slot.take() {
                Some(f) => f(ec, &self.context).map_err(|error| {
                    if error.code().is_ok() {
                        E_FAIL.into()
                    } else {
                        error
                    }
                }),
                None => Ok(()),
            }
        })
    }
}

fn request_session(
    context: &ITfContext,
    client_id: u32,
    flags: u32,
    f: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static,
) -> Result<()> {
    let session = EditSession {
        context: context.clone(),
        f: RefCell::new(Some(Box::new(f))),
        _dll_guard: DllActivityGuard::new(),
    };
    let iface: ITfEditSession = session.into();
    unsafe {
        let hr_session =
            context.RequestEditSession(client_id, &iface, TF_CONTEXT_EDIT_CONTEXT_FLAGS(flags))?;
        hr_session.ok()
    }
}

pub(crate) fn with_write_session(
    context: &ITfContext,
    client_id: u32,
    f: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static,
) -> Result<()> {
    request_session(context, client_id, TF_ES_SYNC.0 | TF_ES_READWRITE.0, f)
}

/// Queued write session for cleanup that may wait for the document to unlock.
/// Never use it when the text service is about to be released — TSF drops the
/// client id and the queued session with it.
pub(crate) fn with_async_write_session(
    context: &ITfContext,
    client_id: u32,
    f: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static,
) -> Result<()> {
    request_session(context, client_id, TF_ES_ASYNC.0 | TF_ES_READWRITE.0, f)
}

/// Queued write session for mouse input arriving from the candidate window.
/// The host may reject `TF_ES_SYNC` outside a TSF key callback.
pub(crate) fn with_async_dontcare_write_session(
    context: &ITfContext,
    client_id: u32,
    f: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static,
) -> Result<()> {
    request_session(
        context,
        client_id,
        TF_ES_ASYNCDONTCARE.0 | TF_ES_READWRITE.0,
        f,
    )
}

/// Read-only counterpart. Hosts may refuse a synchronous session while the
/// document is locked, so callers must treat an error as "unknown", never as a
/// negative answer.
pub(crate) fn with_read_session(
    context: &ITfContext,
    client_id: u32,
    f: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static,
) -> Result<()> {
    request_session(context, client_id, TF_ES_SYNC.0 | TF_ES_READ.0, f)
}

/// Take ownership of the range `ITfContext::GetSelection` filled in.
///
/// The field is a `ManuallyDrop`, so leaving it in place leaks the interface
/// reference TSF handed out.
pub(crate) fn take_selection_range(selection: &mut TF_SELECTION) -> Option<ITfRange> {
    core::mem::ManuallyDrop::into_inner(core::mem::replace(
        &mut selection.range,
        core::mem::ManuallyDrop::new(None),
    ))
}
