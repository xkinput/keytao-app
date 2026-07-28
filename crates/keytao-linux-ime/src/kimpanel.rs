use keytao_core::ImeState;
use keytao_theme::{CandidatePanelInput, ThemeCandidate, ThemeResolver, UiCapabilities};
use std::sync::Arc;
use zbus::{connection, interface, object_server::SignalContext, Connection};

const KIMPANEL_BUS_NAME: &str = "org.kde.kimpanel.inputmethod";
const KIMPANEL_OBJECT_PATH: &str = "/kimpanel";
const IMPANEL_BUS_NAME: &str = "org.kde.impanel";
const IMPANEL_OBJECT_PATH: &str = "/org/kde/impanel";
const IMPANEL2_INTERFACE: &str = "org.kde.impanel2";
const CANDIDATE_LAYOUT_NOT_SET: i32 = 0;
/// Property key of the conversion-mode indicator.
const KIMPANEL_STATUS_KEY: &str = "/KeyTao/im";

/// A Kimpanel property string: `key:label:icon:text:hint`.  The Plasma applet
/// reads `label=` out of the hint field and shows it in the tray indicator, so
/// this is how the desktop learns whether Rime is in Chinese or ASCII mode.
pub(crate) fn mode_property(ascii_mode: bool) -> String {
    let label = if ascii_mode { "英" } else { "中" };
    format!("{KIMPANEL_STATUS_KEY}:KeyTao:input-keyboard:KeyTao:menu,label={label}")
}

struct Kimpanel;

