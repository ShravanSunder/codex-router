//! Async byte access that keeps every read inside the fatal, bounded receipt boundary.
use crate::{BoundaryError, DescriptorGate, MAX_DATA_BYTES, UnixReceipt};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

type ReceiptReadFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>, BoundaryError>> + Send>>;

pub struct ReceiptByteStream {
    receipt: Arc<UnixReceipt>,
    pending_read: Option<ReceiptReadFuture>,
    read_bytes: Vec<u8>,
    read_offset: usize,
    read_eof: bool,
}

impl ReceiptByteStream {
    pub fn new(receipt: UnixReceipt) -> Self {
        Self {
            receipt: Arc::new(receipt),
            pending_read: None,
            read_bytes: Vec::new(),
            read_offset: 0,
            read_eof: false,
        }
    }

    fn copy_buffered_bytes(&mut self, read_buffer: &mut ReadBuf<'_>) -> io::Result<bool> {
        let available = self.read_bytes.len().saturating_sub(self.read_offset);
        if available == 0 || read_buffer.remaining() == 0 {
            return Ok(false);
        }
        let count = available.min(read_buffer.remaining());
        let end = self.read_offset + count;
        let Some(bytes) = self.read_bytes.get(self.read_offset..end) else {
            return Err(io::Error::other("receipt read offset exceeded its buffer"));
        };
        read_buffer.put_slice(bytes);
        self.read_offset = end;
        if self.read_offset == self.read_bytes.len() {
            self.read_bytes.clear();
            self.read_offset = 0;
        }
        Ok(true)
    }

    fn start_receipt_read(&mut self) {
        let receipt = Arc::clone(&self.receipt);
        self.pending_read = Some(Box::pin(async move {
            let mut bytes = vec![0; MAX_DATA_BYTES];
            let received = receipt
                .read(&mut bytes, false, DescriptorGate::global())
                .await?;
            bytes.truncate(received.bytes);
            Ok(bytes)
        }));
    }
}

impl AsyncRead for ReceiptByteStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        read_buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if read_buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        match this.copy_buffered_bytes(read_buffer) {
            Ok(true) => return Poll::Ready(Ok(())),
            Ok(false) => {}
            Err(error) => return Poll::Ready(Err(error)),
        }
        if this.read_eof {
            return Poll::Ready(Ok(()));
        }
        if this.pending_read.is_none() {
            this.start_receipt_read();
        }
        let result = this
            .pending_read
            .as_mut()
            .map(|read| read.as_mut().poll(context));
        match result {
            Some(Poll::Pending) => Poll::Pending,
            Some(Poll::Ready(Ok(bytes))) => {
                this.pending_read = None;
                if bytes.is_empty() {
                    this.read_eof = true;
                    return Poll::Ready(Ok(()));
                }
                this.read_bytes = bytes;
                this.read_offset = 0;
                match this.copy_buffered_bytes(read_buffer) {
                    Ok(_) => Poll::Ready(Ok(())),
                    Err(error) => Poll::Ready(Err(error)),
                }
            }
            Some(Poll::Ready(Err(error))) => {
                this.pending_read = None;
                Poll::Ready(Err(boundary_error_to_io(error)))
            }
            None => Poll::Ready(Err(io::Error::other(
                "receipt read state disappeared during polling",
            ))),
        }
    }
}

impl AsyncWrite for ReceiptByteStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        match self
            .get_mut()
            .receipt
            .as_socket()
            .poll_send_without_rights(context, bytes)
        {
            Poll::Ready(Ok(0)) => Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero))),
            result => result,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .receipt
            .as_socket()
            .shutdown_write()
            .map_or_else(
                |error| Poll::Ready(Err(boundary_error_to_io(error))),
                |_| Poll::Ready(Ok(())),
            )
    }
}

fn boundary_error_to_io(error: BoundaryError) -> io::Error {
    match error {
        BoundaryError::Io(error) => error,
        BoundaryError::WriteZero => io::Error::from(io::ErrorKind::WriteZero),
        BoundaryError::Closed => io::Error::from(io::ErrorKind::BrokenPipe),
        other => io::Error::other(other),
    }
}
