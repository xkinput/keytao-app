//! Launch the settings app outside the host application's input thread.

use windows::{
    core::{w, Error, Result, HSTRING, PCWSTR},
    Win32::{
        Foundation::E_FAIL,
        System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        },
        UI::{
            Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

use crate::{globals::DllActivityGuard, guard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AppAction {
    OpenApp,
    Redeploy,
}

impl AppAction {
    fn uri(self) -> &'static str {
        match self {
            Self::OpenApp => "keytao://ime/open",
            Self::Redeploy => "keytao://ime/redeploy",
        }
    }
}

pub(crate) fn launch_action(action: AppAction) -> Result<()> {
    spawn_action(action, open_action_uri).map(|_| ())
}

fn spawn_action(
    action: AppAction,
    execute: impl FnOnce(AppAction) -> Result<()> + Send + 'static,
) -> Result<std::thread::JoinHandle<()>> {
    // Acquire before spawning: COM can release the menu object as soon as its
    // callback returns. Keep the TIP alive until all worker code has finished.
    let dll_guard = DllActivityGuard::new();
    std::thread::Builder::new()
        .name("keytao-app-action".into())
        .spawn(move || {
            let _dll_guard = dll_guard;
            if let Err(error) = guard(|| execute(action)) {
                keytao_core::rt_log!(
                    keytao_core::runtime_log::Level::Info,
                    "error",
                    "app_action_launch_failed",
                    action = action.uri(),
                    hresult = format!("0x{:08X}", error.code().0 as u32),
                    msg = error.to_string()
                );
            }
        })
        .map_err(|error| {
            Error::new(
                E_FAIL,
                format!("Unable to queue KeyTao app action: {error}"),
            )
        })
}

fn open_action_uri(action: AppAction) -> Result<()> {
    // Shell handlers can use COM. This fresh worker owns its apartment and
    // never inherits the host's TSF context, engine or foreground editor.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;
    }
    struct ComApartment;
    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _apartment = ComApartment;
    let uri = HSTRING::from(action.uri());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: PCWSTR(uri.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // The installer registers a quoted executable/URI command. Only fixed
    // actions cross this boundary; no text from the editor becomes arguments.
    unsafe { ShellExecuteExW(&mut info) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn menu_action_returns_while_launcher_waits_on_its_own_thread() {
        let caller = std::thread::current().id();
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let worker = spawn_action(AppAction::Redeploy, move |action| {
            started_tx
                .send((std::thread::current().id(), action))
                .unwrap();
            finish_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
        let (worker_id, action) = started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(worker_id, caller);
        assert_eq!(action, AppAction::Redeploy);
        assert!(!worker.is_finished());
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
    }
}
