use codex_router_descriptor_boundary::{
    BoundaryError, DescriptorGate, OwnedPipe, RecordWriter, SocketReadRecord, SocketReadStream,
};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn ordered_records_end_only_on_explicit_half_close() -> TestResult {
    let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
    let mut writer = RecordWriter::new(write);
    let mut reader = SocketReadStream::new(read);
    let sending = async {
        for bytes in [b"first".to_vec(), vec![255; 16384], b"last".to_vec()] {
            writer.send(SocketReadRecord::data(bytes)?).await?;
        }
        writer.send(SocketReadRecord::ReadHalfClosed).await?;
        Ok::<_, BoundaryError>(())
    };
    let reading = async {
        for expected in [b"first".to_vec(), vec![255; 16384], b"last".to_vec()] {
            match reader.next_record().await? {
                SocketReadRecord::Data(bytes) if bytes == expected => {}
                _ => return Err(BoundaryError::InvalidRecord),
            }
        }
        if !matches!(
            reader.next_record().await?,
            SocketReadRecord::ReadHalfClosed
        ) {
            return Err(BoundaryError::InvalidRecord);
        }
        Ok::<_, BoundaryError>(())
    };
    let (sent, received) = tokio::join!(sending, reading);
    sent?;
    received?;
    Ok(())
}

#[tokio::test]
async fn abrupt_and_partial_pipe_eof_never_become_socket_half_close() -> TestResult {
    for prefix in [Vec::new(), vec![0, 0], vec![0, 0, 0, 10, b'{']] {
        let (read, write) = OwnedPipe::pair(DescriptorGate::global()).await?;
        write.write_all(&prefix).await?;
        drop(write);
        let mut reader = SocketReadStream::new(read);
        if !matches!(
            reader.next_record().await,
            Err(BoundaryError::UnexpectedEof)
        ) {
            return Err("abrupt reader EOF was reported clean".into());
        }
    }
    Ok(())
}

#[test]
fn data_record_limits_validate_before_encoding() -> TestResult {
    if SocketReadRecord::data(Vec::new()).is_ok() || SocketReadRecord::data(vec![0; 16385]).is_ok()
    {
        return Err("invalid data chunk accepted".into());
    }
    if SocketReadRecord::data(vec![0; 16384]).is_err() {
        return Err("legal chunk refused".into());
    }
    Ok(())
}
