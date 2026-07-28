//! IBus engine backend for GNOME Wayland.
//!
//! GNOME/mutter does NOT support zwp_input_method_manager_v2.  Instead it
//! runs its own ibus-daemon and exposes input methods as IBus engines.
//!
//! This module implements the IBus Engine protocol:
//!   1. Connect to the ibus-daemon's own bus (IBUS_ADDRESS, not the session bus).
//!   2. Serve org.freedesktop.IBus.Factory at /org/freedesktop/IBus/Factory and
//!      own the component name so the daemon can find the factory.
//!   3. Call org.freedesktop.IBus.RegisterComponent for sessions where no
//!      component XML is installed.
//!   4. When the user switches to "keytao" in GNOME Settings / ibus-setup,
//!      the daemon calls Factory.CreateEngine("keytao").
//!   5. We create an engine object and return its path.
//!   6. The daemon routes ProcessKeyEvent calls to our engine object.
//!   7. Our engine emits CommitText / UpdatePreeditText / UpdateLookupTable
//!      signals which the daemon proxies to the focused application.

use crate::{
    engine::{ibus_cursor_pos, CoreEngine, ImeSession},
    ibus_shared::{
        self, ibus_lookup_table_value, ibus_text_value, ibus_text_variant, state_to_panel_model,
        ContentTypeState, KeyOutcome, ModeIndicator,
    },
    panel::{spawn_x11_overlay_panel, OverlayPanelMessage},
};
use keytao_core::ImeState;
use keytao_theme::ThemeResolver;
use std::sync::{
    atomic::{AtomicI32, AtomicU32, Ordering},
    Arc,
};
use zbus::{interface, object_server::SignalContext, proxy, zvariant};

// ── IBus constants ────────────────────────────────────────────────────────────

/// `IBusPreeditFocusMode::CLEAR`.  The engine drops the composition itself when
/// the input context goes away (see `focus_out`), so the client must not commit
/// the leftover preedit; the protocol's other value is `COMMIT` (1).
const IBUS_ENGINE_PREEDIT_CLEAR: u32 = 0;

/// `IBusPropType::NORMAL` and `IBusPropState::UNCHECKED`.
const IBUS_PROP_TYPE_NORMAL: u32 = 0;
const IBUS_PROP_STATE_UNCHECKED: u32 = 0;

/// Key of the conversion-mode property the panel shows in its indicator.
const MODE_PROPERTY_KEY: &str = "InputMode";

/// Bus name the component owns; it must match `<name>` in the component XML and
/// the name inside `ibus_component()`, because that is how ibus-daemon looks up
/// the factory of a component it started.
const IBUS_COMPONENT_BUS_NAME: &str = "org.freedesktop.IBus.KeyTao";

/// Well-known name of the desktop's own IBus panel (gnome-shell, ibus-ui-gtk3).
const IBUS_PANEL_BUS_NAME: &str = "org.freedesktop.IBus.Panel";

/// Draw the X11 overlay even when the desktop has its own panel.
const FORCE_OVERLAY_ENV: &str = "KEYTAO_IME_FORCE_OVERLAY";

// ── IBus type builders ────────────────────────────────────────────────────────

/// Build an IBusPropList: `("IBusPropList", a{sv}, av)`.
fn ibus_prop_list_variant(props: Vec<zvariant::Value<'static>>) -> zvariant::Value<'static> {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());
    let mut items = Array::new(sig_v);
    for prop in props {
        items.append(Value::Value(Box::new(prop))).ok();
    }
    let list = StructureBuilder::new()
        .add_field("IBusPropList".to_owned())
        .append_field(Value::Dict(empty_dict))
        .append_field(Value::Array(items))
        .build();
    Value::Structure(list)
}

