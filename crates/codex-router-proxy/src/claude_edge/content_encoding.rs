//! Bounded decoding of Claude response inspection copies.

use std::io;
use std::io::Cursor;
use std::pin::Pin;

use async_compression::tokio::bufread::BrotliDecoder;
use async_compression::tokio::bufread::GzipDecoder;
use async_compression::tokio::bufread::ZlibDecoder;
use async_compression::tokio::bufread::ZstdDecoder;
use bytes::Bytes;
use futures_util::stream;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

use crate::headers::HeaderCollection;

type ResponseInspectionReader = Pin<Box<dyn AsyncRead + Send>>;

/// A response content-coding supported by the Claude inspection path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResponseContentEncoding {
    /// RFC 1952 gzip stream.
    Gzip,
    /// RFC 1950 zlib-wrapped HTTP deflate stream.
    Deflate,
    /// Brotli stream.
    Brotli,
    /// Zstandard stream.
    Zstd,
}

/// Failure to decode an inspection copy of an upstream response.
#[derive(Debug, Error)]
pub(crate) enum ResponseContentDecodeError {
    /// The response selected a coding this edge does not inspect.
    #[error("response content encoding is unsupported")]
    Unsupported,
}

/// Decoded bytes and completeness available to the provider error classifier.
pub(crate) struct DecodedErrorEvidence {
    /// Bounded plaintext prefix used only for classification.
    pub(crate) prefix: Bytes,
    /// Whether the full encoded response decoded within the evidence limit.
    pub(crate) complete: bool,
}

/// Parses the response coding chain in wire order.
pub(crate) fn response_content_encodings(
    headers: &HeaderCollection,
) -> Result<Vec<ResponseContentEncoding>, ResponseContentDecodeError> {
    let mut encodings = Vec::new();
    for value in headers.values("content-encoding") {
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "" => return Err(ResponseContentDecodeError::Unsupported),
                "identity" => {}
                "gzip" => encodings.push(ResponseContentEncoding::Gzip),
                "deflate" => encodings.push(ResponseContentEncoding::Deflate),
                "br" => encodings.push(ResponseContentEncoding::Brotli),
                "zstd" => encodings.push(ResponseContentEncoding::Zstd),
                _ => return Err(ResponseContentDecodeError::Unsupported),
            }
        }
    }
    Ok(encodings)
}

/// Builds a streaming decoder over a bounded inspection channel.
pub(crate) fn decoder_for_chunks(
    receiver: mpsc::Receiver<Bytes>,
    encodings: &[ResponseContentEncoding],
) -> ResponseInspectionReader {
    let input = stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|chunk| (Ok::<_, io::Error>(chunk), receiver))
    });
    decoder_chain(Box::pin(StreamReader::new(input)), encodings)
}

/// Decodes a buffered error body without retaining more than `evidence_limit + 1` output bytes.
pub(crate) async fn decode_bounded_error_evidence(
    encoded: Bytes,
    encodings: &[ResponseContentEncoding],
    evidence_limit: usize,
    encoded_body_is_complete: bool,
) -> DecodedErrorEvidence {
    let input: ResponseInspectionReader = Box::pin(BufReader::new(Cursor::new(encoded)));
    let decoder = decoder_chain(input, encodings);
    let output_limit = evidence_limit.saturating_add(1);
    let mut limited = decoder.take(output_limit as u64);
    let mut decoded = Vec::with_capacity(output_limit.min(8 * 1024));
    let decoded_to_end = limited.read_to_end(&mut decoded).await.is_ok();
    let complete = encoded_body_is_complete && decoded_to_end && decoded.len() <= evidence_limit;
    decoded.truncate(evidence_limit);
    DecodedErrorEvidence {
        prefix: Bytes::from(decoded),
        complete,
    }
}

fn decoder_chain(
    mut reader: ResponseInspectionReader,
    encodings: &[ResponseContentEncoding],
) -> ResponseInspectionReader {
    for encoding in encodings.iter().rev() {
        let input = BufReader::new(reader);
        reader = match encoding {
            ResponseContentEncoding::Gzip => Box::pin(GzipDecoder::new(input)),
            ResponseContentEncoding::Deflate => Box::pin(ZlibDecoder::new(input)),
            ResponseContentEncoding::Brotli => Box::pin(BrotliDecoder::new(input)),
            ResponseContentEncoding::Zstd => Box::pin(ZstdDecoder::new(input)),
        };
    }
    reader
}
