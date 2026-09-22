//! Windows URL actions from the TSF language-bar menu.
//!
//! Keep requests until the webview is ready. Events are only wakeups, so a cold
//! start (or an event arriving before listen()) cannot lose a redeploy request.

use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

const ACTION_EVENT: &str = "windows-ime-action";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Open,
    Redeploy,
}

fn parse_action(argument: &str) -> Option<Action> {
    match argument {
        "keytao://ime/open" => Some(Action::Open),
        "keytao://ime/redeploy" => Some(Action::Redeploy),
        _ => None,
    }
}

#[derive(Default)]
struct Queue {
    next_id: u32,
    pending: Option<u32>,
    deploying: bool,
}

#[derive(Clone, Default)]
pub(crate) struct AppActions(Arc<Mutex<Queue>>);

impl AppActions {
    fn receive_args(&self, args: impl IntoIterator<Item = impl AsRef<str>>) {
        let redeploy = args
            .into_iter()
            .any(|argument| parse_action(argument.as_ref()) == Some(Action::Redeploy));
        if !redeploy {
            return;
        }
        let mut queue = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if queue.pending.is_none() && !queue.deploying {
            queue.next_id = queue.next_id.wrapping_add(1);
            queue.pending = Some(queue.next_id);
        }
    }

    fn pending(&self) -> Option<u32> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
    }

    fn dismiss(&self, id: u32) {
        let mut queue = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if queue.pending == Some(id) {
            queue.pending = None;
        }
    }

    pub(crate) fn begin_deploy(&self, action_id: Option<u32>) -> Result<DeployGuard, String> {
        let mut queue = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if queue.deploying {
            return Err("正在重新部署，请等待完成".into());
        }
        if action_id.is_some() && action_id != queue.pending {
            return Err("此重新部署请求已处理".into());
        }
        // A manual deployment also satisfies a queued menu request.
        queue.pending = None;
        queue.deploying = true;
        Ok(DeployGuard(self.clone()))
    }
}

pub(crate) struct DeployGuard(AppActions);

impl Drop for DeployGuard {
    fn drop(&mut self) {
        self.0
             .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .deploying = false;
    }
}

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Call before registering any other plugin: the plugin exits secondary
/// processes before they initialize an engine or another application's window.
pub(crate) fn configure(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    let actions = AppActions::default();
    actions.receive_args(std::env::args_os().filter_map(|argument| argument.into_string().ok()));
    builder
        .manage(actions.clone())
        .plugin(tauri_plugin_single_instance::init(
            move |app, args, _cwd| {
                actions.receive_args(args);
                show_main_window(app);
                let _ = app.emit(ACTION_EVENT, ());
            },
        ))
}

#[tauri::command]
pub(crate) fn windows_pending_ime_action(actions: tauri::State<AppActions>) -> Option<u32> {
    actions.pending()
}

/// Acknowledge a request whose UI displayed an error before deployment could
/// start (for example no installed schema). Never dismiss a newer request.
#[tauri::command]
pub(crate) fn windows_dismiss_ime_action(actions: tauri::State<AppActions>, id: u32) {
    actions.dismiss(id);
}

#[tauri::command]
pub(crate) async fn windows_redeploy_ime_action(
    app: AppHandle,
    id: u32,
) -> Result<super::DeployResult, String> {
    let _guard = app.state::<AppActions>().begin_deploy(Some(id))?;
    super::rime_deploy_default_inner(app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_menu_uris_are_accepted() {
        assert_eq!(parse_action("keytao://ime/open"), Some(Action::Open));
        assert_eq!(
            parse_action("keytao://ime/redeploy"),
            Some(Action::Redeploy)
        );
        for input in [
            "keytao://ime/redeploy/",
            "keytao://ime/redeploy?x=1",
            "keytao://ime/open#redeploy",
            "KEYTAO://ime/redeploy",
            "keytao://other/redeploy",
            "https://ime/redeploy",
            "--keytao://ime/redeploy",
            " keytao://ime/redeploy",
            "keytao://ime/redeploy ",
        ] {
            assert_eq!(parse_action(input), None, "{input}");
        }
    }

    #[test]
    fn cold_start_survives_until_claimed_and_duplicates_coalesce() {
        let actions = AppActions::default();
        actions.receive_args(["keytao-app.exe", "keytao://ime/redeploy"]);
        let id = actions.pending().unwrap();
        assert_eq!(actions.pending(), Some(id)); // non-destructive readiness check
        actions.receive_args(["keytao://ime/open", "keytao://ime/redeploy"]);
        assert_eq!(actions.pending(), Some(id));
        let guard = actions.begin_deploy(Some(id)).unwrap();
        assert_eq!(actions.pending(), None);
        actions.receive_args(["keytao://ime/redeploy"]);
        assert_eq!(actions.pending(), None);
        assert!(actions.begin_deploy(None).is_err());
        drop(guard);
        assert!(actions.begin_deploy(Some(id)).is_err()); // no replay after finish
        actions.receive_args(["keytao://ime/redeploy"]);
        assert_ne!(actions.pending(), Some(id));
    }

    #[test]
    fn manual_deploy_satisfies_pending_request_and_allows_retry_after_failure() {
        let actions = AppActions::default();
        actions.receive_args(["keytao://ime/redeploy"]);
        let id = actions.pending().unwrap();
        let guard = actions.begin_deploy(None).unwrap();
        assert_eq!(actions.pending(), None);
        assert!(actions.begin_deploy(Some(id)).is_err());
        drop(guard); // the same RAII path is used when a deployment returns Err
        assert!(actions.begin_deploy(None).is_ok());
    }

    #[test]
    fn stale_ack_cannot_drop_new_request_and_open_never_deploys() {
        let actions = AppActions::default();
        actions.receive_args(["keytao://ime/open", "keytao://ime/redeploy?ignored"]);
        assert_eq!(actions.pending(), None);
        actions.receive_args(["keytao://ime/redeploy"]);
        let id = actions.pending().unwrap();
        actions.dismiss(id);
        actions.receive_args(["keytao://ime/redeploy"]);
        let next = actions.pending().unwrap();
        actions.dismiss(id);
        assert_eq!(actions.pending(), Some(next));
    }

    #[test]
    fn simultaneous_callbacks_and_claims_share_one_deployment() {
        let actions = AppActions::default();
        std::thread::scope(|scope| {
            for _ in 0..12 {
                let actions = actions.clone();
                scope.spawn(move || actions.receive_args(["keytao://ime/redeploy"]));
            }
        });
        let id = actions.pending().unwrap();
        let barrier = std::sync::Barrier::new(12);
        let winners = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| {
                    barrier.wait();
                    if let Ok(_guard) = actions.begin_deploy(Some(id)) {
                        winners.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(winners.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
