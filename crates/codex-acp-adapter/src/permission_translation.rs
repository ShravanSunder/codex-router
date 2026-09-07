//! Connection/generation-scoped permission correlation without responder exclusivity claims.
use crate::AcpSchemaCatalog;
use communication_protocol::CodexGeneration;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum PermissionTranslationError {
    #[error("unsupported or malformed native permission request")]
    InvalidRequest,
    #[error("no representable native permission options")]
    NoOptions,
    #[error("permission response belongs to a retired generation")]
    RetiredGeneration,
    #[error("permission schema unavailable")]
    Schema,
}
pub struct PendingPermission {
    generation: CodexGeneration,
    native_id: Value,
    acp_id: String,
    options: BTreeMap<String, &'static str>,
    request: Value,
}
pub struct PermissionReply {
    /// Sending this proves only submission; native callback resolution is first-response-wins.
    pub native_response: Value,
    pub invalid_selection: bool,
}
impl PendingPermission {
    pub fn translate(
        catalog: &mut AcpSchemaCatalog,
        generation: CodexGeneration,
        acp_id: String,
        native: &Value,
    ) -> Result<Self, PermissionTranslationError> {
        let method = native
            .get("method")
            .and_then(Value::as_str)
            .ok_or(PermissionTranslationError::InvalidRequest)?;
        let (kind, title) = match method {
            "item/commandExecution/requestApproval" => ("execute", "Execute command"),
            "item/fileChange/requestApproval" => ("edit", "Apply file changes"),
            _ => return Err(PermissionTranslationError::InvalidRequest),
        };
        let native_id = native
            .get("id")
            .filter(|id| id.is_string() || id.as_i64().is_some())
            .ok_or(PermissionTranslationError::InvalidRequest)?
            .clone();
        let params = native
            .get("params")
            .ok_or(PermissionTranslationError::InvalidRequest)?;
        let session_id = params
            .get("threadId")
            .and_then(Value::as_str)
            .ok_or(PermissionTranslationError::InvalidRequest)?;
        let item_id = params
            .get("itemId")
            .and_then(Value::as_str)
            .ok_or(PermissionTranslationError::InvalidRequest)?;
        let defaults = vec![json!("accept"), json!("acceptForSession"), json!("decline")];
        let decisions = match params.get("availableDecisions") {
            None | Some(Value::Null) => &defaults,
            Some(Value::Array(decisions)) => decisions,
            _ => return Err(PermissionTranslationError::InvalidRequest),
        };
        let mut options = BTreeMap::new();
        let mut offered = Vec::new();
        for decision in decisions {
            let mapping = match decision.as_str() {
                Some("accept") => Some((
                    "native-accept",
                    "accept",
                    "allow_once",
                    "Allow this operation once",
                )),
                Some("acceptForSession") => Some((
                    "native-accept-session",
                    "acceptForSession",
                    "allow_always",
                    "Remember for this Codex session only",
                )),
                Some("decline") => Some((
                    "native-decline",
                    "decline",
                    "reject_once",
                    "Deny this operation",
                )),
                _ => None,
            };
            if let Some((id, native, kind, name)) = mapping
                && options.insert(id.to_owned(), native).is_none()
            {
                offered.push(json!({"optionId":id,"kind":kind,"name":name}));
            }
        }
        if offered.is_empty() {
            return Err(PermissionTranslationError::NoOptions);
        }
        let mut context = Vec::new();
        for (field, label) in [
            ("command", "Command"),
            ("cwd", "Working directory"),
            ("reason", "Reason"),
            ("grantRoot", "Requested write root"),
        ] {
            if let Some(value) = params.get(field).and_then(Value::as_str) {
                context.push(format!("{label}: {value}"));
            }
        }
        let content = if context.is_empty() {
            Vec::new()
        } else {
            vec![json!({"type":"content","content":{"type":"text","text":context.join("\n")}})]
        };
        let request_params = json!({"sessionId":session_id,"toolCall":{"toolCallId":item_id,"kind":kind,"title":title,"status":"pending","content":content},"options":offered});
        if !catalog
            .validate("RequestPermissionRequest", &request_params)
            .map_err(|_| PermissionTranslationError::Schema)?
        {
            return Err(PermissionTranslationError::Schema);
        }
        let request = json!({"jsonrpc":"2.0","id":acp_id,"method":"session/request_permission","params":request_params});
        Ok(Self {
            generation,
            native_id,
            acp_id,
            options,
            request,
        })
    }
    #[must_use]
    pub fn request(&self) -> &Value {
        &self.request
    }
    /// Consumption prevents duplicate responses from changing the chosen native response.
    pub fn resolve(
        self,
        catalog: &mut AcpSchemaCatalog,
        generation: &CodexGeneration,
        response: &Value,
    ) -> Result<PermissionReply, PermissionTranslationError> {
        if self.generation != *generation {
            return Err(PermissionTranslationError::RetiredGeneration);
        }
        let valid_envelope = response.get("jsonrpc") == Some(&json!("2.0"))
            && response.get("id") == Some(&json!(self.acp_id))
            && response.get("error").is_none();
        let result = response.get("result").unwrap_or(&Value::Null);
        let valid = valid_envelope
            && catalog
                .validate("RequestPermissionResponse", result)
                .map_err(|_| PermissionTranslationError::Schema)?;
        let outcome = result.get("outcome");
        let decision = if valid {
            match outcome
                .and_then(|outcome| outcome.get("outcome"))
                .and_then(Value::as_str)
            {
                Some("cancelled") => Some("cancel"),
                Some("selected") => outcome
                    .and_then(|outcome| outcome.get("optionId"))
                    .and_then(Value::as_str)
                    .and_then(|id| self.options.get(id).copied()),
                _ => None,
            }
        } else {
            None
        };
        Ok(PermissionReply {
            native_response: json!({"id":self.native_id,"result":{"decision":decision.unwrap_or("cancel")}}),
            invalid_selection: decision.is_none(),
        })
    }
    #[must_use]
    pub fn cancel(self) -> Value {
        json!({"id":self.native_id,"result":{"decision":"cancel"}})
    }
}
