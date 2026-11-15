use std::io::{self, Cursor, Read, Write};

use keytao_core::{Candidate, RIME_RELEASE_MASK};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use super::*;

fn state() -> ImeState {
    ImeState {
        preedit: "键🐱tao".into(),
        cursor: 2,
        sel_start: 0,
        sel_end: 2,
        candidates: vec![
            Candidate {
                text: "键道".into(),
                comment: Some("输入法 🐱".into()),
            },
            Candidate {
                text: "键盘".into(),
                comment: None,
            },
        ],
        highlighted_candidate_index: 1,
        page_size: 5,
        page: 2,
        is_last_page: false,
        committed: Some("提交🐱".into()),
        select_keys: Some("12345".into()),
        ascii_mode: false,
        schema_id: "xmjd6".into(),
        schema_name: "星猫键道".into(),
    }
}

fn key_result() -> KeyProcessResult {
    KeyProcessResult {
        state: state(),
        accepted: true,
    }
}

fn assert_round_trip<T: Serialize + DeserializeOwned>(value: &T) {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, value).unwrap();
    let decoded: T = read_frame(&mut Cursor::new(bytes)).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::to_value(value).unwrap()
    );
}

fn responses() -> Vec<Response> {
    vec![
        Response::HelloOk {
            protocol: PROTOCOL_VERSION,
            server_build: "server-build".into(),
        },
        Response::HelloErr("unsupported protocol".into()),
        Response::Handle {
            schema_id: "xmjd6".into(),
        },
        Response::Ok,
        Response::Key(Some(key_result())),
        Response::State(Some(state())),
        Response::English(Ok(state())),
        Response::Generation(u64::MAX),
        Response::Deployed(Ok(())),
        Response::Err(EngineError::Busy),
    ]
}

fn raw_frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn every_request_variant_round_trips() {
    let requests = vec![
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            client_build: "client-build".into(),
            role: ClientRole::Ime,
            is_appcontainer: false,
        },
        Request::CreateSession,
        Request::DropSession,
        Request::ProcessKeys {
            keys: vec![(0xffe1, 0), (0xffe1, RIME_RELEASE_MASK)],
        },
        Request::ProcessEnter,
        Request::SelectCandidate { index: 1 },
        Request::ChangePage { backward: true },
        Request::Reset,
        Request::GetState,
        Request::SetInputPolicy {
            policy: InputContextPolicy::sensitive(),
        },
        Request::SetEnglishMode { enabled: true },
        Request::Generation,
        Request::Deploy,
    ];
    assert_eq!(requests.len(), 13);
    for (seq, message) in requests.into_iter().enumerate() {
        assert_round_trip(&Envelope {
            seq: seq as u64,
            message,
        });
    }
}

#[test]
fn every_response_variant_round_trips() {
    let responses = responses();
    assert_eq!(responses.len(), 10);
    for (seq, message) in responses.into_iter().enumerate() {
        assert_round_trip(&Envelope {
            seq: seq as u64,
            message,
        });
    }
}

#[test]
fn hello_preserves_both_roles_and_appcontainer_flags() {
    for role in [ClientRole::Ime, ClientRole::App] {
        for is_appcontainer in [false, true] {
            assert_round_trip(&Request::Hello {
                protocol: PROTOCOL_VERSION,
                client_build: "build-🐱".into(),
                role,
                is_appcontainer,
            });
        }
    }
}

#[test]
fn frame_prefix_is_payload_byte_length_in_little_endian() {
    let message = "键🐱";
    let payload = serde_json::to_vec(message).unwrap();
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &message).unwrap();
    assert_eq!(&bytes[..4], &(payload.len() as u32).to_le_bytes());
    assert_eq!(&bytes[4..], payload);
}

