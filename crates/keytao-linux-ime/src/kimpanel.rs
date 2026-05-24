use keytao_core::{Candidate, ImeState};
use zbus::{connection, interface, object_server::SignalContext, Connection};

const KIMPANEL_BUS_NAME: &str = "org.kde.kimpanel.inputmethod";
const KIMPANEL_OBJECT_PATH: &str = "/kimpanel";
const IMPANEL_BUS_NAME: &str = "org.kde.impanel";
const IMPANEL_OBJECT_PATH: &str = "/org/kde/impanel";
const IMPANEL2_INTERFACE: &str = "org.kde.impanel2";
const CANDIDATE_LAYOUT_NOT_SET: i32 = 0;

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

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

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
            tracing::warn!("Kimpanel: failed to request Kimpanel name (running as secondary?): {e}");
        }
        let ctxt = match SignalContext::new(&conn, KIMPANEL_OBJECT_PATH) {
            Ok(ctxt) => ctxt,
            Err(e) => {
                tracing::warn!("Kimpanel: failed to create signal context: {e}");
                return None;
            }
        };
        tracing::info!("Kimpanel D-Bus panel ready");
        let handle = Self { ctxt, _conn: conn };
        handle.register_status().await;
        Some(handle)
    }

    pub async fn clear(&self) {
        let _ = self.set_lookup_table(&[], &[], &[], false, false, -1).await;
        let _ = Kimpanel::update_aux(&self.ctxt, "", "").await;
        let _ = Kimpanel::show_aux(&self.ctxt, false).await;
        let _ = Kimpanel::update_preedit_text(&self.ctxt, "", "").await;
        let _ = Kimpanel::update_preedit_caret(&self.ctxt, 0).await;
        let _ = Kimpanel::show_preedit_text(&self.ctxt, false).await;
        let _ = Kimpanel::show_lookup_table(&self.ctxt, false).await;
    }

    pub async fn update_state(&self, state: &ImeState) {
        tracing::info!("Kimpanel: updating state, preedit={}, candidates_len={}", state.preedit, state.candidates.len());
        if state.preedit.is_empty() {
            if let Err(e) = Kimpanel::show_preedit_text(&self.ctxt, false).await {
                tracing::warn!("Kimpanel: show_preedit_text(false) failed: {e}");
            }
        } else {
            if let Err(e) = Kimpanel::update_preedit_text(&self.ctxt, &state.preedit, "").await {
                tracing::warn!("Kimpanel: update_preedit_text failed: {e}");
            }
            let caret = state.preedit.chars().count() as i32;
            if let Err(e) = Kimpanel::update_preedit_caret(&self.ctxt, caret).await {
                tracing::warn!("Kimpanel: update_preedit_caret failed: {e}");
            }
            if let Err(e) = Kimpanel::show_preedit_text(&self.ctxt, true).await {
                tracing::warn!("Kimpanel: show_preedit_text(true) failed: {e}");
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

        let select_keys = state.select_keys.as_deref();
        let labels: Vec<String> = state
            .candidates
            .iter()
            .enumerate()
            .map(|(index, _)| candidate_label(index, select_keys))
            .collect();
        let candidates: Vec<String> = state
            .candidates
            .iter()
            .map(candidate_display_text)
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let attrs: Vec<&str> = Vec::new();
        let cursor = state
            .highlighted_candidate_index
            .min(state.candidates.len().saturating_sub(1)) as i32;

        if let Err(e) = self
            .set_lookup_table(
                &label_refs,
                &candidate_refs,
                &attrs,
                state.page > 0,
                !state.is_last_page,
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
            state.page > 0,
            !state.is_last_page,
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

    async fn register_status(&self) {
        let status = "/KeyTao/im:KeyTao:input-keyboard:KeyTao:menu,label=键";
        let props = [status];
        if let Err(e) = Kimpanel::register_properties(&self.ctxt, &props).await {
            tracing::warn!("Kimpanel: register_properties failed: {e}");
        }
        if let Err(e) = Kimpanel::update_property(&self.ctxt, status).await {
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
        self._conn
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
            .map(|_| ())
    }
}

fn candidate_display_text(candidate: &Candidate) -> String {
    match candidate
        .comment
        .as_deref()
        .filter(|comment| !comment.is_empty())
    {
        Some(comment) => format!("{} {}", candidate.text, comment),
        None => candidate.text.clone(),
    }
}

fn candidate_label(index: usize, select_keys: Option<&str>) -> String {
    select_keys
        .and_then(|keys| keys.chars().nth(index))
        .or_else(|| "1234567890".chars().nth(index))
        .map(|ch| ch.to_string())
        .unwrap_or_else(|| (index + 1).to_string())
}
