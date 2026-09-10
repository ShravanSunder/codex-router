use communication_protocol::{ControlFrameDecoder, FrameError, MAX_CONTROL_FRAME_BYTES};
use proptest::prelude::*;
use serde_json::{Value, json};

const JSON_OBJECT_OVERHEAD_BYTES: usize = br#"{"data":""}"#.len();

fn json_object_strategy() -> impl Strategy<Value = Value> {
    (
        any::<u16>(),
        prop::collection::vec(any::<char>(), 0..24),
        prop::collection::vec(any::<i16>(), 0..8),
    )
        .prop_map(|(sequence, generated_characters, nested_values)| {
            let generated_text = format!(
                "é界{}",
                generated_characters.into_iter().collect::<String>()
            );
            json!({
                "sequence": sequence,
                "text": generated_text,
                "nested": { "values": nested_values },
            })
        })
}

fn serialized_json_lines(frames: &[Value]) -> Result<Vec<u8>, serde_json::Error> {
    let mut encoded_frames = Vec::new();
    for frame in frames {
        encoded_frames.extend(serde_json::to_vec(frame)?);
        encoded_frames.push(b'\n');
    }
    Ok(encoded_frames)
}

fn split_boundaries(encoded_frames: &[u8], generated_chunk_lengths: &[usize]) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut next_boundary = 0_usize;
    for chunk_length in generated_chunk_lengths {
        next_boundary = next_boundary
            .saturating_add(*chunk_length)
            .min(encoded_frames.len());
        boundaries.push(next_boundary);
    }

    if let Some(inside_utf8_character) = encoded_frames
        .iter()
        .position(|byte| (*byte & 0b1100_0000) == 0b1000_0000)
    {
        boundaries.push(inside_utf8_character);
    }
    boundaries.push(encoded_frames.len());
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        max_shrink_iters: 4_096,
        .. ProptestConfig::default()
    })]

    #[test]
    fn generated_json_objects_survive_arbitrary_byte_fragmentation(
        expected_frames in prop::collection::vec(json_object_strategy(), 1..12),
        generated_chunk_lengths in prop::collection::vec(1_usize..128, 0..64),
    ) {
        let encoded_frames = serialized_json_lines(&expected_frames)?;
        prop_assert!(encoded_frames.len() < MAX_CONTROL_FRAME_BYTES);

        let boundaries = split_boundaries(&encoded_frames, &generated_chunk_lengths);
        let mut decoder = ControlFrameDecoder::default();
        let mut decoded_frames = Vec::new();
        let mut chunk_start = 0;
        for chunk_end in boundaries {
            decoded_frames.extend(decoder.push(&encoded_frames[chunk_start..chunk_end])?);
            chunk_start = chunk_end;
        }

        prop_assert_eq!(decoded_frames, expected_frames);
        prop_assert_eq!(decoder.finish(), Ok(()));
        prop_assert_eq!(decoder.buffered_bytes(), 0);
    }

    #[test]
    fn invalid_generated_frame_closes_decoder_and_releases_buffer(
        buffered_prefix in prop::collection::vec(Just(b' '), 0..4_096),
        invalid_suffix in prop::collection::vec(any::<u8>(), 0..128),
    ) {
        let mut decoder = ControlFrameDecoder::default();
        prop_assert!(decoder.push(&buffered_prefix)?.is_empty());
        prop_assert_eq!(decoder.buffered_bytes(), buffered_prefix.len());

        let mut invalid_frame_end = vec![0xff];
        invalid_frame_end.extend(invalid_suffix);
        invalid_frame_end.push(b'\n');
        prop_assert_eq!(decoder.push(&invalid_frame_end), Err(FrameError::InvalidJson));
        prop_assert_eq!(decoder.buffered_bytes(), 0);
        prop_assert_eq!(decoder.push(b"{}\n"), Err(FrameError::Closed));
        prop_assert_eq!(decoder.finish(), Err(FrameError::Closed));
    }

    #[test]
    fn unterminated_generated_object_is_terminal_at_eof(frame in json_object_strategy()) {
        let encoded_frame = serde_json::to_vec(&frame)?;
        let split_at = encoded_frame.len() / 2;
        let mut decoder = ControlFrameDecoder::default();

        prop_assert!(decoder.push(&encoded_frame[..split_at])?.is_empty());
        prop_assert!(decoder.push(&encoded_frame[split_at..])?.is_empty());
        prop_assert_eq!(decoder.finish(), Err(FrameError::Truncated));
        prop_assert_eq!(decoder.buffered_bytes(), 0);
        prop_assert_eq!(decoder.push(b"{}\n"), Err(FrameError::Closed));
    }
}

fn frame_with_exact_byte_length(frame_bytes: usize) -> Vec<u8> {
    assert!(frame_bytes >= JSON_OBJECT_OVERHEAD_BYTES);
    let payload_bytes = frame_bytes - JSON_OBJECT_OVERHEAD_BYTES;
    format!(r#"{{"data":"{}"}}"#, "x".repeat(payload_bytes)).into_bytes()
}

#[test]
fn exact_frame_size_boundaries_are_enforced() {
    for accepted_size in [MAX_CONTROL_FRAME_BYTES - 1, MAX_CONTROL_FRAME_BYTES] {
        let mut decoder = ControlFrameDecoder::default();
        let mut frame = frame_with_exact_byte_length(accepted_size);
        frame.push(b'\n');

        let decoded = decoder.push(&frame);
        assert_eq!(decoded.map(|frames| frames.len()), Ok(1));
        assert_eq!(decoder.finish(), Ok(()));
    }

    let mut decoder = ControlFrameDecoder::default();
    let oversized_frame = frame_with_exact_byte_length(MAX_CONTROL_FRAME_BYTES + 1);
    assert_eq!(decoder.push(&oversized_frame), Err(FrameError::TooLarge));
    assert_eq!(decoder.buffered_bytes(), 0);
    assert_eq!(decoder.push(b"{}\n"), Err(FrameError::Closed));
}