/// Build an IBusProperty:
/// `("IBusProperty", a{sv}, key, type, label, icon, tooltip, sensitive,
///   visible, state, sub_props, symbol)`.
fn ibus_property_variant(
    key: &str,
    label: &str,
    symbol: &str,
    tooltip: &str,
) -> zvariant::Value<'static> {
    use zvariant::{Dict, Signature, StructureBuilder, Value};
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v);
    let property = StructureBuilder::new()
        .add_field("IBusProperty".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field(key.to_owned())
        .add_field(IBUS_PROP_TYPE_NORMAL)
        .append_field(Value::Value(Box::new(ibus_text_variant(label))))
        .add_field(String::new()) // icon
        .append_field(Value::Value(Box::new(ibus_text_variant(tooltip))))
        .add_field(true) // sensitive
        .add_field(true) // visible
        .add_field(IBUS_PROP_STATE_UNCHECKED)
        .append_field(Value::Value(Box::new(ibus_prop_list_variant(Vec::new()))))
        .append_field(Value::Value(Box::new(ibus_text_variant(symbol))))
        .build();
    Value::Structure(property)
}

fn ibus_mode_property_variant(ascii_mode: bool) -> zvariant::Value<'static> {
    let (label, symbol) = if ascii_mode {
        ("英文", "英")
    } else {
        ("中文", "中")
    };
    ibus_property_variant(MODE_PROPERTY_KEY, label, symbol, label)
}

/// Build an IBusEngineDesc OwnedValue.
fn ibus_engine_desc() -> zvariant::OwnedValue {
    use zvariant::{Dict, Signature, StructureBuilder, Value};
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v);
    let engine = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field("keytao".to_owned())
        .add_field("键道".to_owned())
        .add_field("KeyTao Input Method".to_owned())
        .add_field("zh".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("default".to_owned())
        .add_field(0u32)
        .add_field("".to_owned())
        .add_field("键".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .build();
    zvariant::OwnedValue::try_from(Value::Structure(engine)).expect("ibus_engine_desc")
}

/// Build an IBusComponent OwnedValue containing the keytao engine descriptor.
/// D-Bus type: (sa{sv}ssssssssavavav)
fn ibus_component() -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, OwnedValue, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());

    // engines array: av of IBusEngineDesc (each wrapped as v inside av)
    let mut engines = Array::new(sig_v.clone());
    let engine_ov: OwnedValue = ibus_engine_desc();
    let engine_v = Value::try_from(engine_ov).expect("engine value");
    engines
        .append(Value::Value(Box::new(engine_v)))
        .expect("append engine");

    let empty_paths = Array::new(sig_v.clone());
    let empty_sub = Array::new(sig_v);

    let exec = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "keytao-ime".to_string());

    let component = StructureBuilder::new()
        .add_field("IBusComponent".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field(IBUS_COMPONENT_BUS_NAME.to_owned())
        .add_field("KeyTao Input Method".to_owned())
        .add_field(env!("CARGO_PKG_VERSION").to_owned())
        .add_field("free".to_owned())
        .add_field("KeyTao Team".to_owned())
        .add_field("https://keytao.app".to_owned())
        .add_field(format!("{exec} --ibus-engine"))
        .add_field("keytao".to_owned())
        .append_field(Value::Array(empty_paths)) // observed_paths
        .append_field(Value::Array(empty_sub)) // sub_components
        .append_field(Value::Array(engines)) // engines
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(component)).expect("ibus_component")
}

// ── IBusBus proxy (to call RegisterComponent on the existing daemon) ──────────

