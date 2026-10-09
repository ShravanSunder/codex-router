use crate::ComponentFingerprint;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentFingerprints {
    pub keeper: ComponentFingerprint,
    pub agent_collaboration_services: ComponentFingerprint,
    pub agent_proxy_services: ComponentFingerprint,
    pub agent_provider_services: ComponentFingerprint,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildInfo {
    pub package_version: semver::Version,
    pub fingerprints: ComponentFingerprints,
}
