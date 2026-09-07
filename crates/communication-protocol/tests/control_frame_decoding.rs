use communication_protocol::{ControlFrameDecoder, FrameError, MAX_CONTROL_FRAME_BYTES};

#[test]
fn fragmented_and_coalesced_frames_preserve_object_boundaries() {
    // Arrange
    let mut decoder = ControlFrameDecoder::default();
    // Act
    assert!(
        decoder
            .push(b"{\"value\":")
            .unwrap_or_else(|error| panic!("fragment: {error}"))
            .is_empty()
    );
    let frames = decoder
        .push(b"1}\n{\"value\":2}\n")
        .unwrap_or_else(|error| panic!("frames: {error}"));
    // Assert
    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames
            .first()
            .and_then(|frame| frame.get("value"))
            .and_then(serde_json::Value::as_u64),
        Some(1)
    );
    assert!(decoder.finish().is_ok());
}

#[test]
fn batches_invalid_utf8_and_truncated_input_are_rejected() {
    for input in [b"[]\n".as_slice(), b"null\n", b"\xff\n", b"{bad}\n"] {
        let mut decoder = ControlFrameDecoder::default();
        assert!(decoder.push(input).is_err());
        assert_eq!(decoder.push(b"{}\n"), Err(FrameError::Closed));
    }
    let mut decoder = ControlFrameDecoder::default();
    assert!(decoder.push(b"{}").is_ok());
    assert_eq!(decoder.finish(), Err(FrameError::Truncated));
}

#[test]
fn frame_budget_is_enforced_before_unbounded_buffer_growth() {
    let mut decoder = ControlFrameDecoder::default();
    let input = vec![b' '; MAX_CONTROL_FRAME_BYTES + 1];
    assert_eq!(decoder.push(&input), Err(FrameError::TooLarge));
    assert_eq!(decoder.buffered_bytes(), 0);
}
