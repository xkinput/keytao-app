//! IBus D-Bus backend for keytao-ime.
//!
//! Implements enough of the IBus D-Bus protocol so that Chromium/CEF apps
//! (e.g. WeChatAppEx) can use keytao as their IME without requiring a real
//! IBus daemon.

use crate::engine::{ibus_cursor_pos, CoreEngine, ImeSession};
use crate::ibus_shared::{
    self, candidate_display_text, ibus_lookup_table_value, ibus_text_value, state_to_panel_model,
    ContentTypeState, KeyOutcome, ModeIndicator,
};
use crate::kimpanel::mode_property as kimpanel_mode_property;
use crate::panel::{spawn_x11_overlay_panel, OverlayPanelMessage};
use keytao_core::ImeState;
use keytao_theme::ThemeResolver;
use std::{
    fs,
    sync::{
        atomic::{AtomicI32, AtomicU32, Ordering},
        Arc,
    },
};
use zbus::{connection, interface, object_server::SignalContext, zvariant};

/// Build an IBusEngineDesc value for the "keytao" engine.
/// Structure: (sa{sv} name longname description language license author icon layout rank hotkeys symbol setup layout_variant layout_option version textdomain)
fn ibus_engine_desc_value() -> zvariant::OwnedValue {
    use zvariant::{Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v);

    let engine = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field("keytao".to_owned()) // name
        .add_field("KeyTao".to_owned()) // longname
        .add_field("KeyTao Input Method".to_owned()) // description
        .add_field("zh".to_owned()) // language
        .add_field("".to_owned()) // license
        .add_field("".to_owned()) // author
        .add_field("".to_owned()) // icon
        .add_field("default".to_owned()) // layout
        .add_field(0u32) // rank
        .add_field("".to_owned()) // hotkeys
        .add_field("键".to_owned()) // symbol
        .add_field("".to_owned()) // setup
        .add_field("".to_owned()) // layout_variant
        .add_field("".to_owned()) // layout_option
        .add_field("".to_owned()) // version
        .add_field("".to_owned()) // textdomain
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(engine)).expect("ibus_engine_desc_value")
}

// ── InputContext D-Bus object ─────────────────────────────────────────────────

struct InputContext {
    session: ImeSession,
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
    async fn clear_ui(&self, ctxt: &SignalContext<'_>) {
        let _ = Self::hide_preedit_text(ctxt).await;
        let _ = Self::hide_lookup_table(ctxt).await;
        if let Some(kc) = &self.kimpanel_ctxt {
            let _ = Kimpanel::show_preedit(kc, false).await;
            let _ = Kimpanel::show_lookup_table(kc, false).await;
        }
        let _ = self.x11_panel_tx.send(OverlayPanelMessage::Hide);
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
        let ascii_mode = self.session.state().ascii_mode;
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
        let change = self.content_type.set(purpose, hints);
        self.session.set_input_policy(change.policy);
        if change.turned_sensitive {
            tracing::debug!("IBus InputContext: sensitive content type, passing keys through");
            if let Some(ctxt) = &self.signal_ctxt {
                self.clear_ui(ctxt).await;
            }
        }
    }

    async fn apply_ime_state(&self, ime_state: ImeState, ctxt: &SignalContext<'_>) {
        let ascii_mode = ime_state.ascii_mode;
        let has_candidates = !ime_state.candidates.is_empty();
        if let Some(ref text) = ime_state.committed {
            if !text.is_empty() {
                tracing::debug!("IBus CommitText: {} chars", text.chars().count());
                tracing::trace!("IBus CommitText: {text:?}");
                clear_preedit(ctxt, &self.kimpanel_ctxt).await;
                let ov = ibus_text_value(text);
                if let Ok(v) = zvariant::Value::try_from(&ov) {
                    let _ = Self::commit_text(ctxt, v).await;
                }
            }
        }

        if ime_state.preedit.is_empty() {
            let _ = Self::hide_preedit_text(ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_preedit(kctxt, false).await;
            }
        } else {
            clear_preedit(ctxt, &None).await;

            // IBusText cursors count characters, which is also the unit
            // `ImeState::cursor` uses.
            let cursor = ibus_cursor_pos(&ime_state);
            let ov = ibus_text_value(&ime_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_preedit_text(ctxt, v, cursor, true).await;
            }
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::update_preedit_text(kctxt, &ime_state.preedit, "").await;
                let _ = Kimpanel::show_preedit(kctxt, true).await;
            }
        }

        if ime_state.candidates.is_empty() {
            let _ = Self::hide_lookup_table(ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_lookup_table(kctxt, false).await;
            }
            let _ = self.x11_panel_tx.send(OverlayPanelMessage::Hide);
        } else {
            let model = state_to_panel_model(&ime_state, &self.theme_resolver);
            let ov = ibus_lookup_table_value(&model);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_lookup_table(ctxt, v, true).await;
            }
            if let Some(kctxt) = &self.kimpanel_ctxt {
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

            let cx = self.cursor_x.load(Ordering::Relaxed);
            let cy = self.cursor_y.load(Ordering::Relaxed);
            let _ = self.x11_panel_tx.send(OverlayPanelMessage::Show {
                state: ime_state,
                x: cx,
                y: cy + 24,
            });
        }

