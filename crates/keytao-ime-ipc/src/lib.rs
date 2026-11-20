//! Platform-independent messages and framing for the IME engine server.

pub mod engine_config;
mod framing;

pub use engine_config::{pipe_name, EngineMode};
pub use framing::{read_frame, read_response, write_frame, MAX_FRAME_SIZE};
use keytao_core::{DeployProgress, ImeState, InputContextPolicy, KeyProcessResult};
use serde::{Deserialize, Serialize};

/// The server accepts this protocol version and its immediate predecessor.
pub const PROTOCOL_VERSION: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientRole {
    Ime,
    App,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// These client claims do not replace server-side process/token checks.
    Hello {
        protocol: u32,
        client_build: String,
        role: ClientRole,
        is_appcontainer: bool,
    },
    /// Maps to `ImeEngineBackend::build`.
    CreateSession,
    DropSession,
    /// One entry for `process_key`, or a press/release pair for Shift.
    /// The server processes entries in order and returns the last result.
    ProcessKeys {
        keys: Vec<(u32, u32)>,
    },
    ProcessEnter,
    SelectCandidate {
        index: usize,
    },
    ChangePage {
        backward: bool,
    },
    Reset,
    GetState,
    SetInputPolicy {
        policy: InputContextPolicy,
    },
    SetEnglishMode {
        enabled: bool,
    },
    /// The client compares the generation to implement `reload_needed`.
    Generation,
    /// The App's deployment path; requires server authorization.
    Deploy,
    /// Uses the same authorization as Deploy and streams progress before Deployed.
    DeployPackages {
        plan: String,
    },
    /// A replacement server may ask the installed server to exit.
    Shutdown,
    SetContextAsciiMode {
        enabled: Option<bool>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineError {
    Busy,
    NotReady,
    Failed(String),
}

impl EngineError {
    fn into_optional<T>(self) -> Result<Option<T>, String> {
        match self {
            Self::Busy | Self::NotReady => Ok(None),
            Self::Failed(message) => Err(message),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    HelloOk {
        protocol: u32,
        server_build: String,
    },
    HelloErr(String),
    Handle {
        schema_id: String,
    },
    Ok,
    Key(Option<KeyProcessResult>),
    /// `GetState` returns `Some`; optional-state methods may return `None`.
    State(Option<ImeState>),
    English(Result<ImeState, String>),
    Generation(u64),
    DeployProgress(DeployProgress),
    Deployed(Result<(), String>),
    Err(EngineError),
}

impl Response {
    /// Preserve the backend's busy/not-ready `None` semantics for key calls.
    pub fn into_key_result(self) -> Result<Option<KeyProcessResult>, String> {
        match self {
            Self::Key(result) => Ok(result),
            Self::Err(error) => error.into_optional(),
            _ => Err("expected a key response".into()),
        }
    }

    /// Preserve the backend's busy/not-ready `None` semantics for state calls.
    pub fn into_state(self) -> Result<Option<ImeState>, String> {
        match self {
            Self::State(state) => Ok(state),
            Self::Err(error) => error.into_optional(),
            _ => Err("expected a state response".into()),
        }
    }
}

/// Negotiate the requested wire version, retaining it for older clients.
pub fn negotiate_protocol(protocol: u32, server_build: impl Into<String>) -> Response {
    if protocol == PROTOCOL_VERSION || Some(protocol) == PROTOCOL_VERSION.checked_sub(1) {
        Response::HelloOk {
            protocol,
            server_build: server_build.into(),
        }
    } else {
        Response::HelloErr(format!(
            "unsupported protocol {protocol}; expected {} or {}",
            PROTOCOL_VERSION,
            PROTOCOL_VERSION - 1
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub seq: u64,
    pub message: T,
}

pub type RequestEnvelope = Envelope<Request>;
pub type ResponseEnvelope = Envelope<Response>;

impl RequestEnvelope {
    pub fn reply(&self, message: Response) -> ResponseEnvelope {
        Envelope {
            seq: self.seq,
            message,
        }
    }
}

#[cfg(test)]
mod tests;
