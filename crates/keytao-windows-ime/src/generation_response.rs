#[derive(Clone, Copy)]
pub(super) enum GenerationResponse {
    Generation(u64),
    Busy,
    Timeout,
    Disconnected,
}

impl GenerationResponse {
    pub(super) fn requires_reload(self, current_generation: u64) -> bool {
        match self {
            Self::Generation(generation) => generation != current_generation,
            Self::Busy | Self::Timeout => false,
            Self::Disconnected => true,
        }
    }
}

#[derive(Debug)]
pub(super) struct RequestError {
    pub(super) message: String,
    pub(super) written: bool,
    pub(super) timed_out: bool,
}

impl RequestError {
    pub(super) fn timeout() -> Self {
        Self {
            message: "engine pipe request timed out".into(),
            written: false,
            timed_out: true,
        }
    }

    pub(super) fn open(message: String, hresult: i32) -> Self {
        Self {
            // ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT, ERROR_TIMEOUT.
            timed_out: matches!(hresult as u32, 0x8007_00e7 | 0x8007_0079 | 0x8007_05b4),
            ..Self::from(message)
        }
    }

    pub(super) fn verification_unavailable(message: String) -> Self {
        Self {
            message,
            ..Self::timeout()
        }
    }

    pub(super) fn verification(message: String, hresult: i32) -> Self {
        // Broken/closed pipe failures still prove a disconnect during verification.
        if matches!(
            hresult as u32,
            0x8007_0006 | 0x8007_006d | 0x8007_00e8 | 0x8007_00e9
        ) {
            message.into()
        } else {
            Self::verification_unavailable(message)
        }
    }

    pub(super) fn consume_key(&self) -> bool {
        self.written && self.timed_out
    }

    pub(super) fn generation_response(&self) -> GenerationResponse {
        if self.timed_out {
            GenerationResponse::Timeout
        } else {
            GenerationResponse::Disconnected
        }
    }
}

impl From<String> for RequestError {
    fn from(message: String) -> Self {
        Self {
            message,
            written: false,
            timed_out: false,
        }
    }
}

impl From<&str> for RequestError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<RequestError> for String {
    fn from(error: RequestError) -> Self {
        error.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_timeout_and_busy_keep_the_current_session() {
        assert!(!GenerationResponse::Timeout.requires_reload(7));
        assert!(!GenerationResponse::Busy.requires_reload(7));
        assert!(!GenerationResponse::Generation(7).requires_reload(7));
    }

    #[test]
    fn disconnect_or_generation_change_requires_reload() {
        assert!(GenerationResponse::Disconnected.requires_reload(7));
        assert!(GenerationResponse::Generation(8).requires_reload(7));
    }

    #[test]
    fn transient_control_reopen_after_timeout_keeps_the_current_session() {
        for code in [231_u32, 121, 1460] {
            assert!(!RequestError::timeout()
                .generation_response()
                .requires_reload(7));
            let error = RequestError::open(
                "control pipe unavailable".into(),
                (0x8007_0000 | code) as i32,
            );
            assert!(
                !error.generation_response().requires_reload(7),
                "Win32 error {code}"
            );
            assert!(!error.consume_key());
        }
        let error = RequestError::verification_unavailable("cannot inspect server image".into());
        assert!(!error.generation_response().requires_reload(7));
        assert!(!error.consume_key());
        let error = RequestError::verification(
            "server inspection ran out of memory".into(),
            0x8007_0008_u32 as i32,
        );
        assert!(!error.generation_response().requires_reload(7));
    }

    #[test]
    fn disconnected_control_and_rejected_identity_require_reload() {
        for code in [2_u32, 109, 232, 233] {
            let error = RequestError::open(
                "control pipe disconnected".into(),
                (0x8007_0000 | code) as i32,
            );
            assert!(error.generation_response().requires_reload(7));
        }
        let error =
            RequestError::verification("server pipe disconnected".into(), 0x8007_006d_u32 as i32);
        assert!(error.generation_response().requires_reload(7));
        let error = RequestError::from("engine server image is outside the KeyTao runtime root");
        assert!(error.generation_response().requires_reload(7));
        let error = RequestError::from("engine pipe owner does not match the client user");
        assert!(error.generation_response().requires_reload(7));
    }
}
