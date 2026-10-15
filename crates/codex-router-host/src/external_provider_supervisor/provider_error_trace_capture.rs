//! Strict safe-event capture for synthetic tests; no raw fields or arbitrary strings retained.
use std::{
    fmt,
    fmt::Write as _,
    sync::{Arc, Mutex},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

pub(super) const PROVIDER_ERROR_TARGET: &str = "acp_client_runtime::provider_diagnostics";
pub(super) const PROVIDER_ERROR_EVENT: &str = "codex_router.acp.error_diagnostic";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClosedProviderDiagnostic {
    pub(super) provider_code: i64,
    pub(super) reported_error_kind: &'static str,
    pub(super) correlation_id: uuid::Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiagnosticField {
    EventName,
    Code,
    Category,
    Correlation,
    Message,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TraceCaptureError {
    UnexpectedField,
    UnexpectedLevel,
    InvalidField(DiagnosticField),
    MissingField(DiagnosticField),
    UnexpectedEventCount,
    LockPoisoned,
}
impl fmt::Display for TraceCaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedField => {
                formatter.write_str("unexpected new provider diagnostic field")
            }
            Self::UnexpectedLevel => formatter.write_str("safe provider diagnostic was not DEBUG"),
            Self::InvalidField(field) => write!(
                formatter,
                "invalid closed provider diagnostic field: {field:?}"
            ),
            Self::MissingField(field) => write!(
                formatter,
                "missing closed provider diagnostic field: {field:?}"
            ),
            Self::UnexpectedEventCount => {
                formatter.write_str("expected exactly one provider diagnostic event")
            }
            Self::LockPoisoned => {
                formatter.write_str("structured provider diagnostic capture lock poisoned")
            }
        }
    }
}
impl std::error::Error for TraceCaptureError {}

type CapturedDiagnostic = Result<ClosedProviderDiagnostic, TraceCaptureError>;

#[derive(Clone)]
pub(super) struct StructuredProviderTrace {
    expected_kind: &'static str,
    events: Arc<Mutex<Vec<CapturedDiagnostic>>>,
}
impl StructuredProviderTrace {
    pub(super) fn new(expected_kind: &'static str) -> Self {
        Self {
            expected_kind,
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub(super) fn single(&self) -> CapturedDiagnostic {
        let events = self
            .events
            .lock()
            .map_err(|_| TraceCaptureError::LockPoisoned)?;
        match events.as_slice() {
            [event] => event.clone(),
            _ => Err(TraceCaptureError::UnexpectedEventCount),
        }
    }
    pub(super) fn is_empty(&self) -> Result<bool, TraceCaptureError> {
        Ok(self
            .events
            .lock()
            .map_err(|_| TraceCaptureError::LockPoisoned)?
            .is_empty())
    }
}
impl<TSubscriber: Subscriber> Layer<TSubscriber> for StructuredProviderTrace {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, TSubscriber>) {
        if event.metadata().target() != PROVIDER_ERROR_TARGET {
            return;
        }
        let mut selector = EventNameSelector::default();
        event.record(&mut selector);
        if !selector.matches {
            return;
        }
        let mut fields = DiagnosticFields::new(self.expected_kind);
        if *event.metadata().level() != tracing::Level::DEBUG {
            fields.error = Some(TraceCaptureError::UnexpectedLevel);
        }
        for field in event.metadata().fields() {
            if !matches!(
                field.name(),
                "event.name"
                    | "provider_code"
                    | "reported_error_kind"
                    | "correlation_id"
                    | "message"
            ) {
                fields.error = Some(TraceCaptureError::UnexpectedField);
            }
        }
        event.record(&mut fields);
        if let Ok(mut events) = self.events.lock() {
            events.push(fields.finish());
        }
    }
}

#[derive(Default)]
struct EventNameSelector {
    matches: bool,
}
impl Visit for EventNameSelector {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "event.name" {
            self.matches = value == PROVIDER_ERROR_EVENT;
        }
    }
    fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {}
}

