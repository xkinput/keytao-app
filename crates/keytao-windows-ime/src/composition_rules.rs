pub(crate) fn composition_end_to_finish_first(
    pending: impl IntoIterator<Item = (usize, bool)>,
    context: usize,
) -> Option<usize> {
    pending
        .into_iter()
        .position(|(pending_context, queued)| pending_context == context && queued)
}

pub(crate) fn may_terminate_composition(owns_preedit: bool, cleared: bool) -> bool {
    !owns_preedit || cleared
}

pub(crate) fn leftover_to_delete_on_host_termination(
    range_text: &str,
    last_preedit: &str,
    committed: bool,
) -> bool {
    !committed && !last_preedit.is_empty() && range_text == last_preedit
}

pub(crate) fn may_process_key(
    pending_apply: bool,
    pending_ends: impl IntoIterator<Item = usize>,
    context: usize,
) -> bool {
    !pending_apply && !pending_ends.into_iter().any(|pending| pending == context)
}

pub(crate) fn should_drop_pending_context(
    document_attached: Option<bool>,
    session_disconnected: bool,
) -> bool {
    document_attached == Some(false) || session_disconnected
}

pub(crate) fn should_retry_commit(written: bool) -> bool {
    !written
}
