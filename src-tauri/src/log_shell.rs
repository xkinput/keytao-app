#[cfg(target_os = "android")]
use super::ScopedStorageHandle;
#[cfg(target_os = "android")]
use tauri::Manager;
#[cfg(target_os = "ios")]
use std::path::PathBuf;
use keytao_app_core::logs::RuntimeLogShare;

pub(super) async fn share<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    request: RuntimeLogShare,
) -> Result<serde_json::Value, String> {
    match request {
        #[cfg(target_os = "android")]
        RuntimeLogShare::Android => app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("shareRuntimeLog", ())
            .map_err(|e| e.to_string()),
        #[cfg(target_os = "ios")]
        RuntimeLogShare::Ios(paths) => {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            app.run_on_main_thread(move || {
                let _ = sender.send(present_runtime_log_share(paths));
            })
            .map_err(|e| e.to_string())?;
            receiver
                .await
                .map_err(|e| e.to_string())?
                .map(|()| serde_json::Value::Null)
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        RuntimeLogShare::Directory(dir) => {
            use tauri_plugin_opener::OpenerExt;
            app.opener()
                .open_path(dir, None::<&str>)
                .map(|()| serde_json::Value::Null)
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(target_os = "ios")]
#[allow(deprecated)] // Foundation re-exports the UIKit-compatible main-thread token.
fn present_runtime_log_share(paths: Vec<PathBuf>) -> Result<(), String> {
    use objc2_foundation::{MainThreadMarker, NSArray, NSURL};
    use objc2_ui_kit::{UIActivityViewController, UIApplication, UIWindowScene};

    let mtm = MainThreadMarker::new().ok_or("Sharing must run on the main thread")?;
    let application = UIApplication::sharedApplication(mtm);
    let window = application
        .connectedScenes()
        .iter()
        .filter_map(|scene| scene.downcast::<UIWindowScene>().ok())
        .flat_map(|scene| scene.windows().to_vec())
        .find(|window| window.isKeyWindow())
        .or_else(|| application.keyWindow())
        .ok_or("No active window is available for sharing")?;
    let presenter = window
        .rootViewController()
        .ok_or("No root view controller is available")?;
    if presenter.presentedViewController().is_some() {
        return Err("Dismiss the current sheet before sharing logs".into());
    }
    let urls = paths
        .iter()
        .map(|path| NSURL::from_file_path(path).ok_or("Cannot create runtime log file URL"))
        .collect::<Result<Vec<_>, _>>()?;
    let items = NSArray::from_retained_slice(&urls);
    // UIKit accepts file NSURL items; erasing the array's generic type is safe.
    let controller = unsafe {
        UIActivityViewController::initWithActivityItems_applicationActivities(
            mtm.alloc(),
            items.cast_unchecked(),
            None,
        )
    };
    if let Some(popover) = controller.popoverPresentationController() {
        let view = presenter
            .view()
            .ok_or("No source view is available for sharing")?;
        popover.setSourceView(Some(&view));
        popover.setSourceRect(view.bounds());
    }
    presenter.presentViewController_animated_completion(&controller, true, None);
    Ok(())
}