#[proxy(
    interface = "org.freedesktop.IBus",
    default_service = "org.freedesktop.IBus",
    default_path = "/org/freedesktop/IBus"
)]
trait IBusBusDaemon {
    async fn register_component(&self, component: &zvariant::Value<'_>) -> zbus::Result<()>;
}

// ── IBus Factory (we expose this so the daemon can call CreateEngine) ─────────

pub struct IBusFactory {
    engine: CoreEngine,
    counter: Arc<AtomicU32>,
    theme_resolver: Arc<ThemeResolver>,
    x11_panel_tx: Option<std::sync::mpsc::Sender<OverlayPanelMessage>>,
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl IBusFactory {
    async fn create_engine(
        &self,
        engine_name: &str,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<zvariant::OwnedObjectPath> {
        tracing::info!("IBus Factory: CreateEngine({engine_name})");
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let path = format!("/org/freedesktop/IBus/engines/keytao{n}");
        let session = self
            .engine
            .create_session()
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let signal_ctxt = SignalContext::new(connection, path.clone()).ok();
        server
            .at(
                path.clone(),
                IBusEngine {
                    session: session.clone(),
                    theme_resolver: self.theme_resolver.clone(),
                    cursor_x: Arc::new(AtomicI32::new(0)),
                    cursor_y: Arc::new(AtomicI32::new(0)),
                    mode: ModeIndicator::new(),
                    x11_panel_tx: self.x11_panel_tx.clone(),
                    content_type: ContentTypeState::new(),
                    signal_ctxt,
                },
            )
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        // The daemon destroys engines through org.freedesktop.IBus.Service, so
        // that interface has to live at the same path as the engine.
        server
            .at(
                path.clone(),
                IBusEngineService {
                    session,
                    x11_panel_tx: self.x11_panel_tx.clone(),
                },
            )
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        zvariant::OwnedObjectPath::try_from(path)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

/// `org.freedesktop.IBus.Service` for the factory: the daemon calls it when it
/// drops the component.  Engines are destroyed through their own object, so
/// there is nothing to release here.
pub struct IBusFactoryService;

#[interface(name = "org.freedesktop.IBus.Service")]
impl IBusFactoryService {
    async fn destroy(&self) {
        tracing::info!("IBus Factory: Destroy");
    }
}

// ── IBus Engine ───────────────────────────────────────────────────────────────

pub struct IBusEngine {
    session: ImeSession,
    theme_resolver: Arc<ThemeResolver>,
    cursor_x: Arc<AtomicI32>,
    cursor_y: Arc<AtomicI32>,
    mode: ModeIndicator,
    /// `None` when the desktop runs its own IBus panel: drawing a second
    /// candidate window next to gnome-shell's would show every candidate twice.
    x11_panel_tx: Option<std::sync::mpsc::Sender<OverlayPanelMessage>>,
    content_type: ContentTypeState,
    /// Signal context for paths that do not get one from the caller, such as the
    /// `ContentType` property setter.
    signal_ctxt: Option<SignalContext<'static>>,
}

impl IBusEngine {
    fn overlay(&self, message: OverlayPanelMessage) {
        if let Some(tx) = &self.x11_panel_tx {
            let _ = tx.send(message);
        }
    }

    /// Whether the engine, not the desktop panel, draws the candidate list.
    fn draws_candidates(&self) -> bool {
        self.x11_panel_tx.is_some()
    }

    async fn clear_ui(&self, ctxt: &SignalContext<'_>) {
        let _ = IBusEngine::hide_preedit_text(ctxt).await;
        let _ = IBusEngine::hide_lookup_table(ctxt).await;
        self.overlay(OverlayPanelMessage::Hide);
    }

    fn show_mode_hint(&self, ascii_mode: bool) {
        let cx = self.cursor_x.load(Ordering::Relaxed);
        let cy = self.cursor_y.load(Ordering::Relaxed);
        self.overlay(OverlayPanelMessage::ModeHint {
            ascii_mode,
            x: cx,
            y: cy + 24,
        });
    }

    /// Announce the conversion-mode indicator, showing the mode Rime is actually
    /// in rather than the one this engine object last saw.
    async fn register_mode_property(&self, ctxt: &SignalContext<'_>) {
        let ascii_mode = self.session.state().ascii_mode;
        self.mode.adopt(ascii_mode);
        let props = ibus_prop_list_variant(vec![ibus_mode_property_variant(ascii_mode)]);
        let _ = IBusEngine::register_properties(ctxt, props).await;
    }

    /// Keep the panel indicator and the overlay hint in sync with Rime's mode.
    async fn apply_mode_change(&self, ascii_mode: bool, show_hint: bool, ctxt: &SignalContext<'_>) {
        let Some(change) = self.mode.update(ascii_mode, show_hint) else {
            return;
        };
        let _ =
            IBusEngine::update_property(ctxt, ibus_mode_property_variant(change.ascii_mode)).await;
        if change.hint {
            self.show_mode_hint(change.ascii_mode);
        }
    }

    async fn apply_ime_state(&self, ime_state: ImeState, ctxt: &SignalContext<'_>) {
        let ascii_mode = ime_state.ascii_mode;
        let has_candidates = !ime_state.candidates.is_empty();
        if let Some(ref text) = ime_state.committed {
            if !text.is_empty() {
                tracing::debug!("IBus Engine CommitText: {} chars", text.chars().count());
                tracing::trace!("IBus Engine CommitText: {text:?}");
                let _ = IBusEngine::update_preedit_text(
                    ctxt,
                    ibus_text_variant(""),
                    0,
                    false,
                    IBUS_ENGINE_PREEDIT_CLEAR,
                )
                .await;
                let ov = ibus_text_value(text);
                if let Ok(v) = zvariant::Value::try_from(&ov) {
                    let _ = IBusEngine::commit_text(ctxt, v).await;
                }
            }
        }

        if ime_state.preedit.is_empty() {
            let _ = IBusEngine::hide_preedit_text(ctxt).await;
        } else {
            // IBusText cursors count characters, which is also the unit
            // `ImeState::cursor` uses.
            let cursor = ibus_cursor_pos(&ime_state);
            let ov = ibus_text_value(&ime_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = IBusEngine::update_preedit_text(
                    ctxt,
                    v,
                    cursor,
                    true,
                    IBUS_ENGINE_PREEDIT_CLEAR,
                )
                .await;
            }
        }

        if ime_state.candidates.is_empty() {
            let _ = IBusEngine::hide_lookup_table(ctxt).await;
            self.overlay(OverlayPanelMessage::Hide);
        } else {
            let model = state_to_panel_model(&ime_state, &self.theme_resolver);
            let ov = ibus_lookup_table_value(&model);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                // Either the desktop panel renders the table or we draw it, never
                // both.
                let _ = IBusEngine::update_lookup_table(ctxt, v, !self.draws_candidates()).await;
            }
            let cx = self.cursor_x.load(Ordering::Relaxed);
            let cy = self.cursor_y.load(Ordering::Relaxed);
            self.overlay(OverlayPanelMessage::Show {
                state: ime_state.clone(),
                x: cx,
                y: cy + 24,
            });
        }

        self.apply_mode_change(ascii_mode, !has_candidates, ctxt)
            .await;
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

    /// Apply an IBus content-type: password and PIN fields stop composing so
    /// that no secret ever reaches librime or the user dictionary.
    async fn apply_content_type(&self, purpose: u32, hints: u32) {
        let change = self.content_type.set(purpose, hints);
        self.session.set_input_policy(change.policy);
        if change.turned_sensitive {
            tracing::debug!("IBus Engine: sensitive content type, passing keys through");
            if let Some(ctxt) = &self.signal_ctxt {
                self.clear_ui(ctxt).await;
            }
        }
    }
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl IBusEngine {
    // ── Signals (emitted by our engine to the daemon) ─────────────────────

    #[zbus(signal)]
    async fn commit_text(ctxt: &SignalContext<'_>, text: zvariant::Value<'_>) -> zbus::Result<()>;

    /// `mode` is an `IBusPreeditFocusMode`; ibus-daemon parses this signal as
    /// `(vubu)` and drops it when the fourth argument is missing.
    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: zvariant::Value<'_>,
        cursor_pos: u32,
        visible: bool,
        mode: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: zvariant::Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn forward_key_event(
        ctxt: &SignalContext<'_>,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn register_properties(
        ctxt: &SignalContext<'_>,
        props: zvariant::Value<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_property(
        ctxt: &SignalContext<'_>,
        prop: zvariant::Value<'_>,
    ) -> zbus::Result<()>;

    // ── Methods (called by the daemon on key/focus events) ────────────────

    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> bool {
        // The decision is shared with the standalone IBus shim; this object only
        // publishes the outcome over org.freedesktop.IBus.Engine.
        let outcome = ibus_shared::process_key_event(&self.session, keyval, state);
        let accepted = outcome.accepted();
        match outcome {
            KeyOutcome::Forward => {}
            KeyOutcome::ClearUi => self.clear_ui(&ctxt).await,
            KeyOutcome::ModeChanged { ascii_mode, .. } => {
                self.apply_mode_change(ascii_mode, true, &ctxt).await;
            }
            KeyOutcome::Publish { state, .. } => self.apply_ime_state(*state, &ctxt).await,
        }
        accepted
    }

    async fn focus_in(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus Engine: FocusIn");
        self.register_mode_property(&ctxt).await;
    }

    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.session.clear_composition();
        self.clear_ui(&ctxt).await;
    }

    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.session.clear_composition();
        self.clear_ui(&ctxt).await;
    }

    async fn enable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus Engine: Enable");
        self.register_mode_property(&ctxt).await;
    }

    async fn disable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.session.clear_composition();
        self.clear_ui(&ctxt).await;
    }

    async fn set_cursor_location(&self, x: i32, y: i32, _w: i32, _h: i32) {
        self.cursor_x.store(x, Ordering::Relaxed);
        self.cursor_y.store(y, Ordering::Relaxed);
    }
    async fn set_capabilities(&self, _caps: u32) {}
    async fn set_surrounding_text(&self, _text: zvariant::Value<'_>, _cursor: u32, _anchor: u32) {}

    /// ibus-daemon writes the content type through `Properties.Set`, so this has
    /// to be a property; the method below stays for older callers.
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

    #[zbus(property)]
    fn name(&self) -> String {
        "keytao".to_string()
    }
}

/// `org.freedesktop.IBus.Service` for an engine object.
///
/// `ibus_proxy_real_destroy()` calls this when the daemon drops an engine; not
/// answering it would leak one object and one librime session per input context.
pub struct IBusEngineService {
    session: ImeSession,
    x11_panel_tx: Option<std::sync::mpsc::Sender<OverlayPanelMessage>>,
}

#[interface(name = "org.freedesktop.IBus.Service")]
impl IBusEngineService {
    async fn destroy(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        tracing::info!("IBus Engine: Destroy {}", ctxt.path());
        self.session.clear_composition();
        if let Some(tx) = &self.x11_panel_tx {
            let _ = tx.send(OverlayPanelMessage::Hide);
        }
        // Both interfaces have to go before the object server drops the node.
        let path = ctxt.path().to_owned();
        server
            .remove::<IBusEngine, _>(&path)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        server
            .remove::<IBusEngineService, _>(&path)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }
}

// ── IBus bus discovery ────────────────────────────────────────────────────────

/// Address of the ibus-daemon bus.
///
/// ibus-daemon runs its own bus and only owns `org.freedesktop.IBus` on the
/// session bus as a discovery marker, so an engine that talks to the session
/// bus never reaches the daemon.
fn ibus_bus_address() -> Option<String> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS") {
        let address = address.trim().to_owned();
        if !address.is_empty() {
            return Some(address);
        }
    }
    ibus_address_from_files()
}

fn ibus_address_from_files() -> Option<String> {
    let bus_dir = dirs::config_dir()?.join("ibus").join("bus");
    let machine_id = crate::ibus_backend::read_machine_id();
    let mut names = Vec::new();
    if let Some(number) = crate::ibus_backend::wayland_display_number() {
        names.push(format!("{machine_id}-unix-wayland-{number}"));
    }
    names.push(format!("{machine_id}-unix-wayland-0"));
    names.push(format!(
        "{machine_id}-unix-{}",
        crate::ibus_backend::display_number()
    ));

    for name in names {
        if let Some(address) = read_ibus_address_file(&bus_dir.join(name)) {
            return Some(address);
        }
    }
    None
}

fn read_ibus_address_file(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut address = None;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("IBUS_ADDRESS=") {
            address = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("IBUS_DAEMON_PID=") {
            // The IBus shim in this very process writes the same files; talking
            // to ourselves would register the engine with nobody.
            if value.trim().parse::<u32>().ok() == Some(std::process::id()) {
                return None;
            }
        }
    }
    address.filter(|address| !address.is_empty())
}

/// Whether this process draws the candidate list itself.
///
/// The desktop panel renders every lookup table it is sent, so running the X11
/// overlay next to gnome-shell puts two candidate lists on screen; the overlay
/// only exists for sessions that have no panel of their own.
fn should_draw_overlay(force: bool, panel_available: bool, has_display: bool) -> bool {
    force || (!panel_available && has_display)
}

async fn ibus_panel_available(conn: &zbus::Connection) -> bool {
    let Ok(proxy) = zbus::fdo::DBusProxy::new(conn).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(IBUS_PANEL_BUS_NAME) else {
        return false;
    };
    proxy.name_has_owner(name).await.unwrap_or(false)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Connect to the running IBus daemon (GNOME's ibus-daemon) and register as an
/// IBus engine.  Blocks until the connection drops or the process exits.
pub async fn run(engine: CoreEngine) {
    tracing::info!("IBus engine backend starting (GNOME mode)");

    let address = ibus_bus_address();
    let builder = match &address {
        Some(address) => {
            tracing::info!("IBus engine backend: connecting to daemon bus {address}");
            zbus::connection::Builder::address(address.as_str())
        }
        None => {
            tracing::warn!(
                "IBus engine backend: no IBUS_ADDRESS and no ~/.config/ibus/bus address file; \
                 falling back to the session bus"
            );
            zbus::connection::Builder::session()
        }
    };

    let conn = match builder.map(|b| b.build()) {
        Ok(fut) => match fut.await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("IBus engine backend: failed to connect to the IBus bus: {e}");
                return;
            }
        },
        Err(e) => {
            tracing::error!("IBus engine backend: builder error: {e}");
            return;
        }
    };

    // GNOME renders the lookup table itself.  Only fall back to the X11 overlay
    // when no panel owns the name, otherwise every candidate shows up twice.
    let force_overlay = std::env::var_os(FORCE_OVERLAY_ENV).is_some();
    let has_panel = ibus_panel_available(&conn).await;
    let has_display = std::env::var_os("DISPLAY").is_some();
    let x11_panel_tx = if should_draw_overlay(force_overlay, has_panel, has_display) {
        let (tx, rx) = std::sync::mpsc::channel::<OverlayPanelMessage>();
        spawn_x11_overlay_panel(rx);
        tracing::info!("IBus engine backend: drawing candidates in the X11 overlay");
        Some(tx)
    } else {
        tracing::info!("IBus engine backend: candidates rendered by the desktop IBus panel");
        None
    };

    let factory = IBusFactory {
        engine,
        counter: Arc::new(AtomicU32::new(0)),
        theme_resolver: Arc::new(ThemeResolver::from_default_locations()),
        x11_panel_tx,
    };

    if let Err(e) = conn
        .object_server()
        .at("/org/freedesktop/IBus/Factory", factory)
        .await
    {
        tracing::error!("IBus engine backend: failed to serve the factory: {e}");
        return;
    }
    if let Err(e) = conn
        .object_server()
        .at("/org/freedesktop/IBus/Factory", IBusFactoryService)
        .await
    {
        tracing::warn!("IBus engine backend: failed to serve the factory service interface: {e}");
    }

    tracing::info!(
        "IBus engine factory ready at /org/freedesktop/IBus/Factory (unique name: {})",
        conn.unique_name().map(|n| n.as_str()).unwrap_or("?")
    );

    // Register our component with the running IBus daemon so that sessions
    // without an installed component XML still see the "keytao" engine.
    let component_ov = ibus_component();
    match zvariant::Value::try_from(&component_ov) {
        Ok(component_v) => match IBusBusDaemonProxy::new(&conn).await {
            Ok(proxy) => match proxy.register_component(&component_v).await {
                Ok(()) => tracing::info!("IBus component registered with daemon"),
                Err(e) => tracing::warn!(
                    "IBus RegisterComponent failed (the installed component XML takes over): {e}"
                ),
            },
            Err(e) => tracing::warn!("IBus daemon proxy failed: {e}"),
        },
        Err(e) => tracing::warn!("Failed to serialize IBus component: {e}"),
    }

    // ibus-daemon looks the factory up by the component's bus name.
    match conn.request_name(IBUS_COMPONENT_BUS_NAME).await {
        Ok(()) => tracing::info!("IBus component name {IBUS_COMPONENT_BUS_NAME} acquired"),
        Err(e) => tracing::error!("IBus engine backend: cannot own {IBUS_COMPONENT_BUS_NAME}: {e}"),
    }

    tracing::info!(
        "IBus engine backend ready. \
         To activate: open ibus-setup or GNOME Settings → Keyboard → Input Sources \
         and add 'KeyTao' (Other → Chinese → KeyTao)."
    );

    let _conn = conn;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ibus_mode_property_variant, ibus_prop_list_variant, read_ibus_address_file,
        should_draw_overlay,
    };
    use zbus::zvariant::{self, Type};

