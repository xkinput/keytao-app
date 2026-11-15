use std::io::{self, Read, Write};

use keytao_ime_ipc::{
    negotiate_protocol, read_frame, write_frame, Request, RequestEnvelope, Response,
};

pub fn accept<T>(
    pipe: &mut (impl Read + Write),
    authenticate: impl FnOnce() -> Result<T, String>,
    server_build: &str,
) -> Result<Option<(RequestEnvelope, T)>, String> {
    let hello: RequestEnvelope = match read_frame(pipe) {
        Ok(hello) => hello,
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(format!("read client Hello: {error}")),
    };
    // Named-pipe impersonation identifies the sender of the last message read.
    let client = authenticate();
    let response = match &client {
        Err(error) => Response::HelloErr(format!("client authentication failed: {error}")),
        Ok(_) => match &hello.message {
            Request::Hello { protocol, .. } => negotiate_protocol(*protocol, server_build),
            _ => Response::HelloErr("expected Hello".into()),
        },
    };
    write_frame(pipe, &hello.reply(response.clone()))
        .map_err(|error| format!("reply to client Hello: {error}"))?;
    match response {
        Response::HelloErr(reason) => Err(reason),
        Response::HelloOk { .. } => Ok(Some((hello, client?))),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, io::Cursor, rc::Rc};

    use keytao_ime_ipc::{read_response, ClientRole, PROTOCOL_VERSION};

    use super::*;

    struct Pipe {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
        bytes_read: Rc<Cell<usize>>,
    }

    impl Pipe {
        fn request(message: Request) -> Self {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &RequestEnvelope { seq: 42, message }).unwrap();
            Self {
                input: Cursor::new(bytes),
                output: Vec::new(),
                bytes_read: Rc::new(Cell::new(0)),
            }
        }
    }

    impl Read for Pipe {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.input.read(buffer)?;
            self.bytes_read.set(self.bytes_read.get() + count);
            Ok(count)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.output.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn hello(protocol: u32) -> Request {
        Request::Hello {
            protocol,
            client_build: "client-build".into(),
            role: ClientRole::App,
            is_appcontainer: false,
        }
    }

    #[test]
    fn token_is_queried_after_reading_hello_and_rejection_preserves_sequence_and_reason() {
        let mut pipe = Pipe::request(hello(PROTOCOL_VERSION));
        let bytes_read = Rc::clone(&pipe.bytes_read);
        let frame_length = pipe.input.get_ref().len();
        let error = accept::<()>(
            &mut pipe,
            || {
                assert_eq!(bytes_read.get(), frame_length);
                Err("open pipe client token: access denied".into())
            },
            "server-build",
        )
        .unwrap_err();
        assert_eq!(
            error,
            "client authentication failed: open pipe client token: access denied"
        );
        assert!(matches!(
            read_response(&mut Cursor::new(pipe.output), 42).unwrap(),
            Response::HelloErr(reason) if reason == error
        ));
    }

    #[test]
    fn only_current_and_previous_protocols_can_finish_authenticated_handshake() {
        for protocol in [
            PROTOCOL_VERSION,
            PROTOCOL_VERSION - 1,
            PROTOCOL_VERSION - 2,
            PROTOCOL_VERSION + 1,
        ] {
            let mut pipe = Pipe::request(hello(protocol));
            let result = accept(&mut pipe, || Ok("authenticated-token"), "server-build");
            let response = read_response(&mut Cursor::new(pipe.output), 42).unwrap();
            if protocol == PROTOCOL_VERSION || protocol == PROTOCOL_VERSION - 1 {
                assert!(matches!(result, Ok(Some((_, "authenticated-token")))));
                assert!(
                    matches!(response, Response::HelloOk { protocol: accepted, .. } if accepted == protocol)
                );
            } else {
                assert!(result.is_err());
                assert!(matches!(response, Response::HelloErr(_)));
            }
        }
    }

    #[test]
    fn non_hello_frame_gets_a_rejection_reply() {
        let mut pipe = Pipe::request(Request::Deploy);
        assert_eq!(
            accept(&mut pipe, || Ok(()), "server-build").unwrap_err(),
            "expected Hello"
        );
        assert!(matches!(
            read_response(&mut Cursor::new(pipe.output), 42).unwrap(),
            Response::HelloErr(reason) if reason == "expected Hello"
        ));
    }

    #[test]
    fn partial_frame_cannot_trigger_token_authentication() {
        let mut pipe = Pipe::request(hello(PROTOCOL_VERSION));
        pipe.input.get_mut().truncate(3);
        assert!(accept::<()>(
            &mut pipe,
            || panic!("no complete message to identify"),
            "server-build"
        )
        .unwrap()
        .is_none());
        assert!(pipe.output.is_empty());
    }
}
