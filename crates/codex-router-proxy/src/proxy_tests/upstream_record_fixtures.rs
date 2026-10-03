use super::*;

#[derive(Default)]
pub(super) struct RecordingAuditFailureReporter {
    pub(super) diagnostics: RefCell<Vec<String>>,
}

impl AuditFailureReporter for RecordingAuditFailureReporter {
    fn report_audit_failure(&self, diagnostic: &str) {
        self.diagnostics.borrow_mut().push(diagnostic.to_owned());
    }
}

pub(super) struct RecordingUpstream {
    response: HttpProxyResponse,
    recorded: RefCell<Vec<UpstreamHttpRequest>>,
}

impl RecordingUpstream {
    pub(super) fn new(response: HttpProxyResponse) -> Self {
        Self {
            response,
            recorded: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<UpstreamHttpRequest> {
        self.recorded.take()
    }
}

impl UpstreamHttpTransport for RecordingUpstream {
    fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError> {
        self.recorded.borrow_mut().push(request);
        Ok(self.response.clone())
    }
}

impl StreamingUpstreamHttpTransport for RecordingUpstream {
    fn send_streaming(
        &self,
        request: UpstreamHttpRequest,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
        self.recorded.borrow_mut().push(request);
        Ok(StreamingHttpProxyResponse::from_buffered(
            self.response.clone(),
        ))
    }
}

pub(super) struct ChannelUpstream {
    request_sender: mpsc::Sender<UpstreamHttpRequest>,
    response: HttpProxyResponse,
}

impl ChannelUpstream {
    pub(super) fn new(
        request_sender: mpsc::Sender<UpstreamHttpRequest>,
        response: HttpProxyResponse,
    ) -> Self {
        Self {
            request_sender,
            response,
        }
    }
}

impl UpstreamHttpTransport for ChannelUpstream {
    fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError> {
        if let Err(error) = self.request_sender.send(request) {
            return Err(HttpProxyError::Upstream {
                message: format!("recording channel closed: {error}"),
            });
        }

        Ok(self.response.clone())
    }
}
