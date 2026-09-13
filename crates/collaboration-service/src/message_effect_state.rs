//! Request-local effect evidence; never a replay or delivery store.
use serde_json::{Value, json};

pub(crate) struct MessageEffects {
    pub id: Value,
    pub resume: &'static str,
    pub submission: &'static str,
    pub correlation: Option<String>,
}
impl MessageEffects {
    pub fn new(id: Value) -> Self {
        Self {
            id,
            resume: "notRequested",
            submission: "notDispatched",
            correlation: None,
        }
    }
    pub fn failure(&self, kind: &str, stage: &str) -> Value {
        let mut data = json!({"kind":kind,"stage":stage,"message":"Message operation failed",
            "effects":{"resume":self.resume,"submission":self.submission}});
        if let (Some(fields), Some(id)) = (data.as_object_mut(), &self.correlation) {
            fields.insert("clientUserMessageId".to_owned(), json!(id));
        }
        json!({"jsonrpc":"2.0","id":self.id,"error":{"code":-32050,"message":"Message operation failed","data":data}})
    }
}
