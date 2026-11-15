//! Portable configuration shared by the DLL and server.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineMode {
    #[default]
    InProc,
    Pipe,
}

impl EngineMode {
    /// Only an exactly sized REG_DWORD containing one enables IPC.
    pub fn from_registry(value: Option<(u32, &[u8])>) -> Self {
        match value {
            Some((4, [1, 0, 0, 0])) => Self::Pipe,
            _ => Self::InProc,
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
    fn mode_fails_closed() {
        assert_eq!(
            EngineMode::from_registry(Some((4, &[1, 0, 0, 0]))),
            EngineMode::Pipe
        );
        for value in [
            None,
            Some((4, &[0, 0, 0, 0][..])),
            Some((4, &[2, 0, 0, 0][..])),
            Some((1, &[1, 0, 0, 0][..])),
            Some((4, &[1][..])),
            Some((4, &[1, 0, 0, 0, 0][..])),
            Some((4, &[255, 255, 255, 255][..])),
        ] {
            assert_eq!(EngineMode::from_registry(value), EngineMode::InProc);
        }
    }
}