    #[test]
    fn mode_property_matches_the_ibus_property_signature() {
        let value = ibus_mode_property_variant(false);
        assert_eq!(value.value_signature().as_str(), "(sa{sv}suvsvbbuvv)");
    }

    #[test]
    fn prop_list_matches_the_ibus_prop_list_signature() {
        let value = ibus_prop_list_variant(vec![ibus_mode_property_variant(true)]);
        assert_eq!(value.value_signature().as_str(), "(sa{sv}av)");
    }

    #[test]
    fn engine_desc_and_component_keep_their_signatures() {
        // The daemon rejects the whole component when a field type drifts.
        let component = super::ibus_component();
        assert_eq!(
            zvariant::Value::try_from(&component)
                .expect("component value")
                .value_signature()
                .as_str(),
            "(sa{sv}ssssssssavavav)"
        );
        assert_eq!(
            <(u32, u32)>::signature().as_str(),
            "(uu)",
            "the ContentType property must stay (uu)"
        );
    }

    #[test]
    fn the_overlay_stays_off_while_a_desktop_panel_is_running() {
        // GNOME: a panel owns the name and XWayland provides DISPLAY.
        assert!(!should_draw_overlay(false, true, true));
        assert!(!should_draw_overlay(false, true, false));
        // No panel: the overlay is the only candidate UI, but it needs X11.
        assert!(should_draw_overlay(false, false, true));
        assert!(!should_draw_overlay(false, false, false));
        // KEYTAO_IME_FORCE_OVERLAY wins over the panel probe.
        assert!(should_draw_overlay(true, true, true));
    }

    #[test]
    fn address_files_written_by_this_process_are_ignored() {
        let dir = std::env::temp_dir().join(format!("keytao-ibus-address-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let foreign = dir.join("foreign");
        std::fs::write(
            &foreign,
            "IBUS_ADDRESS=unix:path=/run/user/1000/ibus/dbus-abc\nIBUS_DAEMON_PID=4242\n",
        )
        .expect("write");
        assert_eq!(
            read_ibus_address_file(&foreign).as_deref(),
            Some("unix:path=/run/user/1000/ibus/dbus-abc")
        );

        let ours = dir.join("ours");
        std::fs::write(
            &ours,
            format!(
                "IBUS_ADDRESS=unix:path=/run/user/1000/ibus/dbus-abc\nIBUS_DAEMON_PID={}\n",
                std::process::id()
            ),
        )
        .expect("write");
        assert_eq!(read_ibus_address_file(&ours), None);

        assert_eq!(read_ibus_address_file(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
