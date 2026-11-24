//! IBus D-Bus backend for keytao-ime.
//!
//! Implements enough of the IBus D-Bus protocol so that Chromium/CEF apps
//! (e.g. WeChatAppEx) can use keytao as their IME without requiring a real
//! IBus daemon.

use crate::engine::{client_preedit, panel_has_composition, ControlAltTap, CoreEngine, ImeSession};
use crate::ibus_shared::{
    self, candidate_display_text, ibus_engine_desc, ibus_lookup_table_value,
    ibus_preedit_text_variant, ibus_text_variant, state_to_panel_model, ContentTypeState,
    KeyOutcome, ModeIndicator,
};
use crate::kimpanel::{mode_property as kimpanel_mode_property, Kimpanel};
use crate::panel::{spawn_x11_overlay_panel, OverlayPanelMessage};
use keytao_core::{runtime_log, ImeState};
use keytao_theme::ThemeResolver;
use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::{
        atomic::{AtomicI32, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use zbus::{
    connection,
    export::futures_util::StreamExt,
    fdo::{DBusProxy, NameOwnerChangedStream},
    interface,
    names::BusName,
    object_server::SignalContext,
    zvariant::{self, ObjectPath, OwnedObjectPath},
    ObjectServer,
};

type ContextOwners = Arc<Mutex<HashMap<String, HashSet<OwnedObjectPath>>>>;
type ActiveContext = Arc<tokio::sync::Mutex<Option<FocusedContext>>>;

struct FocusedContext {
    id: u64,
    deployment: Weak<ibus_shared::IbusDeploymentHint>,
}

// ── InputContext D-Bus object ─────────────────────────────────────────────────

struct InputContext {
    id: u64,
    active: ActiveContext,
    closed: bool,
    session: ImeSession,
    control_alt_tap: ControlAltTap,
    deployment: Arc<ibus_shared::IbusDeploymentHint>,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    cursor_x: Arc<AtomicI32>,
    cursor_y: Arc<AtomicI32>,
    mode: ModeIndicator,
    x11_panel_tx: std::sync::mpsc::Sender<OverlayPanelMessage>,
    theme_resolver: Arc<ThemeResolver>,
    content_type: ContentTypeState,
    /// Signal context for the paths the caller does not give one, such as the
    /// `ContentType` property setter.
    signal_ctxt: Option<SignalContext<'static>>,
}

impl InputContext {
    fn is_active(&self, active: &Option<FocusedContext>) -> bool {
        active.as_ref().is_some_and(|context| context.id == self.id)
    }

    async fn activate(&self, active: &mut Option<FocusedContext>) {
        if self.is_active(active) {
            return;
        }
        if let Some(previous) = active.take() {
            if let Some(deployment) = previous.deployment.upgrade() {
                deployment.discard().await;
            }
            self.hide_kimpanel_ui().await;
        }
        *active = Some(FocusedContext {
            id: self.id,
            deployment: Arc::downgrade(&self.deployment),
        });
    }

    async fn clear_ui(&self, ctxt: &SignalContext<'_>, is_active: bool) {
        let _ = Self::hide_preedit_text(ctxt).await;
        let _ = Self::hide_lookup_table(ctxt).await;
        if is_active {
            self.hide_shared_ui().await;
            self.deployment
                .apply_state(&ImeState::empty(), false, false)
                .await;
        }
    }

    async fn hide_shared_ui(&self) {
        self.hide_kimpanel_ui().await;
        let _ = self.x11_panel_tx.send(OverlayPanelMessage::Hide);
    }

    async fn hide_kimpanel_ui(&self) {
        if let Some(kc) = &self.kimpanel_ctxt {
            let _ = Kimpanel::show_preedit(kc, false).await;
            let _ = Kimpanel::show_lookup_table(kc, false).await;
        }
    }

    async fn reset_ui(&self, ctxt: Option<&SignalContext<'_>>, deactivate: bool) {
        let mut active = self.active.lock().await;
        if let Some(ctxt) = ctxt {
            let _ = Self::hide_preedit_text(ctxt).await;
            let _ = Self::hide_lookup_table(ctxt).await;
        }
        if self.is_active(&active) {
            self.deployment.reset().await;
            self.hide_shared_ui().await;
            if deactivate {
                *active = None;
            }
        } else {
            self.deployment.discard().await;
        }
    }

    fn show_mode_hint(&self, ascii_mode: bool) {
        let cx = self.cursor_x.load(Ordering::Relaxed);
        let cy = self.cursor_y.load(Ordering::Relaxed);
        let _ = self.x11_panel_tx.send(OverlayPanelMessage::ModeHint {
            ascii_mode,
            x: cx,
            y: cy + 24,
        });
    }

    /// Publish the conversion mode to the desktop indicator.  Without this the
    /// Kimpanel applet keeps showing whatever mode was registered at startup.
    async fn publish_mode_property(&self, ascii_mode: bool) {
        if let Some(kctxt) = &self.kimpanel_ctxt {
            let _ = Kimpanel::update_property(kctxt, &kimpanel_mode_property(ascii_mode)).await;
        }
    }

    /// Show the mode Rime is actually in, not the one this context last saw.
    async fn publish_current_mode(&self) {
        let ascii_mode = self.session.state().is_english_mode();
        self.mode.adopt(ascii_mode);
        self.publish_mode_property(ascii_mode).await;
    }

    async fn apply_mode_change(&self, ascii_mode: bool, show_hint: bool) {
        let Some(change) = self.mode.update(ascii_mode, show_hint) else {
            return;
        };
        self.publish_mode_property(change.ascii_mode).await;
        if change.hint {
            self.show_mode_hint(change.ascii_mode);
        }
    }

    /// Apply an IBus content-type: password and PIN fields stop composing so
    /// that no secret ever reaches librime or the user dictionary.
    async fn apply_content_type(&self, purpose: u32, hints: u32) {
        let _activity = crate::hot_update::activity();
        let change = self.content_type.set(purpose, hints);
        if change.turned_sensitive {
            self.control_alt_tap.clear();
        }
        self.session.set_input_policy(change.policy);
        if change.turned_sensitive {
            tracing::debug!("IBus InputContext: sensitive content type, passing keys through");
            self.reset_ui(self.signal_ctxt.as_ref(), false).await;
        }
    }

    async fn apply_ime_state(
        &self,
        ime_state: ImeState,
        ctxt: &SignalContext<'_>,
        active: &Option<FocusedContext>,
    ) {
        if self.closed {
            return;
        }
        let is_active = self.is_active(active);
        let kimpanel_ctxt = self.kimpanel_ctxt.as_ref().filter(|_| is_active);
        let ascii_mode = ime_state.is_english_mode();
        let theme = self.theme_resolver.current();
        let hide_candidate_panel =
            theme.ui.hide_candidate_panel && !ime_state.is_package_switcher();
        let has_content = panel_has_composition(&ime_state, &theme.ui);
        if let Some(ref text) = ime_state.committed {
            if !text.is_empty() {
                tracing::debug!("IBus CommitText: {} chars", text.chars().count());
                tracing::trace!("IBus CommitText: {text:?}");
                clear_preedit(ctxt, kimpanel_ctxt).await;
                let _ = Self::commit_text(ctxt, ibus_text_variant(text)).await;
            }
        }

        let (preedit, cursor) =
            client_preedit(&ime_state, &theme.ui, self.deployment.has_overlay());
        if preedit.is_empty() {
            let _ = Self::hide_preedit_text(ctxt).await;
        } else {
            clear_preedit(ctxt, None).await;

            let _ = Self::update_preedit_text(
                ctxt,
                ibus_preedit_text_variant(preedit, hide_candidate_panel, &self.theme_resolver),
                cursor as u32,
                true,
            )
            .await;
        }

        if ime_state.candidates.is_empty() || hide_candidate_panel {
            let _ = Self::hide_lookup_table(ctxt).await;
            if let Some(kctxt) = kimpanel_ctxt {
                let _ = Kimpanel::show_lookup_table(kctxt, false).await;
            }
        } else {
            let model = state_to_panel_model(&ime_state, &self.theme_resolver);
            let _ = Self::update_lookup_table(ctxt, ibus_lookup_table_value(&model), true).await;
            if let Some(kctxt) = kimpanel_ctxt {
                let labels: Vec<String> = model
                    .candidates
                    .iter()
                    .map(|candidate| candidate.label.clone())
                    .collect();
                let cands: Vec<String> = model
                    .candidates
                    .iter()
                    .map(candidate_display_text)
                    .collect();
                let labels_ref: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                let cands_ref: Vec<&str> = cands.iter().map(|s| s.as_str()).collect();
                let attrs: Vec<&str> = vec![];
                let _ = Kimpanel::update_lookup_table(
                    kctxt,
                    &labels_ref,
                    &cands_ref,
                    &attrs,
                    model.navigation.can_go_previous,
                    model.navigation.can_go_next,
                )
                .await;
                let _ = Kimpanel::show_lookup_table(kctxt, true).await;
                let _ = Kimpanel::update_spot_location(
                    kctxt,
                    self.cursor_x.load(Ordering::Relaxed),
                    self.cursor_y.load(Ordering::Relaxed),
                )
                .await;
            }
        }

        if !is_active {
            return;
        }
        let message = if has_content {
            OverlayPanelMessage::Show {
                state: ime_state.clone(),
                x: self.cursor_x.load(Ordering::Relaxed),
                y: self.cursor_y.load(Ordering::Relaxed) + 24,
            }
        } else {
            OverlayPanelMessage::Hide
        };
        let _ = self.x11_panel_tx.send(message);

        self.apply_mode_change(ascii_mode, !has_content).await;
        self.deployment
            .apply_state(&ime_state, has_content, false)
            .await;
    }

    async fn select_candidate_at(&self, index: usize, ctxt: &SignalContext<'_>) {
        let _activity = crate::hot_update::activity();
        if let Some(ime_state) = ibus_shared::select_candidate(&self.session, index) {
            let active = self.active.lock().await;
            self.apply_ime_state(ime_state, ctxt, &active).await;
        }
    }

    async fn change_page(&self, backward: bool, ctxt: &SignalContext<'_>) {
        let _activity = crate::hot_update::activity();
        if let Some(ime_state) = ibus_shared::change_page(&self.session, backward) {
            let active = self.active.lock().await;
            self.apply_ime_state(ime_state, ctxt, &active).await;
        }
    }

    async fn process_navigation_key(&self, keyval: u32, ctxt: &SignalContext<'_>) {
        let _activity = crate::hot_update::activity();
        if let Some(ime_state) = ibus_shared::navigation_key(&self.session, keyval) {
            let active = self.active.lock().await;
            self.apply_ime_state(ime_state, ctxt, &active).await;
        }
    }
}

// zbus 4.4's macro inverts the explicit `spawn` flag: true generates
// spawn_tasks_for_methods() == false. Keep key/focus/reset calls serialized
// across signal publication. ibus_shared tests guard this dependency behavior.
#[interface(name = "org.freedesktop.IBus.InputContext", spawn = true)]
impl InputContext {
    async fn focus_in(&self) {
        if self.closed {
            return;
        }
        let mut active = self.active.lock().await;
        self.activate(&mut active).await;
        self.control_alt_tap.clear();
        tracing::debug!("IBus InputContext: FocusIn");
        self.deployment.reset().await;
        self.publish_current_mode().await;
    }

    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.control_alt_tap.clear();
        let _activity = crate::hot_update::activity();
        tracing::debug!("IBus InputContext: FocusOut");
        self.session.clear_composition();
        self.reset_ui(Some(&ctxt), true).await;
        runtime_log::drain_key_stats();
    }

    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        if self.closed {
            return;
        }
        self.control_alt_tap.clear();
        let _activity = crate::hot_update::activity();
        tracing::debug!("IBus InputContext: Reset");
        self.session.clear_composition();
        self.reset_ui(Some(&ctxt), false).await;
    }

    async fn set_cursor_location(&self, x: i32, y: i32, _w: i32, _h: i32) {
        self.cursor_x.store(x, Ordering::Relaxed);
        self.cursor_y.store(y, Ordering::Relaxed);
        if let Some(kctxt) = &self.kimpanel_ctxt {
            let _ = Kimpanel::update_spot_location(kctxt, x, y).await;
        }
    }
    async fn set_cursor_location_relative(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    async fn set_capabilities(&self, _caps: u32) {}
    async fn set_surrounding_text(&self, _text: zvariant::Value<'_>, _cursor: u32, _anchor: u32) {}

    /// GTK and Chromium set the content type through `Properties.Set`, so this
    /// has to be a property; the method below stays for older callers.
    #[zbus(property(emits_changed_signal = "false"))]
    fn content_type(&self) -> (u32, u32) {
        self.content_type.get()
    }

    #[zbus(property)]
    async fn set_content_type(&mut self, value: (u32, u32)) {
        let (purpose, hints) = value;
        self.apply_content_type(purpose, hints).await;
    }

    #[zbus(name = "SetContentType")]
    async fn set_content_type_method(&self, purpose: u32, hints: u32) {
        self.apply_content_type(purpose, hints).await;
    }

    async fn enable(&self) {
        if self.closed {
            return;
        }
        tracing::debug!("IBus InputContext: Enable");
        self.deployment.reset().await;
        self.publish_current_mode().await;
    }

    async fn disable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.control_alt_tap.clear();
        let _activity = crate::hot_update::activity();
        tracing::debug!("IBus InputContext: Disable");
        self.session.clear_composition();
        self.reset_ui(Some(&ctxt), true).await;
        runtime_log::drain_key_stats();
    }

    async fn page_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.change_page(true, &ctxt).await;
    }

    async fn page_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.change_page(false, &ctxt).await;
    }

    async fn cursor_up(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.process_navigation_key(0xff52, &ctxt).await;
    }

    async fn cursor_down(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.process_navigation_key(0xff54, &ctxt).await;
    }

    async fn candidate_clicked(
        &self,
        index: u32,
        _button: u32,
        _state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) {
        self.select_candidate_at(index as usize, &ctxt).await;
    }

    async fn property_activate(&self, _name: &str, _state: u32) {}
    async fn property_show(&self, _name: &str) {}
    async fn property_hide(&self, _name: &str) {}

    /// Process a key event. Returns true if consumed by the IME.
    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> crate::hot_update::KeyReply {
        let activity = crate::hot_update::activity();
        if self.closed || !crate::reload_bus::engine_enabled() {
            return crate::hot_update::KeyReply::new(false, activity);
        }
        let started = Instant::now();
        let mut active = self.active.lock().await;
        if active.is_none() {
            self.activate(&mut active).await;
        }
        let is_active = self.is_active(&active);
        if is_active
            && state & keytao_core::RIME_RELEASE_MASK == 0
            && self.session.input_policy().composing
        {
            self.deployment.key_pressed().await;
        }
        // The decision is shared with the GNOME engine backend; this object only
        // publishes the outcome over org.freedesktop.IBus.InputContext.
        let outcome = ibus_shared::process_key_event(
            &self.session,
            &self.control_alt_tap,
            keyval,
            state,
            &self.theme_resolver,
        );
        let accepted = outcome.accepted();
        match outcome {
            KeyOutcome::Forward => {}
            KeyOutcome::ClearUi => {
                self.clear_ui(&ctxt, is_active).await;
            }
            KeyOutcome::Publish { state, .. } => {
                self.apply_ime_state(*state, &ctxt, &active).await;
                runtime_log::record_keystroke(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        crate::hot_update::KeyReply::new(accepted, activity)
    }

    // ── Signals ──────────────────────────────────────────────────────────────

    #[zbus(signal)]
    async fn commit_text(ctxt: &SignalContext<'_>, text: zvariant::Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: zvariant::Value<'_>,
        cursor_pos: u32,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: zvariant::Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

/// `org.freedesktop.IBus.Service` for an input context object.
///
/// GTK and Chromium release their context through `IBusProxy`, which calls
/// `org.freedesktop.IBus.Service.Destroy`; `org.freedesktop.IBus.InputContext`
/// has no Destroy of its own, so without this interface every context the
/// client drops leaks a D-Bus object and a librime session.
struct InputContextService {
    owner: String,
    contexts: ContextOwners,
}

// Use the same serialized dispatch as InputContext, including Destroy.
#[interface(name = "org.freedesktop.IBus.Service", spawn = true)]
impl InputContextService {
    async fn destroy(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        tracing::info!("IBus InputContext: Destroy {}", ctxt.path());
        let path = OwnedObjectPath::from(ctxt.path().clone());
        if untrack_context(&self.contexts, &self.owner, &path) {
            remove_input_context(server, &path)
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        }
        Ok(())
    }
}

fn untrack_context(contexts: &ContextOwners, owner: &str, path: &OwnedObjectPath) -> bool {
    let mut contexts = contexts.lock().unwrap();
    let Some(paths) = contexts.get_mut(owner) else {
        return false;
    };
    let removed = paths.remove(path);
    if paths.is_empty() {
        contexts.remove(owner);
    }
    removed
}

async fn remove_input_context(server: &ObjectServer, path: &ObjectPath<'_>) -> zbus::Result<()> {
    let _activity = crate::hot_update::activity();
    let context = server.interface::<_, InputContext>(path).await;
    // Detach both interfaces before locking the context to drain any in-flight key handling.
    let input = server.remove::<InputContext, _>(path).await;
    let service = server.remove::<InputContextService, _>(path).await;
    if let Ok(context) = context {
        let mut input = context.get_mut().await;
        input.closed = true;
        runtime_log::drain_key_stats();
        input.session.clear_composition();
        input.reset_ui(None, true).await;
    }
    input?;
    service?;
    Ok(())
}

async fn watch_context_owners(
    mut changes: NameOwnerChangedStream<'_>,
    server: &ObjectServer,
    contexts: &ContextOwners,
) {
    while let Some(change) = changes.next().await {
        let Ok(args) = change.args() else { continue };
        if let (BusName::Unique(owner), None) = (args.name(), args.new_owner().as_ref()) {
            let paths = contexts.lock().unwrap().remove(owner.as_str());
            for path in paths.into_iter().flatten() {
                if let Err(e) = remove_input_context(server, &path).await {
                    tracing::warn!("IBus: could not remove disconnected context {path}: {e}");
                }
            }
        }
    }
}

/// Send an empty UpdatePreeditText to tell the client the composition ended
/// before committing. This is the sequence Chromium/CEF requires so that it
/// can correctly place the committed text without conflating it with the
/// still-active preedit region.
async fn clear_preedit(ctxt: &SignalContext<'_>, kctxt: Option<&SignalContext<'_>>) {
    let _ = InputContext::update_preedit_text(ctxt, ibus_text_variant(""), 0, false).await;
    if let Some(kc) = kctxt {
        let _ = Kimpanel::update_preedit_text(kc, "", "").await;
        let _ = Kimpanel::show_preedit(kc, false).await;
    }
}

// ── IBusBus D-Bus object ──────────────────────────────────────────────────────

struct IBusBus {
    engine: CoreEngine,
    ctx_counter: Arc<AtomicU64>,
    contexts: ContextOwners,
    active: ActiveContext,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    x11_panel_tx: std::sync::mpsc::Sender<OverlayPanelMessage>,
    x11_panel_available: bool,
    theme_resolver: Arc<ThemeResolver>,
}

#[interface(name = "org.freedesktop.IBus")]
impl IBusBus {
    /// CreateInputContext(client_name) → object_path
    async fn create_input_context(
        &self,
        client_name: &str,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing input context owner".into()))?;
        let owner = sender.as_str().to_owned();
        let n = self.ctx_counter.fetch_add(1, Ordering::SeqCst);
        let path_str = format!("/org/freedesktop/IBus/InputContext_{n}");
        let path = OwnedObjectPath::try_from(path_str.clone())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        tracing::info!("IBus CreateInputContext client={client_name:?} -> {path_str}");

        let session = crate::engine::create_session(&self.engine)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let cursor_x = Arc::new(AtomicI32::new(0));
        let cursor_y = Arc::new(AtomicI32::new(0));
        let signal_ctxt = SignalContext::new(connection, path_str.clone()).ok();
        let deployment = ibus_shared::IbusDeploymentHint::new(
            signal_ctxt.clone(),
            self.kimpanel_ctxt.clone(),
            "org.freedesktop.IBus.InputContext",
            self.x11_panel_available.then(|| self.x11_panel_tx.clone()),
            cursor_x.clone(),
            cursor_y.clone(),
        );
        let ctx = InputContext {
            id: n,
            active: self.active.clone(),
            closed: false,
            session,
            control_alt_tap: ControlAltTap::default(),
            deployment: deployment.clone(),
            kimpanel_ctxt: self.kimpanel_ctxt.clone(),
            cursor_x,
            cursor_y,
            mode: ModeIndicator::new(),
            x11_panel_tx: self.x11_panel_tx.clone(),
            theme_resolver: self.theme_resolver.clone(),
            content_type: ContentTypeState::new(),
            signal_ctxt,
        };
        server
            .at(path.clone(), ctx)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        // Clients drop the context through org.freedesktop.IBus.Service, so that
        // interface has to live at the same path as the context.
        if let Err(e) = server
            .at(
                path.clone(),
                InputContextService {
                    owner: owner.clone(),
                    contexts: self.contexts.clone(),
                },
            )
            .await
        {
            let _ = server.remove::<InputContext, _>(&path).await;
            return Err(zbus::fdo::Error::Failed(e.to_string()));
        }
        deployment.watch_reload(|ctxt| async move {
            let server = ctxt.connection().object_server();
            if let Ok(context) = server.interface::<_, InputContext>(ctxt.path()).await {
                // Serialize reset with key replies that may still be publishing.
                context.get_mut().await.reset(ctxt.clone()).await;
            }
        });

        self.contexts
            .lock()
            .unwrap()
            .entry(owner.clone())
            .or_default()
            .insert(path.clone());
        // A disconnect can arrive while the two interfaces are being registered.
        let still_connected = async {
            DBusProxy::new(connection)
                .await?
                .name_has_owner(BusName::Unique(sender.clone()))
                .await
        }
        .await;
        match still_connected {
            Ok(true) => Ok(path),
            result => {
                if untrack_context(&self.contexts, &owner, &path) {
                    if let Err(e) = remove_input_context(server, &path).await {
                        tracing::warn!("IBus: could not remove unclaimed context {path}: {e}");
                    }
                }
                Err(result
                    .err()
                    .unwrap_or_else(|| zbus::fdo::Error::NameHasNoOwner(owner)))
            }
        }
    }

    async fn is_global_engine(&self) -> bool {
        true
    }

    async fn get_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![ibus_engine_desc()]
    }

    async fn list_active_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![ibus_engine_desc()]
    }

    async fn get_global_engine(&self) -> zbus::fdo::Result<zvariant::OwnedValue> {
        Ok(ibus_engine_desc())
    }

    async fn set_global_engine(&self, name: &str) -> zbus::fdo::Result<()> {
        tracing::info!("IBus SetGlobalEngine: {name}");
        Ok(())
    }

    async fn register_component(&self, _component: zvariant::Value<'_>) -> zbus::fdo::Result<()> {
        tracing::info!("IBus RegisterComponent");
        Ok(())
    }

    async fn exit(&self, restart: bool) {
        tracing::info!("IBus Exit requested restart={restart}");
    }

    #[zbus(signal)]
    async fn global_engine_changed(ctxt: &SignalContext<'_>, name: &str) -> zbus::Result<()>;
}

// ── IBus address file management ──────────────────────────────────────────────

fn write_ibus_address_files(dbus_address: &str) {
    let machine_id = read_machine_id();
    let pid = std::process::id();

    let bus_dir = match dirs::config_dir() {
        Some(d) => d.join("ibus").join("bus"),
        None => {
            tracing::warn!("cannot determine config dir; skipping IBus address files");
            return;
        }
    };
    if let Err(e) = fs::create_dir_all(&bus_dir) {
        tracing::warn!("failed to create {}: {e}", bus_dir.display());
        return;
    }

    let content = format!(
        "# This file is created by keytao-ime (IBus compatible)\nIBUS_ADDRESS={dbus_address}\nIBUS_DAEMON_PID={pid}\n"
    );

    let display_num = display_number();
    let wayland_num = wayland_display_number();

    let mut names = vec![
        format!("{machine_id}-unix-{display_num}"),
        format!("{machine_id}-unix-wayland-0"),
        format!("{machine_id}-unix-wayland-1"),
    ];
    if let Some(wn) = wayland_num {
        names.push(format!("{machine_id}-unix-wayland-{wn}"));
    }
    names.sort();
    names.dedup();

    for name in names {
        let path = bus_dir.join(&name);
        if let Err(e) = fs::write(&path, &content) {
            tracing::warn!("failed to write {}: {e}", path.display());
        } else {
            tracing::debug!("wrote IBus address file: {}", path.display());
        }
    }
}

fn session_bus_address() -> String {
    let uid = unsafe { libc::geteuid() };
    session_bus_address_from(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
        std::env::var("XDG_RUNTIME_DIR").ok(),
        uid,
    )
}

fn session_bus_address_from(
    dbus_session_bus_address: Option<String>,
    xdg_runtime_dir: Option<String>,
    uid: u32,
) -> String {
    dbus_session_bus_address
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            xdg_runtime_dir
                .filter(|value| !value.trim().is_empty())
                .map(|runtime_dir| format!("unix:path={runtime_dir}/bus"))
        })
        .unwrap_or_else(|| format!("unix:path=/run/user/{uid}/bus"))
}

pub(crate) fn read_machine_id() -> String {
    for path in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = fs::read_to_string(path) {
            let id = s.trim().to_owned();
            if !id.is_empty() {
                return id;
            }
        }
    }
    "unknown".to_owned()
}