#[test]
fn frame_size_limit_is_inclusive() {
    let message = "x".repeat(MAX_FRAME_SIZE - 2);
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &message).unwrap();
    assert_eq!(bytes.len(), MAX_FRAME_SIZE + 4);
    assert_eq!(&bytes[..4], &(MAX_FRAME_SIZE as u32).to_le_bytes());
    let decoded: String = read_frame(&mut Cursor::new(bytes)).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn oversized_read_is_rejected_before_reading_payload() {
    for length in [MAX_FRAME_SIZE as u32 + 1, u32::MAX] {
        let mut bytes = length.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"leave this unread");
        let mut reader = Cursor::new(bytes);
        let error = read_frame::<_, Value>(&mut reader).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(reader.position(), 4);
    }
}

#[test]
fn oversized_write_is_rejected_without_writing_any_bytes() {
    let message = "x".repeat(MAX_FRAME_SIZE - 1);
    let mut output = b"existing bytes".to_vec();
    let original = output.clone();
    let error = write_frame(&mut output, &message).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(output, original);
}

#[test]
fn eof_and_partial_frames_report_unexpected_eof() {
    let mut partial_payload = 5_u32.to_le_bytes().to_vec();
    partial_payload.extend_from_slice(b"ab");
    for bytes in [vec![], vec![1], vec![1, 0, 0], partial_payload] {
        let error = read_frame::<_, Value>(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}

#[test]
fn malformed_json_reports_invalid_data() {
    for payload in [b"".as_slice(), b"{", b"invalid", &[0xff]] {
        let error = read_frame::<_, Value>(&mut Cursor::new(raw_frame(payload))).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[test]
fn consecutive_frames_are_read_independently() {
    let first = Envelope {
        seq: 17,
        message: Request::ProcessEnter,
    };
    let second = first.reply(Response::Key(Some(key_result())));
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &first).unwrap();
    let first_length = bytes.len();
    write_frame(&mut bytes, &second).unwrap();
    let mut reader = Cursor::new(bytes);
    let request: RequestEnvelope = read_frame(&mut reader).unwrap();
    assert!(matches!(request.message, Request::ProcessEnter));
    assert_eq!(request.seq, 17);
    assert_eq!(reader.position(), first_length as u64);
    let response = read_response(&mut reader, request.seq).unwrap();
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        serde_json::to_value(second.message).unwrap()
    );
    assert_eq!(reader.position(), reader.get_ref().len() as u64);
}

#[test]
fn matching_response_sequence_returns_the_message() {
    for seq in [0, 42, u64::MAX] {
        let envelope = Envelope {
            seq,
            message: Response::Generation(123),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &envelope).unwrap();
        assert!(matches!(
            read_response(&mut Cursor::new(bytes), seq).unwrap(),
            Response::Generation(123)
        ));
    }
}

#[test]
fn sequence_mismatch_is_rejected_without_scanning_later_responses() {
    let mut bytes = Vec::new();
    write_frame(
        &mut bytes,
        &Envelope {
            seq: 3,
            message: Response::Ok,
        },
    )
    .unwrap();
    let first_length = bytes.len();
    write_frame(
        &mut bytes,
        &Envelope {
            seq: 4,
            message: Response::Ok,
        },
    )
    .unwrap();
    let mut reader = Cursor::new(bytes);
    let error = read_response(&mut reader, 4).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(reader.position(), first_length as u64);
}

#[test]
fn reply_echoes_the_request_sequence() {
    for seq in [0, 42, u64::MAX] {
        let request = Envelope {
            seq,
            message: Request::CreateSession,
        };
        let response = request.reply(Response::Handle {
            schema_id: "xmjd6".into(),
        });
        assert_eq!(response.seq, seq);
        assert!(
            matches!(response.message, Response::Handle { ref schema_id } if schema_id == "xmjd6")
        );
    }
}

#[test]
fn negotiation_accepts_current_and_previous_versions_without_upgrading() {
    for protocol in [PROTOCOL_VERSION, PROTOCOL_VERSION - 1] {
        match negotiate_protocol(protocol, "server-build") {
            Response::HelloOk {
                protocol: accepted,
                server_build,
            } => {
                assert_eq!(accepted, protocol);
                assert_eq!(server_build, "server-build");
            }
            other => panic!("expected a successful handshake, got {other:?}"),
        }
    }
}

#[test]
fn negotiation_rejects_n_minus_two_future_and_maximum_versions() {
    assert!(PROTOCOL_VERSION >= 2);
    for protocol in [PROTOCOL_VERSION - 2, PROTOCOL_VERSION + 1, u32::MAX] {
        match negotiate_protocol(protocol, "server-build") {
            Response::HelloErr(message) => assert!(!message.is_empty()),
            other => panic!("expected handshake rejection, got {other:?}"),
        }
    }
}

#[test]
fn optional_key_and_state_payloads_round_trip() {
    for response in [
        Response::Key(Some(key_result())),
        Response::Key(None),
        Response::State(Some(state())),
        Response::State(Some(ImeState::empty())),
        Response::State(None),
    ] {
        assert_round_trip(&response);
    }
}

#[test]
fn english_and_deployment_results_preserve_success_and_failure() {
    for response in [
        Response::English(Ok(state())),
        Response::English(Err("切换失败 🐱".into())),
        Response::Deployed(Ok(())),
        Response::Deployed(Err("deployment failed".into())),
    ] {
        assert_round_trip(&response);
    }
}

#[test]
fn every_engine_error_variant_round_trips() {
    for error in [
        EngineError::Busy,
        EngineError::NotReady,
        EngineError::Failed("engine failed 🐱".into()),
    ] {
        assert_round_trip(&Response::Err(error));
    }
}

#[test]
fn busy_and_not_ready_map_to_absent_key_and_state_results() {
    for error in [EngineError::Busy, EngineError::NotReady] {
        assert!(matches!(
            Response::Err(error.clone()).into_key_result(),
            Ok(None)
        ));
        assert!(matches!(Response::Err(error).into_state(), Ok(None)));
    }
}

#[test]
fn fatal_engine_errors_preserve_the_failure_message() {
    let message = "engine failed 🐱";
    assert_eq!(
        Response::Err(EngineError::Failed(message.into()))
            .into_key_result()
            .unwrap_err(),
        message
    );
    assert_eq!(
        Response::Err(EngineError::Failed(message.into()))
            .into_state()
            .unwrap_err(),
        message
    );
}

#[test]
fn unrelated_response_variants_are_type_errors() {
    for response in responses() {
        if !matches!(&response, Response::Key(_) | Response::Err(_)) {
            assert_eq!(
                response.clone().into_key_result().unwrap_err(),
                "expected a key response"
            );
        }
        if !matches!(&response, Response::State(_) | Response::Err(_)) {
            assert_eq!(
                response.into_state().unwrap_err(),
                "expected a state response"
            );
        }
    }
}

#[test]
fn successful_helpers_preserve_nonempty_and_absent_payloads() {
    for accepted in [false, true] {
        let expected = KeyProcessResult {
            state: state(),
            accepted,
        };
        let actual = Response::Key(Some(expected.clone()))
            .into_key_result()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    let expected = state();
    let actual = Response::State(Some(expected.clone()))
        .into_state()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert!(matches!(Response::Key(None).into_key_result(), Ok(None)));
    assert!(matches!(Response::State(None).into_state(), Ok(None)));
}

#[test]
fn framing_handles_short_reads_and_writes() {
    struct OneByte<T>(T);

    impl<T: Read> Read for OneByte<T> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let length = output.len().min(1);
            self.0.read(&mut output[..length])
        }
    }

    impl<T: Write> Write for OneByte<T> {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.0.write(&input[..input.len().min(1)])
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    let expected = Envelope {
        seq: 123,
        message: Response::Key(Some(key_result())),
    };
    let mut writer = OneByte(Vec::new());
    write_frame(&mut writer, &expected).unwrap();
    let mut reader = OneByte(Cursor::new(writer.0));
    let actual: ResponseEnvelope = read_frame(&mut reader).unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}
