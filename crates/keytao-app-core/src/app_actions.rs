//! Pending Windows menu requests survive until the webview pulls them.
use crate::{Core, CoreError, deploy::DeployResult, ime_host::ImeHost};
use std::sync::{Arc, Mutex};

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
pub struct AppActions(Arc<Mutex<Queue>>);

impl AppActions {
    pub fn receive_args(&self, args: impl IntoIterator<Item = impl AsRef<str>>) {
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

    pub fn pending(&self) -> Option<u32> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
    }

    pub fn dismiss(&self, id: u32) {
        let mut queue = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if queue.pending == Some(id) {
            queue.pending = None;
        }
    }

    pub fn begin_deploy(&self, action_id: Option<u32>) -> Result<DeployGuard, String> {
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

pub struct DeployGuard(AppActions);

impl Drop for DeployGuard {
    fn drop(&mut self) {
        self.0
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .deploying = false;
    }
}

pub async fn windows_redeploy_ime_action(
    core: &Core,
    host: impl ImeHost,
    actions: &AppActions,
    id: u32,
) -> Result<DeployResult, CoreError> {
    let _guard = actions.begin_deploy(Some(id)).map_err(CoreError::Other)?;
    crate::deploy::rime_deploy_default_inner(core, host)
        .await
        .map_err(CoreError::Other)
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