pub(crate) fn display_number() -> u32 {
    std::env::var("DISPLAY")
        .ok()
        .and_then(|d| {
            d.rsplit(':')
                .next()
                .and_then(|s| s.split('.').next())
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0)
}

pub(crate) fn wayland_display_number() -> Option<u32> {
    std::env::var("WAYLAND_DISPLAY")
        .ok()
        .and_then(|d| d.rsplit('-').next().and_then(|s| s.parse().ok()))
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(engine: CoreEngine, frontend: crate::frontend::Frontend) {
    tracing::info!("IBus D-Bus backend starting");

    let builder = match connection::Builder::session() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to get session bus builder: {e}");
            return;
        }
    };
    let theme_resolver = Arc::new(ThemeResolver::from_default_locations());
    let builder = match builder.serve_at("/org/kde/kimpanel/inputmethod", Kimpanel) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to serve Kimpanel: {e}");
            return;
        }
    };

    let (tx, rx) = std::sync::mpsc::channel::<OverlayPanelMessage>();
    let x11_panel_available = spawn_x11_overlay_panel(rx);

    // Avoid reusing paths held by clients across re-exec, which keeps the PID.
    let ctx_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ (u64::from(std::process::id()) << 32);
    let ctx_counter = Arc::new(AtomicU64::new(ctx_seed));
    let conn = match builder.build().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("IBus D-Bus backend failed to connect: {e}");
            return;
        }
    };
    let contexts = ContextOwners::default();
    let changes = match context_owner_changes(&conn).await {
        Ok(changes) => changes,
        Err(e) => {
            tracing::error!("IBus: failed to watch input context owners: {e}");
            return;
        }
    };

    let kimpanel_ctxt = SignalContext::new(&conn, "/org/kde/kimpanel/inputmethod").ok();
    if let Err(e) = conn
        .object_server()
        .at(
            "/org/freedesktop/IBus",
            IBusBus {
                engine,
                ctx_counter,
                contexts: contexts.clone(),
                active: ActiveContext::default(),
                kimpanel_ctxt: kimpanel_ctxt.clone(),
                x11_panel_tx: tx,
                x11_panel_available,
                theme_resolver,
            },
        )
        .await
    {
        tracing::error!("IBus: failed to register bus object: {e}");
        return;
    }

    if let Err(e) = conn.request_name("org.freedesktop.IBus").await {
        tracing::error!("IBus: failed to request IBus name: {e}");
        return;
    }

    if let Err(e) = conn.request_name("org.kde.kimpanel.inputmethod").await {
        tracing::warn!("Kimpanel: failed to request Kimpanel name (running as secondary?): {e}");
    }

    let dbus_address = session_bus_address();
    write_ibus_address_files(&dbus_address);

    // Announce the conversion-mode indicator once; every input context then
    // keeps it current with UpdateProperty.
    if let Some(kctxt) = &kimpanel_ctxt {
        let status = kimpanel_mode_property(false);
        let props = [status.as_str()];
        if let Err(e) = Kimpanel::register_properties(kctxt, &props).await {
            tracing::warn!("Kimpanel: register_properties failed: {e}");
        }
        if let Err(e) = Kimpanel::update_property(kctxt, &status).await {
            tracing::warn!("Kimpanel: update_property failed: {e}");
        }
    }

    // Notify any already-connected IBus clients that the keytao engine is active.
    // Chromium/CEF clients that connected before this signal can use GetGlobalEngine instead.
    if let Ok(signal_ctx) = SignalContext::new(&conn, "/org/freedesktop/IBus") {
        IBusBus::global_engine_changed(&signal_ctx, "keytao")
            .await
            .ok();
    }

    tracing::info!("IBus D-Bus backend ready ({})", dbus_address);
    frontend.connected();
    let server = conn.object_server();
    tokio::select! {
        () = watch_context_owners(changes, &server, &contexts) => {
            tracing::warn!("IBus D-Bus owner watch closed");
        }
        () = async {
            let mut messages = zbus::MessageStream::from(&conn);
            while let Some(message) = messages.next().await {
                if let Err(error) = message {
                    tracing::error!("IBus D-Bus connection lost: {error}");
                    return;
                }
            }
            tracing::warn!("IBus D-Bus connection closed");
        } => {}
    }
}