        self.apply_mode_change(ascii_mode, !has_candidates).await;
    }

    async fn select_candidate_at(&self, index: usize, ctxt: &SignalContext<'_>) {
        if let Some(ime_state) = ibus_shared::select_candidate(&self.session, index) {
            self.apply_ime_state(ime_state, ctxt).await;
        }
    }

    async fn change_page(&self, backward: bool, ctxt: &SignalContext<'_>) {
        if let Some(ime_state) = ibus_shared::change_page(&self.session, backward) {
            self.apply_ime_state(ime_state, ctxt).await;
        }
    }

    async fn process_navigation_key(&self, keyval: u32, ctxt: &SignalContext<'_>) {
        if let Some(ime_state) = ibus_shared::navigation_key(&self.session, keyval) {
            self.apply_ime_state(ime_state, ctxt).await;
        }
    }
}

#[interface(name = "org.freedesktop.IBus.InputContext")]
impl InputContext {
    async fn focus_in(&self) {
        tracing::debug!("IBus InputContext: FocusIn");
        self.publish_current_mode().await;
    }

    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus InputContext: FocusOut");
        self.session.clear_composition();
        self.clear_ui(&ctxt).await;
    }

    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus InputContext: Reset");
        self.session.clear_composition();
        self.clear_ui(&ctxt).await;
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
    async fn set_content_type(&self, value: (u32, u32)) {
        let (purpose, hints) = value;
        self.apply_content_type(purpose, hints).await;
    }

    #[zbus(name = "SetContentType")]
    async fn set_content_type_method(&self, purpose: u32, hints: u32) {
        self.apply_content_type(purpose, hints).await;
    }

    async fn enable(&self) {
        tracing::debug!("IBus InputContext: Enable");
        self.publish_current_mode().await;
    }

    async fn disable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus InputContext: Disable");
        self.session.clear_composition();
        self.clear_ui(&ctxt).await;
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
    ) -> bool {
        // The decision is shared with the GNOME engine backend; this object only
        // publishes the outcome over org.freedesktop.IBus.InputContext.
        let outcome = ibus_shared::process_key_event(&self.session, keyval, state);
        let accepted = outcome.accepted();
        match outcome {
            KeyOutcome::Forward => {}
            KeyOutcome::ClearUi => self.clear_ui(&ctxt).await,
            KeyOutcome::ModeChanged { ascii_mode, .. } => {
                self.apply_mode_change(ascii_mode, true).await;
            }
            KeyOutcome::Publish { state, .. } => self.apply_ime_state(*state, &ctxt).await,
        }
        accepted
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
    session: ImeSession,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    x11_panel_tx: std::sync::mpsc::Sender<OverlayPanelMessage>,
}

#[interface(name = "org.freedesktop.IBus.Service")]
impl InputContextService {
    async fn destroy(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        tracing::info!("IBus InputContext: Destroy {}", ctxt.path());
        self.session.clear_composition();
        if let Some(kctxt) = &self.kimpanel_ctxt {
            let _ = Kimpanel::show_preedit(kctxt, false).await;
            let _ = Kimpanel::show_lookup_table(kctxt, false).await;
        }
        let _ = self.x11_panel_tx.send(OverlayPanelMessage::Hide);

        // Both interfaces have to go before the object server drops the node.
        let path = ctxt.path().to_owned();
        server
            .remove::<InputContext, _>(&path)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        server
            .remove::<InputContextService, _>(&path)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }
}

/// Send an empty UpdatePreeditText to tell the client the composition ended
/// before committing. This is the sequence Chromium/CEF requires so that it
/// can correctly place the committed text without conflating it with the
/// still-active preedit region.
async fn clear_preedit(ctxt: &SignalContext<'_>, kctxt: &Option<SignalContext<'static>>) {
    let ov = ibus_text_value("");
    if let Ok(v) = zvariant::Value::try_from(&ov) {
        let _ = InputContext::update_preedit_text(ctxt, v, 0, false).await;
    }
    if let Some(kc) = kctxt {
        let _ = Kimpanel::update_preedit_text(kc, "", "").await;
        let _ = Kimpanel::show_preedit(kc, false).await;
    }
}

// ── IBusBus D-Bus object ──────────────────────────────────────────────────────

struct Kimpanel;

