use std::{error::Error, fmt, io};

#[derive(Debug)]
pub enum CoreError {
    Io(io::Error),
    Network(String),
    Parse(serde_json::Error),
    Permission(String),
    Unsupported(String),
    Other(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Network(message) => write!(f, "Network error: {message}"),
            Self::Parse(error) => write!(f, "Parse error: {error}"),
            Self::Permission(message) => write!(f, "Permission denied: {message}"),
            Self::Unsupported(message) => write!(f, "Unsupported: {message}"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for CoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Parse(error)
    }
}
