use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use previewit_broker::{
    BrokerCommand, BrokerControlContract, CommandAck, CommandId, CommandRejectionCode, ExpectedAck,
};
use previewit_protocol::v0::{
    BrokerControlRequest, BrokerControlResponse, ClosePreview, OpenPath, broker_control_request,
};

fn wire_close(command_id: &str, protocol_major: u32, protocol_minor: u32) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major,
        protocol_minor,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::ClosePreview(
            ClosePreview {},
        )),
    }
}

fn wire_open(command_id: &str, path: &Path) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le: path
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect(),
        })),
    }
}

fn accepted_open(command_id: &str, request_id: &str) -> BrokerControlResponse {
    BrokerControlResponse {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: command_id.into(),
        accepted: true,
        request_id: request_id.into(),
        error_code: String::new(),
    }
}

#[test]
fn invalid_command_id_precedes_wrong_protocol_version() {
    let request = wire_close("bad/id", 99, 99);

    let rejection = BrokerControlContract::decode_request(request).unwrap_err();

    assert_eq!(rejection.code(), CommandRejectionCode::InvalidCommandId);
    assert_eq!(rejection.safe_command_id(), None);
}

#[test]
fn valid_command_id_is_preserved_for_protocol_rejection() {
    let request = wire_close("command-1", 99, 1);

    let rejection = BrokerControlContract::decode_request(request).unwrap_err();

    assert_eq!(
        rejection.code(),
        CommandRejectionCode::ProtocolMajorMismatch
    );
    assert_eq!(rejection.safe_command_id(), Some("command-1"));
}

#[test]
fn response_must_match_the_request_command_id() {
    let expected = ExpectedAck::open(CommandId::parse("command-1").unwrap());

    let error =
        BrokerControlContract::decode_response(&expected, accepted_open("command-2", "request-1"))
            .unwrap_err();

    assert_eq!(error.code(), "response-command-mismatch");
}

#[test]
fn response_shape_must_match_acceptance_and_command_kind() {
    let expected = ExpectedAck::open(CommandId::parse("command-1").unwrap());
    let mut response = accepted_open("command-1", "request-1");
    response.error_code = "must-be-empty".into();

    let accepted_error = BrokerControlContract::decode_response(&expected, response).unwrap_err();
    assert_eq!(accepted_error.code(), "invalid-response-shape");

    let missing_request = accepted_open("command-1", "");
    let request_error =
        BrokerControlContract::decode_response(&expected, missing_request).unwrap_err();
    assert_eq!(request_error.code(), "invalid-response-shape");
}

#[test]
fn response_protocol_version_is_validated_before_payload_shape() {
    let expected = ExpectedAck::open(CommandId::parse("command-1").unwrap());
    let mut response = accepted_open("command-1", "");
    response.protocol_minor = 99;

    let error = BrokerControlContract::decode_response(&expected, response).unwrap_err();

    assert_eq!(error.code(), "response-protocol-minor-mismatch");
}

#[test]
fn valid_open_response_becomes_a_domain_ack() {
    let expected = ExpectedAck::open(CommandId::parse("command-1").unwrap());

    let ack =
        BrokerControlContract::decode_response(&expected, accepted_open("command-1", "request-1"))
            .unwrap();

    assert!(matches!(
        ack,
        CommandAck::OpenAccepted {
            command_id,
            request_id
        } if command_id.as_str() == "command-1" && request_id == "request-1"
    ));
}

#[test]
fn absolute_missing_path_is_structurally_valid() {
    let path = Path::new(r"C:\definitely-missing-previewit\file.txt");

    let command = BrokerControlContract::decode_request(wire_open("command-1", path)).unwrap();

    assert_eq!(command.command_id().as_str(), "command-1");
    assert!(matches!(command.command(), BrokerCommand::Open(actual) if actual == path));
}

