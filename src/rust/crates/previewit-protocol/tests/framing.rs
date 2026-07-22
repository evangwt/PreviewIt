use previewit_protocol::{
    MAX_CONTROL_FRAME, decode_frame, encode_frame,
    v0::{Envelope, Hello, envelope},
};
use prost::Message;

#[test]
fn frame_round_trips() {
    let payload = b"previewit";
    let frame = encode_frame(payload).unwrap();
    assert_eq!(decode_frame(&frame).unwrap(), payload);
}

#[test]
fn oversized_control_frame_is_rejected() {
    let payload = vec![0_u8; MAX_CONTROL_FRAME + 1];
    assert!(encode_frame(&payload).is_err());
}

#[test]
fn truncated_frame_is_rejected() {
    assert!(decode_frame(&[4, 0, 0, 0, 1, 2]).is_err());
}

#[test]
fn protobuf_envelope_round_trips() {
    let envelope = Envelope {
        protocol_major: 0,
        protocol_minor: 1,
        request_id: "request-1".into(),
        payload: Some(envelope::Payload::Hello(Hello {
            component_id: "dotnet-probe".into(),
            capabilities: vec!["read-handle-v0".into()],
        })),
    };

    let encoded = envelope.encode_to_vec();
    let parsed = Envelope::decode(encoded.as_slice()).unwrap();

    assert_eq!(parsed.protocol_major, 0);
    assert_eq!(parsed.protocol_minor, 1);
    assert_eq!(parsed.request_id, "request-1");
    let Some(envelope::Payload::Hello(hello)) = parsed.payload else {
        panic!("expected Hello payload");
    };
    assert_eq!(hello.component_id, "dotnet-probe");
    assert_eq!(hello.capabilities, ["read-handle-v0"]);
}

#[test]
fn protobuf_broker_control_round_trips() {
    use previewit_protocol::v0::{
        BrokerControlRequest, BrokerControlResponse, OpenPath, broker_control_request,
    };

    let path = r"C:\fixtures\preview.txt";
    let path_utf16le: Vec<u8> = path.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let request = BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: "command-1".into(),
        command: Some(broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le: path_utf16le.clone(),
        })),
    };

    let parsed = BrokerControlRequest::decode(request.encode_to_vec().as_slice()).unwrap();
    assert_eq!(parsed.protocol_major, 0);
    assert_eq!(parsed.protocol_minor, 1);
    assert_eq!(parsed.command_id, "command-1");
    let Some(broker_control_request::Command::OpenPath(open_path)) = parsed.command else {
        panic!("expected OpenPath command");
    };
    assert_eq!(open_path.path_utf16le, path_utf16le);

    let response = BrokerControlResponse {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: "command-1".into(),
        accepted: true,
        request_id: "request-1".into(),
        error_code: String::new(),
    };
    let parsed = BrokerControlResponse::decode(response.encode_to_vec().as_slice()).unwrap();
    assert!(parsed.accepted);
    assert_eq!(parsed.request_id, "request-1");
    assert!(parsed.error_code.is_empty());
}
