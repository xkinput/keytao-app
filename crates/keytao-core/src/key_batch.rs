use crate::KeyProcessResult;

/// Run key steps in order, retaining commits before the final state snapshot.
pub fn process_keys(
    keys: &[(u32, u32)],
    mut process_key: impl FnMut(u32, u32) -> KeyProcessResult,
) -> Option<KeyProcessResult> {
    let mut result: Option<KeyProcessResult> = None;
    for &(keysym, mods) in keys {
        let mut next = process_key(keysym, mods);
        if let Some(previous) = result {
            next.accepted |= previous.accepted;
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
