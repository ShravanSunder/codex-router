//! Native start/steer wire operations, preserving payloads validated by the caller's schema profile.
use crate::{NativeConnectionError, NativeProtocolConnection};
use serde_json::{Value, json};

pub struct NativeInputSubmission<'a> {
    pub thread_id: &'a str,
    /// Caller validates each item against the admitted native UserInput schema.
    pub input: &'a [Value],
    pub client_user_message_id: Option<&'a str>,
}
impl NativeInputSubmission<'_> {
    fn parameters(&self) -> Result<Value, NativeConnectionError> {
        if self.thread_id.is_empty()
            || self.thread_id.len() > 4096
            || self.thread_id.contains('\0')
            || !(1..=256).contains(&self.input.len())
            || self.input.iter().any(|value| !value.is_object())
        {
            return Err(NativeConnectionError::InvalidInput);
        }
        let mut params = json!({"threadId":self.thread_id,"input":self.input});
        if let Some(id) = self.client_user_message_id {
            if id.is_empty() || id.len() > 4096 || id.contains('\0') {
                return Err(NativeConnectionError::InvalidInput);
            }
            params
                .as_object_mut()
                .ok_or(NativeConnectionError::Protocol)?
                .insert("clientUserMessageId".into(), json!(id));
        }
        Ok(params)
    }
}
impl NativeProtocolConnection {
    /// Native turn/start may steer existing work. The returned ID proves acceptance only.
    pub async fn start_input(
        &mut self,
        input: NativeInputSubmission<'_>,
    ) -> Result<String, NativeConnectionError> {
        let result = self.request("turn/start", input.parameters()?).await?;
        result
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(NativeConnectionError::OutcomeUnknown)
    }
    /// Exact steering never falls back to starting another turn or inserting queued input.
    pub async fn steer_input(
        &mut self,
        input: NativeInputSubmission<'_>,
        expected_turn_id: &str,
    ) -> Result<String, NativeConnectionError> {
        if expected_turn_id.is_empty()
            || expected_turn_id.len() > 4096
            || expected_turn_id.contains('\0')
        {
            return Err(NativeConnectionError::InvalidInput);
        }
        let mut params = input.parameters()?;
        params
            .as_object_mut()
            .ok_or(NativeConnectionError::Protocol)?
            .insert("expectedTurnId".into(), json!(expected_turn_id));
        let result = self.request("turn/steer", params).await?;
        let turn_id = result
            .get("turnId")
            .and_then(Value::as_str)
            .ok_or(NativeConnectionError::OutcomeUnknown)?;
        if turn_id != expected_turn_id {
            return Err(NativeConnectionError::OutcomeUnknown);
        }
        Ok(turn_id.to_owned())
    }
}