#[test]
fn structural_path_and_command_errors_are_stable() {
    let mut overlong_units = vec![b'C' as u16, b':' as u16, b'\\' as u16];
    overlong_units.resize(32_768, b'a' as u16);
    let cases = [
        (
            BrokerControlRequest {
                protocol_major: 0,
                protocol_minor: 1,
                command_id: "command-odd".into(),
                command: Some(broker_control_request::Command::OpenPath(OpenPath {
                    path_utf16le: vec![1],
                })),
            },
            CommandRejectionCode::InvalidUtf16,
        ),
        (
            BrokerControlRequest {
                protocol_major: 0,
                protocol_minor: 1,
                command_id: "command-nul".into(),
                command: Some(broker_control_request::Command::OpenPath(OpenPath {
                    path_utf16le: [b'C' as u16, b':' as u16, b'\\' as u16, 0]
                        .into_iter()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                })),
            },
            CommandRejectionCode::EmbeddedNul,
        ),
        (
            wire_open("command-relative", Path::new(r"relative\file.txt")),
            CommandRejectionCode::PathNotAbsolute,
        ),
        (
            BrokerControlRequest {
                protocol_major: 0,
                protocol_minor: 1,
                command_id: "command-surrogate".into(),
                command: Some(broker_control_request::Command::OpenPath(OpenPath {
                    path_utf16le: [0xd800_u16]
                        .into_iter()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                })),
            },
            CommandRejectionCode::InvalidUtf16,
        ),
        (
            BrokerControlRequest {
                protocol_major: 0,
                protocol_minor: 1,
                command_id: "command-long".into(),
                command: Some(broker_control_request::Command::OpenPath(OpenPath {
                    path_utf16le: overlong_units
                        .into_iter()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                })),
            },
            CommandRejectionCode::PathTooLong,
        ),
        (
            BrokerControlRequest {
                protocol_major: 0,
                protocol_minor: 1,
                command_id: "command-missing".into(),
                command: None,
            },
            CommandRejectionCode::InvalidCommand,
        ),
    ];

    for (request, expected) in cases {
        assert_eq!(
            BrokerControlContract::decode_request(request)
                .unwrap_err()
                .code(),
            expected
        );
    }
}

#[test]
fn close_ack_requires_an_empty_request_id() {
    let expected = ExpectedAck::close(CommandId::parse("command-close").unwrap());
    let mut response = accepted_open("command-close", "unexpected-request");

    let error = BrokerControlContract::decode_response(&expected, response.clone()).unwrap_err();
    assert_eq!(error.code(), "invalid-response-shape");

    response.request_id.clear();
    let ack = BrokerControlContract::decode_response(&expected, response).unwrap();
    assert!(matches!(ack, CommandAck::CloseAccepted { command_id }
        if command_id.as_str() == "command-close"));
}

#[test]
fn rejected_ack_round_trips_through_the_contract() {
    let command_id = CommandId::parse("command-1").unwrap();
    let ack = CommandAck::Rejected {
        command_id: Some(command_id.clone()),
        reason: CommandRejectionCode::QueueFull,
    };

    let wire = BrokerControlContract::encode_response(&ack);
    let decoded =
        BrokerControlContract::decode_response(&ExpectedAck::open(command_id), wire).unwrap();

    assert_eq!(decoded, ack);
}

#[test]
fn rejection_without_a_safe_command_id_encodes_an_empty_id() {
    let response = BrokerControlContract::encode_response(&CommandAck::Rejected {
        command_id: None,
        reason: CommandRejectionCode::InvalidCommandId,
    });

    assert!(!response.accepted);
    assert!(response.command_id.is_empty());
    assert!(response.request_id.is_empty());
    assert_eq!(response.error_code, "invalid-command-id");
}

#[test]
fn rejected_response_requires_error_and_forbids_request_id() {
    let expected = ExpectedAck::close(CommandId::parse("command-close").unwrap());
    let invalid = BrokerControlResponse {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: "command-close".into(),
        accepted: false,
        request_id: "request-must-be-empty".into(),
        error_code: String::new(),
    };

    let error = BrokerControlContract::decode_response(&expected, invalid).unwrap_err();

    assert_eq!(error.code(), "invalid-response-shape");
}
