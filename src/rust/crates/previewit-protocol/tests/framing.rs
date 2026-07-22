use previewit_protocol::{MAX_CONTROL_FRAME, decode_frame, encode_frame};

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