#[interface(name = "org.kde.kimpanel.inputmethod")]
impl Kimpanel {
    #[zbus(signal)]
    async fn update_spot_location(ctxt: &SignalContext<'_>, x: i32, y: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        labels: &[&str],
        candidates: &[&str],
        attrs: &[&str],
        has_prev: bool,
        has_next: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_lookup_table(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: &str,
        attr: &str,
    ) -> zbus::Result<()>;

    /// The Kimpanel visibility signal is `ShowPreedit` — no `Text` suffix,
    /// unlike the IBus signal of a similar name above.
    #[zbus(signal)]
    async fn show_preedit(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn register_properties(ctxt: &SignalContext<'_>, props: &[&str]) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_property(ctxt: &SignalContext<'_>, prop: &str) -> zbus::Result<()>;
}

struct IBusBus {
    engine: CoreEngine,
    ctx_counter: Arc<AtomicU32>,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    x11_panel_tx: std::sync::mpsc::Sender<OverlayPanelMessage>,
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
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        let n = self.ctx_counter.fetch_add(1, Ordering::SeqCst);
        let path_str = format!("/org/freedesktop/IBus/InputContext_{n}");
        let path = zbus::zvariant::OwnedObjectPath::try_from(path_str.clone())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        tracing::info!("IBus CreateInputContext client={client_name:?} -> {path_str}");

        let session = self
            .engine
            .create_session()
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let ctx = InputContext {
            session: session.clone(),
            kimpanel_ctxt: self.kimpanel_ctxt.clone(),
            cursor_x: Arc::new(AtomicI32::new(0)),
            cursor_y: Arc::new(AtomicI32::new(0)),
            mode: ModeIndicator::new(),
            x11_panel_tx: self.x11_panel_tx.clone(),
            theme_resolver: self.theme_resolver.clone(),
            content_type: ContentTypeState::new(),
            signal_ctxt: SignalContext::new(connection, path_str.clone()).ok(),
        };
        server
            .at(path.clone(), ctx)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        // Clients drop the context through org.freedesktop.IBus.Service, so that
        // interface has to live at the same path as the context.
        server
            .at(
                path.clone(),
                InputContextService {
                    session,
                    kimpanel_ctxt: self.kimpanel_ctxt.clone(),
                    x11_panel_tx: self.x11_panel_tx.clone(),
                },
            )
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        Ok(path)
    }

    async fn is_global_engine(&self) -> bool {
        true
    }

    async fn get_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![ibus_engine_desc_value()]
    }

    async fn list_active_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![ibus_engine_desc_value()]
    }

    async fn get_global_engine(&self) -> zbus::fdo::Result<zvariant::OwnedValue> {
        Ok(ibus_engine_desc_value())
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

pub async fn run(engine: CoreEngine) {
    tracing::info!("IBus D-Bus backend starting");

    let builder = match connection::Builder::session() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to get session bus builder: {e}");
            return;
        }
    };
    let engine_clone = engine.clone();
    let theme_resolver = Arc::new(ThemeResolver::from_default_locations());
    let theme_resolver_clone = theme_resolver.clone();
    let builder = match builder.serve_at("/org/kde/kimpanel/inputmethod", Kimpanel) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to serve Kimpanel: {e}");
            return;
        }
    };

    let (tx, rx) = std::sync::mpsc::channel::<OverlayPanelMessage>();
    let tx_clone = tx.clone();
    spawn_x11_overlay_panel(rx);

    let builder = match builder.serve_at(
        "/org/freedesktop/IBus",
        IBusBus {
            engine,
            ctx_counter: Arc::new(AtomicU32::new(1)),
            kimpanel_ctxt: None, // Will fill after build
            x11_panel_tx: tx,
            theme_resolver,
        },
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to serve_at: {e}");
            return;
        }
    };

    let conn = match builder.build().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("IBus D-Bus backend failed to connect: {e}");
            return;
        }
    };

    if let Err(e) = conn.request_name("org.freedesktop.IBus").await {
        tracing::error!("IBus: failed to request IBus name: {e}");
        return;
    }

    if let Err(e) = conn.request_name("org.kde.kimpanel.inputmethod").await {
        tracing::warn!("Kimpanel: failed to request Kimpanel name (running as secondary?): {e}");
    }

    let dbus_address = session_bus_address();
    write_ibus_address_files(&dbus_address);

    let kimpanel_ctxt = SignalContext::new(&conn, "/org/kde/kimpanel/inputmethod").ok();

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

    // We need to update the IBusBus instance with the kimpanel_ctxt.
    // However, IBusBus is owned by the ObjectServer. Instead of mutating it, we just set
    // it properly before serving if possible, or use a shared state.
    // Actually, we can just create the SignalContext from `conn` and share it!

    // Let's re-register IBusBus with the valid kimpanel_ctxt.
    let _ = conn
        .object_server()
        .remove::<IBusBus, _>("/org/freedesktop/IBus")
        .await;
    let _ = conn
        .object_server()
        .at(
            "/org/freedesktop/IBus",
            IBusBus {
                engine: engine_clone,
                ctx_counter: Arc::new(AtomicU32::new(1)),
                kimpanel_ctxt,
                x11_panel_tx: tx_clone,
                theme_resolver: theme_resolver_clone,
            },
        )
        .await;

    // Notify any already-connected IBus clients that the keytao engine is active.
    // Chromium/CEF clients that connected before this signal can use GetGlobalEngine instead.
    if let Ok(signal_ctx) = SignalContext::new(&conn, "/org/freedesktop/IBus") {
        IBusBus::global_engine_changed(&signal_ctx, "keytao")
            .await
            .ok();
    }

    tracing::info!("IBus D-Bus backend ready ({})", dbus_address);
    let _conn = conn; // keep connection alive

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::session_bus_address_from;
    use keytao_core::{key_policy, Candidate, ImeState};

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
