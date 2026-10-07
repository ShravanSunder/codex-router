use serde::{Deserialize, Serialize};

use crate::GenerationCurrentPayload;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "evidenceUse", rename_all = "camelCase")]
pub enum GenerationPreparation {
    CandidateAdmission { payload: GenerationCurrentPayload },
    CurrentGeneration { payload: GenerationCurrentPayload },
}
