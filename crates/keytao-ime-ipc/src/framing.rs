use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Serialize};

use crate::{Response, ResponseEnvelope};

pub const MAX_FRAME_SIZE: usize = 1024 * 1024;

/// Read one u32 little-endian length-prefixed JSON frame.
/// Any framing error invalidates the connection; callers must reconnect.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame exceeds 1 MiB",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Reject oversized payloads before writing any bytes to the connection.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame exceeds 1 MiB",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

/// A mismatched response is discarded. The caller must close and reconnect,
/// rather than consume later responses on an out-of-sync connection.
pub fn read_response<R: Read>(reader: &mut R, expected_seq: u64) -> io::Result<Response> {
    let response: ResponseEnvelope = read_frame(reader)?;
    if response.seq != expected_seq {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "IPC response sequence mismatch: expected {expected_seq}, received {}",
                response.seq
            ),
        ));
    }
    Ok(response.message)
}
