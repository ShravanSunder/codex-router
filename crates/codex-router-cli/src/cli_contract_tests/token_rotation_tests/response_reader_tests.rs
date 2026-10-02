use super::{declared_http_response_body_is_complete, read_http_response_with_progress};
use std::io::{Error, ErrorKind, Read};

struct ResponseThenReset {
    response_bytes: &'static [u8],
    offset: usize,
}

impl Read for ResponseThenReset {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.offset == self.response_bytes.len() {
            return Err(Error::new(ErrorKind::ConnectionReset, "fixture peer reset"));
        }
        let read_bytes = output.len().min(self.response_bytes.len() - self.offset);
        output[..read_bytes]
            .copy_from_slice(&self.response_bytes[self.offset..self.offset + read_bytes]);
        self.offset += read_bytes;
        Ok(read_bytes)
    }
}

#[test]
fn complete_zero_length_auth_response_survives_peer_reset_after_http_body() {
    let response_bytes =
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let mut reader = ResponseThenReset {
        response_bytes,
        offset: 0,
    };

    let response = read_http_response_with_progress(&mut reader)
        .expect("a reset after the complete declared HTTP body does not invalidate status");

    assert_eq!(response.as_bytes(), response_bytes);
    assert!(declared_http_response_body_is_complete(response.as_bytes()));
}

#[test]
fn incomplete_declared_response_body_still_reports_peer_reset() {
    let response_bytes = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nab";
    let mut reader = ResponseThenReset {
        response_bytes,
        offset: 0,
    };

    let error = read_http_response_with_progress(&mut reader)
        .expect_err("reset before the declared response body completes stays a failure");

    assert!(error.contains("fixture peer reset"));
    assert!(error.contains("body_complete=Some(false)"));
}

#[test]
fn incomplete_declared_response_body_still_fails_at_eof() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nab";
    let mut reader = std::io::Cursor::new(response);

    let error = read_http_response_with_progress(&mut reader)
        .expect_err("EOF before the declared body completes remains a failure");

    assert!(error.contains("expected 4 bytes, received 2"));
}
