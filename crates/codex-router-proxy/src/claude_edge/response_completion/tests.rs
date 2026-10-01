use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::StreamBody;
use hyper::body::Frame;

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