async fn context_owner_changes(
    conn: &zbus::Connection,
) -> zbus::Result<NameOwnerChangedStream<'static>> {
    DBusProxy::new(conn)
        .await?
        .receive_name_owner_changed_with_args(&[(2, "")])
        .await
}

#[cfg(test)]
#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use keytao_core::{key_policy, Candidate};
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, BufReader};

    async fn create_test_context(client: &zbus::Connection, service: &str) -> OwnedObjectPath {
        client
            .call_method(
                Some(service),
                "/org/freedesktop/IBus",
                Some("org.freedesktop.IBus"),
                "CreateInputContext",
                &("disconnect-test",),
            )
            .await
            .unwrap()
            .body()
            .deserialize()
            .unwrap()
    }

    #[tokio::test]
    async fn disconnect_removes_all_client_contexts_and_releases_sessions() {
        let mut daemon = tokio::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("start isolated D-Bus daemon");
        let mut address = String::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address),
        )
        .await
        .expect("D-Bus daemon start timeout")
        .unwrap();
        let connect = || {
            connection::Builder::address(address.trim())
                .unwrap()
                .build()
        };
        let server = connect().await.unwrap();
        let changes = context_owner_changes(&server).await.unwrap();
        let dir = smoke_fixture::scratch_dir("ibus-disconnect");
        keytao_core::engine_choice::set_enabled(&dir, false).unwrap();
        let contexts = ContextOwners::default();
        let objects = server.object_server();
        let (x11_panel_tx, panel_events) = std::sync::mpsc::channel();
        let kimpanel_path = "/org/kde/kimpanel/inputmethod";
        objects
            .at(
                "/org/freedesktop/IBus",
                IBusBus {
                    engine: CoreEngine::with_dirs(&dir, dir.to_string_lossy()),
                    ctx_counter: Arc::new(AtomicU64::new(1)),
                    contexts: contexts.clone(),
                    active: ActiveContext::default(),
                    kimpanel_ctxt: Some(SignalContext::new(&server, kimpanel_path).unwrap()),
                    x11_panel_tx,
                    x11_panel_available: true,
                    theme_resolver: Arc::new(ThemeResolver::with_system_scheme(
                        None,
                        None,
                        Some(keytao_theme::EffectiveColorScheme::Light),
                    )),
                },
            )
            .await
            .unwrap();

        let exercise = async {
            let client = connect().await.unwrap();
            let other = connect().await.unwrap();
            let service = server.unique_name().unwrap().as_str();
            let kimpanel = zbus::Proxy::new(
                &other,
                service,
                kimpanel_path,
                "org.kde.kimpanel.inputmethod",
            )
            .await
            .unwrap();
            let mut lookup_visibility = kimpanel.receive_signal("ShowLookupTable").await.unwrap();
            let mut sessions = Vec::new();
            for _ in 0..2 {
                let path = create_test_context(&client, service).await;
                let context = objects.interface::<_, InputContext>(&path).await.unwrap();
                let session = Arc::downgrade(&context.get().await.session);
                sessions.push((path, session));
            }
            let other_path = create_test_context(&other, service).await;
            for (connection, path) in [(&client, &sessions[0].0), (&other, &other_path)] {
                connection
                    .call_method(
                        Some(service),
                        path,
                        Some("org.freedesktop.IBus.InputContext"),
                        "FocusIn",
                        &(),
                    )
                    .await
                    .unwrap();
            }
            assert_eq!(panel_events.try_iter().count(), 2);
            let signal = lookup_visibility.next().await.unwrap();
            assert!(!signal.body().deserialize::<bool>().unwrap());

            {
                let context = objects
                    .interface::<_, InputContext>(&sessions[0].0)
                    .await
                    .unwrap();
                let input = context.get().await;
                let active = input.active.lock().await;
                let mut state = ImeState::empty();
                state.candidates.push(Candidate {
                    text: "你".to_owned(),
                    comment: None,
                });
                input
                    .apply_ime_state(state, context.signal_context(), &active)
                    .await;
            }
            assert!(panel_events.try_recv().is_err());

            client.close().await.unwrap();
            while sessions
                .iter()
                .any(|(_, session)| session.strong_count() != 0)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            for (path, _) in &sessions {
                assert!(matches!(
                    objects.interface::<_, InputContext>(path).await,
                    Err(zbus::Error::InterfaceNotFound)
                ));
                assert!(matches!(
                    objects.interface::<_, InputContextService>(path).await,
                    Err(zbus::Error::InterfaceNotFound)
                ));
            }
            assert!(panel_events.try_recv().is_err());
            assert!(objects
                .interface::<_, InputContext>(&other_path)
                .await
                .is_ok());

            other
                .call_method(
                    Some(service),
                    &other_path,
                    Some("org.freedesktop.IBus.Service"),
                    "Destroy",
                    &(),
                )
                .await
                .unwrap();
            assert!(matches!(
                panel_events.try_recv(),
                Ok(OverlayPanelMessage::Hide)
            ));
            assert!(matches!(
                objects.interface::<_, InputContext>(&other_path).await,
                Err(zbus::Error::InterfaceNotFound)
            ));
            assert!(contexts.lock().unwrap().is_empty());
        };
        let result = tokio::select! {
            () = watch_context_owners(changes, &objects, &contexts) => panic!("D-Bus owner watch closed"),
            result = tokio::time::timeout(Duration::from_secs(5), exercise) => result,
        };
        fs::remove_dir_all(dir).unwrap();
        result.expect("client contexts were not released on disconnect");
    }

    #[test]
    fn empty_composition_backspace_bypasses_to_client() {
        let state = ImeState::empty();
        assert!(key_policy::should_bypass_empty_composition(
            0xff08, 0x10, &state
        ));
    }

    #[test]
    fn digits_are_not_intercepted_as_select_keys() {
        // The schema decides whether a digit selects a candidate; the shim must
        // never take that decision away from librime.
        let mut state = ImeState::empty();
        state.preedit = "ni".to_owned();
        state.candidates = vec![Candidate {
            text: "你".to_owned(),
            comment: None,
        }];
        assert!(!key_policy::should_bypass_empty_composition(
            0x31, 0, &state
        ));
        assert_eq!(key_policy::candidate_index_for_char('1', &state), None);
    }

    #[test]
    fn highlighted_candidate_requires_candidates() {
        let state = ImeState::empty();
        assert_eq!(key_policy::highlighted_candidate_index(&state), None);

        let mut state = ImeState::empty();
        state.candidates = vec![Candidate {
            text: "first".to_owned(),
            comment: None,
        }];
        state.highlighted_candidate_index = 9;
        assert_eq!(key_policy::highlighted_candidate_index(&state), Some(0));
    }

    #[test]
    fn session_bus_address_uses_current_uid_fallback() {
        assert_eq!(
            session_bus_address_from(None, None, 501),
            "unix:path=/run/user/501/bus"
        );
        assert_eq!(
            session_bus_address_from(None, Some("/run/user/502".to_owned()), 501),
            "unix:path=/run/user/502/bus"
        );
        assert_eq!(
            session_bus_address_from(Some("unix:path=/tmp/bus".to_owned()), None, 501),
            "unix:path=/tmp/bus"
        );
    }
}
