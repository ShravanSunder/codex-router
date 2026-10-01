use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::StreamBody;
use hyper::body::Frame;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use super::ClaudeResponseCompletion;
use super::observe_completion;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;

fn response(
    sse: bool,
    frames: Vec<Result<Frame<Bytes>, AsyncHttpBodyError>>,
) -> AsyncStreamingHttpProxyResponse {
    AsyncStreamingHttpProxyResponse::new(
        200,
        HeaderCollection::new(vec![Header::new(
            "content-type",
            if sse {
                "text/event-stream; charset=utf-8"
            } else {
                "application/json"
            },
        )]),
        StreamBody::new(futures_util::stream::iter(frames)).boxed(),
    )
}

#[tokio::test]
async fn claude_completion_waits_for_complete_non_stream_body() {
    let original = AsyncStreamingHttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        Full::new(Bytes::from_static(b"message"))
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    );
    let (response, mut completion) = observe_completion(original);
    assert!(matches!(
        completion.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    let (_, _, body) = response.into_parts();
    assert_eq!(
        body.collect()
            .await
            .unwrap_or_else(|error| panic!("body: {error}"))
            .to_bytes(),
        Bytes::from_static(b"message")
    );
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::Success
    );
}

#[tokio::test]
async fn claude_completion_recognizes_chunk_split_message_stop_and_preserves_bytes() {
    let chunks = [
        b"event: message_st".as_slice(),
        b"op\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n".as_slice(),
    ];
    let frames = chunks
        .iter()
        .map(|bytes| Ok(Frame::data(Bytes::copy_from_slice(bytes))))
        .collect();
    let (response, completion) = observe_completion(response(true, frames));
    let (_, _, body) = response.into_parts();
    let bytes = body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("body: {error}"))
        .to_bytes();
    assert_eq!(bytes.as_ref(), chunks.concat());
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::Success
    );
}

#[tokio::test]
async fn claude_completion_recognizes_message_stop_at_eof_without_trailing_blank_line() {
    let bytes = Bytes::from_static(b"event: message_stop\ndata: {\"type\":\"message_stop\"}");
    let (response, completion) =
        observe_completion(response(true, vec![Ok(Frame::data(bytes.clone()))]));
    let (_, _, body) = response.into_parts();

    assert_eq!(
        body.collect()
            .await
            .unwrap_or_else(|error| panic!("body: {error}"))
            .to_bytes(),
        bytes
    );
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::Success
    );
}

#[tokio::test]
async fn claude_completion_sse_cutoff_without_message_stop_is_not_success() {
    let frames = vec![Ok(Frame::data(Bytes::from_static(
        b"event: content_block_delta\ndata: {}\n\n",
    )))];
    let (response, completion) = observe_completion(response(true, frames));
    let (_, _, body) = response.into_parts();
    let _collected = body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::Incomplete
    );
}

#[tokio::test]
async fn claude_completion_error_event_disqualifies_message_stop() {
    let frames = vec![Ok(Frame::data(Bytes::from_static(b"event: error\ndata: {\"type\":\"error\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")))];
    let (response, completion) = observe_completion(response(true, frames));
    let (_, _, body) = response.into_parts();
    let _collected = body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::ProviderError
    );
}

#[tokio::test]
async fn claude_completion_body_failure_is_not_success() {
    let frames = vec![
        Ok(Frame::data(Bytes::from_static(b"partial"))),
        Err(Box::new(std::io::Error::other("upstream cutoff")) as AsyncHttpBodyError),
    ];
    let (response, completion) = observe_completion(response(false, frames));
    let (_, _, body) = response.into_parts();
    assert!(body.collect().await.is_err());
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::Incomplete
    );
}

#[tokio::test]
async fn claude_completion_dropped_body_is_not_success() {
    let (response, completion) = observe_completion(response(
        false,
        vec![Ok(Frame::data(Bytes::from_static(b"unread")))],
    ));
    drop(response);
    assert_eq!(
        completion
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        ClaudeResponseCompletion::Incomplete
    );
}

