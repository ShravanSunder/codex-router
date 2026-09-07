//! Per-connection snapshot watermarks and contiguous endpoint-change delivery.
use crate::ClientError;
use communication_protocol::{ControlInitializationResult, EndpointChange, UuidIdentity};
use serde_json::{Value, json};
use std::collections::VecDeque;

pub(crate) struct EndpointNotificationState {
    service_id: UuidIdentity,
    service_epoch: UuidIdentity,
    snapshot_sequence: u64,
    delivered_sequence: u64,
}

impl EndpointNotificationState {
    pub(crate) fn new(identity: &ControlInitializationResult) -> Self {
        Self {
            service_id: identity.service_id.clone(),
            service_epoch: identity.service_epoch.clone(),
            snapshot_sequence: 0,
            delivered_sequence: 0,
        }
    }

    pub(crate) fn apply_snapshot(&mut self, sequence: u64) -> Result<(), ClientError> {
        if sequence < self.delivered_sequence {
            return Err(ClientError::Protocol(
                "endpoint snapshot sequence regressed",
            ));
        }
        self.snapshot_sequence = sequence;
        self.delivered_sequence = sequence;
        Ok(())
    }

    pub(crate) fn discard_covered(&self, frames: &mut VecDeque<Value>) -> Result<(), ClientError> {
        let mut retained = VecDeque::with_capacity(frames.len());
        while let Some(frame) = frames.pop_front() {
            if frame
                .pointer("/params/sequence")
                .and_then(Value::as_u64)
                .is_some_and(|sequence| sequence <= self.snapshot_sequence)
            {
                // A covered position never licenses silently hiding malformed/foreign frames.
                self.validate(&frame)?;
            } else {
                retained.push_back(frame);
            }
        }
        *frames = retained;
        Ok(())
    }

    pub(crate) fn consume(&mut self, frame: Value) -> Result<Option<Value>, ClientError> {
        let change = self.validate(&frame)?;
        if change.sequence <= self.snapshot_sequence {
            return Ok(None);
        }
        // Only the snapshot covers old frames. A duplicate of a subsequently delivered
        // event is still a protocol violation, as is a gap above that watermark.
        if change.sequence != self.delivered_sequence + 1 {
            return Err(ClientError::Protocol(
                "notification scope or sequence mismatch",
            ));
        }
        self.delivered_sequence = change.sequence;
        Ok(Some(frame))
    }

    fn validate(&self, frame: &Value) -> Result<EndpointChange, ClientError> {
        if frame.get("jsonrpc") != Some(&json!("2.0"))
            || frame.get("method") != Some(&json!("endpoint/changed"))
            || frame.get("id").is_some()
            || frame.as_object().is_none_or(|value| value.len() != 3)
        {
            return Err(ClientError::Protocol("invalid endpoint notification"));
        }
        let change: EndpointChange = serde_json::from_value(
            frame
                .get("params")
                .cloned()
                .ok_or(ClientError::Protocol("missing notification parameters"))?,
        )
        .map_err(|_| ClientError::Protocol("invalid notification parameters"))?;
        if change.service_epoch != self.service_epoch
            || change.endpoint.endpoint.service_id != self.service_id
            || change.sequence == 0
            || change.sequence > 9_007_199_254_740_991
        {
            return Err(ClientError::Protocol(
                "notification scope or sequence mismatch",
            ));
        }
        Ok(change)
    }
}