enum CapturedField<TValue> {
    Missing,
    Value(TValue),
    Invalid,
}
impl<TValue> CapturedField<TValue> {
    fn record(&mut self, value: Option<TValue>) {
        *self = match (matches!(self, Self::Missing), value) {
            (true, Some(value)) => Self::Value(value),
            _ => Self::Invalid,
        };
    }
    fn require(self, field: DiagnosticField) -> Result<TValue, TraceCaptureError> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Missing => Err(TraceCaptureError::MissingField(field)),
            Self::Invalid => Err(TraceCaptureError::InvalidField(field)),
        }
    }
}

struct DiagnosticFields {
    expected_kind: &'static str,
    event_name: CapturedField<()>,
    code: CapturedField<i64>,
    category: CapturedField<&'static str>,
    correlation: CapturedField<uuid::Uuid>,
    message: CapturedField<()>,
    error: Option<TraceCaptureError>,
}
impl DiagnosticFields {
    fn new(expected_kind: &'static str) -> Self {
        Self {
            expected_kind,
            event_name: CapturedField::Missing,
            code: CapturedField::Missing,
            category: CapturedField::Missing,
            correlation: CapturedField::Missing,
            message: CapturedField::Missing,
            error: None,
        }
    }
    fn finish(self) -> CapturedDiagnostic {
        if let Some(error) = self.error {
            return Err(error);
        }
        // An emitted diagnostic missing this new field fails without parsing formatted log text.
        let reported_error_kind = self.category.require(DiagnosticField::Category)?;
        self.event_name.require(DiagnosticField::EventName)?;
        self.message.require(DiagnosticField::Message)?;
        Ok(ClosedProviderDiagnostic {
            reported_error_kind,
            provider_code: self.code.require(DiagnosticField::Code)?,
            correlation_id: self.correlation.require(DiagnosticField::Correlation)?,
        })
    }
    fn reject_type(&mut self, field: &Field) {
        let field = match field.name() {
            "event.name" => DiagnosticField::EventName,
            "provider_code" => DiagnosticField::Code,
            "reported_error_kind" => DiagnosticField::Category,
            "correlation_id" => DiagnosticField::Correlation,
            "message" => DiagnosticField::Message,
            _ => {
                self.error = Some(TraceCaptureError::UnexpectedField);
                return;
            }
        };
        self.error = Some(TraceCaptureError::InvalidField(field));
    }
}
impl Visit for DiagnosticFields {
    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "provider_code" {
            self.code.record(Some(value));
        } else {
            self.reject_type(field);
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "event.name" => self
                .event_name
                .record((value == PROVIDER_ERROR_EVENT).then_some(())),
            "reported_error_kind" => self
                .category
                .record((value == self.expected_kind).then_some(self.expected_kind)),
            "correlation_id" => self.correlation.record(parse_correlation(value)),
            "message" => self
                .message
                .record((value == "Observed ACP reported error category").then_some(())),
            _ => self.reject_type(field),
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            "correlation_id" => {
                let mut buffer = CorrelationBuffer(Vec::with_capacity(36));
                let correlation = if write!(&mut buffer, "{value:?}").is_ok() {
                    std::str::from_utf8(&buffer.0)
                        .ok()
                        .and_then(parse_correlation)
                } else {
                    None
                };
                self.correlation.record(correlation);
            }
            "message" => {
                let mut expected = ExpectedMessage("Observed ACP reported error category");
                let matches = write!(&mut expected, "{value:?}").is_ok() && expected.0.is_empty();
                self.message.record(matches.then_some(()));
            }
            _ => self.reject_type(field),
        }
    }
}

fn parse_correlation(value: &str) -> Option<uuid::Uuid> {
    if value.len() != 36 {
        return None;
    }
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|identity| identity.get_version_num() == 7)
}
struct CorrelationBuffer(Vec<u8>);
impl fmt::Write for CorrelationBuffer {
    fn write_str(&mut self, chunk: &str) -> fmt::Result {
        if self.0.len().saturating_add(chunk.len()) > 36
            || !chunk
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err(fmt::Error);
        }
        self.0.extend_from_slice(chunk.as_bytes());
        Ok(())
    }
}
struct ExpectedMessage(&'static str);
impl fmt::Write for ExpectedMessage {
    fn write_str(&mut self, chunk: &str) -> fmt::Result {
        self.0 = self.0.strip_prefix(chunk).ok_or(fmt::Error)?;
        Ok(())
    }
}