#[tokio::test]
async fn claude_completion_fixed_length_body_finishes_on_final_frame_without_eof_poll() {
    let original = AsyncStreamingHttpProxyResponse::new(
        200,
        HeaderCollection::new(vec![Header::new("content-length", "7")]),
        Full::new(Bytes::from_static(b"message"))
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    );
    let (response, mut completion) = observe_completion(original);
    let (_, headers, mut body) = response.into_parts();
    let frame = body
        .frame()
        .await
        .expect("final frame")
        .expect("successful frame");
    assert_eq!(frame.data_ref(), Some(&Bytes::from_static(b"message")));
    assert_eq!(headers.value("content-length"), Some("7"));
    assert_eq!(
        completion.try_recv().expect("completion after final frame"),
        ClaudeResponseCompletion::Success
    );
    drop(body);
}

#[tokio::test]
async fn claude_completion_fixed_length_sse_finishes_on_message_stop_frame_without_eof_poll() {
    let bytes = Bytes::from_static(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    let original = AsyncStreamingHttpProxyResponse::new(
        200,
        HeaderCollection::new(vec![
            Header::new("content-type", "text/event-stream"),
            Header::new("content-length", bytes.len().to_string()),
        ]),
        Full::new(bytes.clone())
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    );
    let (response, mut completion) = observe_completion(original);
    let (_, _, mut body) = response.into_parts();
    let frame = body
        .frame()
        .await
        .expect("final frame")
        .expect("successful frame");
    assert_eq!(frame.data_ref(), Some(&bytes));
    assert_eq!(
        completion.try_recv().expect("completion after final frame"),
        ClaudeResponseCompletion::Success
    );
    drop(body);
}

#[tokio::test]
async fn claude_completion_tracks_compressed_sse_and_forwards_original_bytes() {
    let plaintext =
        Bytes::from_static(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    for encoding in ["gzip", "deflate", "br", "zstd"] {
        let compressed = compress_body(encoding, plaintext.clone()).await;
        let (response, completion) = observe_completion(AsyncStreamingHttpProxyResponse::new(
            200,
            HeaderCollection::new(vec![
                Header::new("content-type", "text/event-stream"),
                Header::new("content-encoding", encoding),
                Header::new("content-length", compressed.len().to_string()),
            ]),
            Full::new(compressed.clone())
                .map_err(|never| -> AsyncHttpBodyError { match never {} })
                .boxed(),
        ));
        let (status, headers, body) = response.into_parts();

        assert_eq!(status, 200);
        assert_eq!(headers.value("content-encoding"), Some(encoding));
        assert_eq!(
            body.collect()
                .await
                .unwrap_or_else(|error| panic!("body: {error}"))
                .to_bytes(),
            compressed
        );
        assert_eq!(
            completion
                .await
                .unwrap_or_else(|error| panic!("completion: {error}")),
            ClaudeResponseCompletion::Success,
            "{encoding} response completion"
        );
    }
}

async fn compress_body(encoding: &str, plaintext: Bytes) -> Bytes {
    let (writer, mut reader) = tokio::io::duplex(64);
    let encoding = encoding.to_owned();
    let encoder_task = tokio::spawn(async move {
        match encoding.as_str() {
            "gzip" => {
                let mut encoder = async_compression::tokio::write::GzipEncoder::new(writer);
                encoder.write_all(&plaintext).await?;
                encoder.shutdown().await
            }
            "deflate" => {
                let mut encoder = async_compression::tokio::write::ZlibEncoder::new(writer);
                encoder.write_all(&plaintext).await?;
                encoder.shutdown().await
            }
            "br" => {
                let mut encoder = async_compression::tokio::write::BrotliEncoder::new(writer);
                encoder.write_all(&plaintext).await?;
                encoder.shutdown().await
            }
            "zstd" => {
                let mut encoder = async_compression::tokio::write::ZstdEncoder::new(writer);
                encoder.write_all(&plaintext).await?;
                encoder.shutdown().await
            }
            _ => Err(std::io::Error::other("unsupported test encoding")),
        }
    });
    let mut compressed = Vec::new();
    reader
        .read_to_end(&mut compressed)
        .await
        .unwrap_or_else(|error| panic!("compressed test body: {error}"));
    encoder_task
        .await
        .unwrap_or_else(|error| panic!("encoder task: {error}"))
        .unwrap_or_else(|error| panic!("encode test body: {error}"));
    Bytes::from(compressed)
}
