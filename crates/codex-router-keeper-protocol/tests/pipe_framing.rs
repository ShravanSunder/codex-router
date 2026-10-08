use codex_router_descriptor_boundary::{BoundaryError, DescriptorGate, OwnedPipe};
use codex_router_keeper_protocol::{
    JsonMessage, MAX_FRAME_BYTES, PipeFrameReader, PipeFrameWriter,
};
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[tokio::test]
async fn exact_encoded_megabyte_and_second_frame_round_trip_through_real_pipe() -> TestResult {
    let mut encoded = vec![b'x'; MAX_FRAME_BYTES];
    *encoded.first_mut().ok_or("empty bound")? = b'"';
    *encoded.last_mut().ok_or("empty bound")? = b'"';
    let message = JsonMessage::from_bytes(encoded)?;
    let second = JsonMessage::encode(&serde_json::json!({"next":true}))?;
    let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
    let mut writer = PipeFrameWriter::new(write);
    let mut reader = PipeFrameReader::new(read);
    let sending = async {
        writer.send(&message).await?;
        writer.send(&second).await?;
        Ok::<_, BoundaryError>(())
    };
    let receiving = async {
        for expected in [&message, &second] {
            let actual = reader
                .receive()
                .await?
                .ok_or(BoundaryError::UnexpectedEof)?;
            if actual.bytes() != expected.bytes() {
                return Err(BoundaryError::InvalidRecord);
            }
        }
        Ok::<_, BoundaryError>(())
    };
    let (sent, received) = timeout(Duration::from_secs(3), async {
        tokio::join!(sending, receiving)
    })
    .await?;
    sent?;
    received?;
    if JsonMessage::encode(&"x".repeat(MAX_FRAME_BYTES - 1)).is_ok() {
        return Err("one-byte-over encodedJSON accepted".into());
    }
    Ok(())
}
#[tokio::test]
async fn fragmented_and_coalesced_pipe_frames_preserve_complete_encoding() -> TestResult {
    let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
    let mut reader = PipeFrameReader::new(read);
    let sending = async {
        for byte in b"\0\0\0\x02{}" {
            write.write_all(&[*byte]).await?;
            tokio::task::yield_now().await;
        }
        write.write_all(b"\0\0\0\x04null\0\0\0\x02[]").await?;
        Ok::<_, BoundaryError>(())
    };
    let receiving = async {
        for expected in [b"{}".as_slice(), b"null", b"[]"] {
            let actual = reader
                .receive()
                .await?
                .ok_or(BoundaryError::UnexpectedEof)?;
            if actual.bytes() != expected {
                return Err(BoundaryError::InvalidRecord);
            }
        }
        Ok::<_, BoundaryError>(())
    };
    let (sent, received) = tokio::join!(sending, receiving);
    sent?;
    received?;
    Ok(())
}
#[tokio::test]
async fn invalid_or_partial_frame_never_reuses_later_clean_input() -> TestResult {
    for bytes in [
        vec![0, 0],
        vec![0, 0, 0, 2, b'!'],
        b"\0\0\0\x02!x\0\0\0\x02{}".to_vec(),
        u32::try_from(MAX_FRAME_BYTES + 1)?.to_be_bytes().to_vec(),
    ] {
        let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
        write.write_all(&bytes).await?;
        drop(write);
        let mut reader = PipeFrameReader::new(read);
        if reader.receive().await.is_ok() {
            return Err("malformed or partial framing accepted".into());
        }
        if !matches!(reader.receive().await, Err(BoundaryError::Closed)) {
            return Err("failed frame began another clean frame".into());
        }
    }
    Ok(())
}
#[tokio::test]
async fn drop_partial_read_closes_direction_and_cannot_resume() -> TestResult {
    let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
    write.write_all(b"\0\0").await?;
    let mut reader = PipeFrameReader::new(read);
    let mut receiving = Box::pin(reader.receive());
    tokio::select! {biased;_result=&mut receiving=>return Err("partialprefix settled".into()),()=tokio::task::yield_now()=>{}}
    drop(receiving);
    if !matches!(reader.receive().await, Err(BoundaryError::Closed)) {
        return Err("cancelled frame resumed".into());
    }
    Ok(())
}
#[tokio::test]
async fn clean_pipe_eof_is_distinct_from_missing_result_and_shape_is_not_domain_admission()
-> TestResult {
    let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
    drop(write);
    if PipeFrameReader::new(read).receive().await?.is_some() {
        return Err("clean EOF must be absent frame".into());
    }
    let message = JsonMessage::from_bytes(b"{\"value\":7}".to_vec())?;
    let shape: serde_json::Value = message.decode()?;
    if shape.get("value") != Some(&serde_json::json!(7)) {
        return Err("JSON shape decode changed input".into());
    }
    Ok(())
}
