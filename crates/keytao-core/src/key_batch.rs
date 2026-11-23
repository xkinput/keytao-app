use crate::KeyProcessResult;

/// Run key steps in order, retaining commits and hints from key presses.
pub fn process_keys(
    keys: &[(u32, u32)],
    mut process_key: impl FnMut(u32, u32) -> KeyProcessResult,
) -> Option<KeyProcessResult> {
    let mut result: Option<KeyProcessResult> = None;
    for &(keysym, mods) in keys {
        let mut next = process_key(keysym, mods);
        if let Some(previous) = result {
            next.accepted |= previous.accepted;
            if mods & crate::RIME_RELEASE_MASK != 0
                && next.state.switch_hint.is_none()
                && next.state.schema_id == previous.state.schema_id
            {
                next.state.switch_hint = previous.state.switch_hint;
            }
            if let Some(mut committed) = previous.state.committed {
                if let Some(text) = next.state.committed.take() {
                    committed.push_str(&text);
                }
                next.state.committed = Some(committed);
            }
        }
        result = Some(next);
    }
    result
}