#[interface(name = "org.kde.kimpanel.inputmethod")]
impl Kimpanel {
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
    async fn update_lookup_table_cursor(ctxt: &SignalContext<'_>, pos: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: &str,
        attr: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_caret(ctxt: &SignalContext<'_>, pos: i32) -> zbus::Result<()>;

    /// The Kimpanel visibility signal is `ShowPreedit` — no `Text` suffix,
    /// unlike the IBus signal of a similar name.  The Plasma applet listens
    /// for this name only.
    #[zbus(signal)]
    async fn show_preedit(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_aux(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_aux(ctxt: &SignalContext<'_>, text: &str, attr: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn enable(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn register_properties(ctxt: &SignalContext<'_>, props: &[&str]) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_property(ctxt: &SignalContext<'_>, prop: &str) -> zbus::Result<()>;
}

#[derive(Clone)]
pub struct KimpanelHandle {
    ctxt: SignalContext<'static>,
    _conn: Connection,
    theme_resolver: Arc<ThemeResolver>,
}

impl KimpanelHandle {
    pub async fn new() -> Option<Self> {
        let builder = match connection::Builder::session() {
            Ok(builder) => builder,
            Err(e) => {
                tracing::warn!("Kimpanel: failed to get session bus builder: {e}");
                return None;
            }
        };
        let builder = match builder.serve_at(KIMPANEL_OBJECT_PATH, Kimpanel) {
            Ok(builder) => builder,
            Err(e) => {
                tracing::warn!("Kimpanel: failed to serve object: {e}");
                return None;
            }
        };
        let conn = match builder.build().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("Kimpanel: failed to connect session bus: {e}");
                return None;
            }
        };
        if let Err(e) = conn.request_name(KIMPANEL_BUS_NAME).await {
            tracing::warn!(
                "Kimpanel: failed to request Kimpanel name (running as secondary?): {e}"
            );
        }
        let ctxt = match SignalContext::new(&conn, KIMPANEL_OBJECT_PATH) {
            Ok(ctxt) => ctxt,
            Err(e) => {
                tracing::warn!("Kimpanel: failed to create signal context: {e}");
                return None;
            }
        };
        tracing::info!("Kimpanel D-Bus panel ready");
        let handle = Self {
            ctxt,
            _conn: conn,
            theme_resolver: Arc::new(ThemeResolver::from_default_locations()),
        };
        handle.register_status().await;
        Some(handle)
    }

    pub async fn clear(&self) {
        let _ = self.set_lookup_table(&[], &[], &[], false, false, -1).await;
        let _ = Kimpanel::update_aux(&self.ctxt, "", "").await;
        let _ = Kimpanel::show_aux(&self.ctxt, false).await;
        let _ = Kimpanel::update_preedit_text(&self.ctxt, "", "").await;
        let _ = Kimpanel::update_preedit_caret(&self.ctxt, 0).await;
        let _ = Kimpanel::show_preedit(&self.ctxt, false).await;
        let _ = Kimpanel::show_lookup_table(&self.ctxt, false).await;
    }

    pub async fn update_state(&self, state: &ImeState) {
        tracing::debug!(
            "Kimpanel: updating state, preedit={} chars, candidates_len={}",
            state.preedit.chars().count(),
            state.candidates.len()
        );
        if state.preedit.is_empty() {
            if let Err(e) = Kimpanel::show_preedit(&self.ctxt, false).await {
                tracing::warn!("Kimpanel: show_preedit(false) failed: {e}");
            }
        } else {
            if let Err(e) = Kimpanel::update_preedit_text(&self.ctxt, &state.preedit, "").await {
                tracing::warn!("Kimpanel: update_preedit_text failed: {e}");
            }
            let caret = state.preedit.chars().count() as i32;
            if let Err(e) = Kimpanel::update_preedit_caret(&self.ctxt, caret).await {
                tracing::warn!("Kimpanel: update_preedit_caret failed: {e}");
            }
            if let Err(e) = Kimpanel::show_preedit(&self.ctxt, true).await {
                tracing::warn!("Kimpanel: show_preedit(true) failed: {e}");
            }
        }

        if state.candidates.is_empty() {
            if let Err(e) = self.set_lookup_table(&[], &[], &[], false, false, -1).await {
                tracing::warn!("Kimpanel: set_lookup_table(empty) failed: {e}");
            }
            if let Err(e) = Kimpanel::show_lookup_table(&self.ctxt, false).await {
                tracing::warn!("Kimpanel: show_lookup_table(false) failed: {e}");
            }
            return;
        }

        let theme = self.theme_resolver.current();
        let model = theme.candidate_panel_model(
            state_to_panel_input(state),
            &UiCapabilities::system_lookup_table(),
        );
        let labels: Vec<String> = model
            .candidates
            .iter()
            .map(|candidate| candidate.label.clone())
            .collect();
        let candidates: Vec<String> = model
            .candidates
            .iter()
            .map(candidate_display_text)
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let attrs: Vec<&str> = Vec::new();
        let cursor = model
            .candidates
            .iter()
            .position(|candidate| candidate.selected)
            .unwrap_or(0) as i32;

        if let Err(e) = self
            .set_lookup_table(
                &label_refs,
                &candidate_refs,
                &attrs,
                model.navigation.can_go_previous,
                model.navigation.can_go_next,
                cursor,
            )
            .await
        {
            tracing::warn!("Kimpanel: set_lookup_table failed: {e}");
        }
        if let Err(e) = Kimpanel::update_lookup_table(
            &self.ctxt,
            &label_refs,
            &candidate_refs,
            &attrs,
            model.navigation.can_go_previous,
            model.navigation.can_go_next,
        )
        .await
        {
            tracing::warn!("Kimpanel: update_lookup_table failed: {e}");
        }
        if let Err(e) = Kimpanel::update_lookup_table_cursor(&self.ctxt, cursor).await {
            tracing::warn!("Kimpanel: update_lookup_table_cursor failed: {e}");
        }
        if let Err(e) = Kimpanel::show_lookup_table(&self.ctxt, true).await {
            tracing::warn!("Kimpanel: show_lookup_table(true) failed: {e}");
        }
    }

    /// Publish the conversion mode to the desktop indicator.  Registering the
    /// property once at startup is not enough: the applet keeps showing the
    /// registered value until an UpdateProperty arrives.
    pub async fn update_mode(&self, ascii_mode: bool) {
        if let Err(e) = Kimpanel::update_property(&self.ctxt, &mode_property(ascii_mode)).await {
            tracing::warn!("Kimpanel: update_property failed: {e}");
        }
    }

    async fn register_status(&self) {
        let status = mode_property(false);
        let props = [status.as_str()];
        if let Err(e) = Kimpanel::register_properties(&self.ctxt, &props).await {
            tracing::warn!("Kimpanel: register_properties failed: {e}");
        }
        if let Err(e) = Kimpanel::update_property(&self.ctxt, &status).await {
            tracing::warn!("Kimpanel: update_property failed: {e}");
        }
        if let Err(e) = Kimpanel::enable(&self.ctxt, true).await {
            tracing::warn!("Kimpanel: enable failed: {e}");
        }
    }

    async fn set_lookup_table(
        &self,
        labels: &[&str],
        candidates: &[&str],
        attrs: &[&str],
        has_prev: bool,
        has_next: bool,
        cursor: i32,
    ) -> zbus::Result<()> {
        match self
            ._conn
            .call_method(
                Some(IMPANEL_BUS_NAME),
                IMPANEL_OBJECT_PATH,
                Some(IMPANEL2_INTERFACE),
                "SetLookupTable",
                &(
                    labels,
                    candidates,
                    attrs,
                    has_prev,
                    has_next,
                    cursor,
                    CANDIDATE_LAYOUT_NOT_SET,
                ),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("ServiceUnknown") => {
                tracing::debug!(
                    "Kimpanel impanel2 service is not available; using compositor/X11 fallback panel"
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

fn candidate_display_text(candidate: &keytao_theme::CandidateOptionModel) -> String {
    match candidate.comment.as_deref() {
        Some(comment) => format!("{} {}", candidate.text, comment),
        None => candidate.text.clone(),
    }
}

fn state_to_panel_input(state: &ImeState) -> CandidatePanelInput {
    CandidatePanelInput {
        preedit: state.preedit.clone(),
        candidates: state
            .candidates
            .iter()
            .map(|candidate| ThemeCandidate {
                text: candidate.text.clone(),
                comment: candidate.comment.clone(),
            })
            .collect(),
        highlighted_candidate_index: state.highlighted_candidate_index,
        page: state.page,
        is_last_page: state.is_last_page,
        select_keys: state.select_keys.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{mode_property, Kimpanel};

    fn kimpanel_introspection() -> String {
        let mut xml = String::new();
        zbus::object_server::Interface::introspect_to_writer(&Kimpanel, &mut xml, 0);
        xml
    }

    #[test]
    fn mode_property_carries_the_conversion_label() {
        assert!(mode_property(false).ends_with("label=中"));
        assert!(mode_property(true).ends_with("label=英"));
        assert!(mode_property(true).starts_with("/KeyTao/im:"));
    }

    #[test]
    fn kimpanel_signal_names_match_the_protocol() {
        let xml = kimpanel_introspection();
        // The visibility signal is ShowPreedit; a panel listening for the
        // protocol name never sees a signal called ShowPreeditText.
        assert!(xml.contains("<signal name=\"ShowPreedit\">"), "{xml}");
        assert!(!xml.contains("<signal name=\"ShowPreeditText\""), "{xml}");
        for name in [
            "UpdatePreeditText",
            "UpdatePreeditCaret",
            "ShowLookupTable",
            "UpdateLookupTable",
            "UpdateLookupTableCursor",
            "ShowAux",
            "UpdateAux",
            "Enable",
            "RegisterProperties",
            "UpdateProperty",
        ] {
            assert!(xml.contains(&format!("<signal name=\"{name}\">")), "{xml}");
        }
    }
}
