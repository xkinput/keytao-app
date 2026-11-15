//! Portable configuration shared by the DLL and server.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineMode {
    InProc,
    #[default]
    Pipe,
}

impl EngineMode {
    /// Only an explicit, exactly sized REG_DWORD zero disables IPC.
    pub fn from_registry(value: Option<(u32, &[u8])>) -> Self {
        match value {
            Some((4, [0, 0, 0, 0])) => Self::InProc,
            _ => Self::Pipe,
        }
    }
}

pub fn pipe_name(sid: &str, session_id: u32) -> String {
    format!(r"\\.\pipe\KeyTao.Ime.{sid}.{session_id}.v1")
}

pub fn pipe_sddl(sid: &str) -> String {
    format!("D:(A;;GRGW;;;{sid})(A;;GRGW;;;S-1-15-2-1)S:(ML;;NW;;;LW)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_zero_selects_inproc() {
        assert_eq!(
            EngineMode::from_registry(Some((4, &[0, 0, 0, 0]))),
            EngineMode::InProc
        );
        for value in [
            None,
            Some((4, &[1, 0, 0, 0][..])),
            Some((4, &[2, 0, 0, 0][..])),
            Some((1, &[1, 0, 0, 0][..])),
            Some((4, &[1][..])),
            Some((4, &[1, 0, 0, 0, 0][..])),
            Some((4, &[255, 255, 255, 255][..])),
            Some((1, &[0, 0, 0, 0][..])),
            Some((4, &[0][..])),
        ] {
            assert_eq!(EngineMode::from_registry(value), EngineMode::Pipe);
        }
    }
}
