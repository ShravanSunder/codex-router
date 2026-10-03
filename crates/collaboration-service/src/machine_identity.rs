use collaboration_protocol::{MachineLabel, MachineLabelError, UuidIdentity};

/// Router's local identity used by push links and the service discovery manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineIdentity {
    service_id: UuidIdentity,
    machine_label: MachineLabel,
}

impl MachineIdentity {
    /// Chooses the upstream Remote Control name, then the OS hostname, then
    /// the stable service id when the OS cannot provide a hostname.
    pub fn new(
        service_id: UuidIdentity,
        remote_control_server_name: Option<&str>,
    ) -> Result<Self, MachineLabelError> {
        let host_name = nix::unistd::gethostname()
            .ok()
            .and_then(|host_name| host_name.into_string().ok());
        Self::from_sources(service_id, remote_control_server_name, host_name.as_deref())
    }

    fn from_sources(
        service_id: UuidIdentity,
        remote_control_server_name: Option<&str>,
        host_name: Option<&str>,
    ) -> Result<Self, MachineLabelError> {
        let service_id_fallback = String::from(service_id.clone());
        let machine_label_source = remote_control_server_name
            .filter(|name| !name.trim().is_empty())
            .or_else(|| host_name.filter(|name| !name.trim().is_empty()))
            .unwrap_or(&service_id_fallback);
        let machine_label = MachineLabel::try_from(machine_label_source.to_owned())?;
        Ok(Self {
            service_id,
            machine_label,
        })
    }

    #[must_use]
    pub const fn service_id(&self) -> &UuidIdentity {
        &self.service_id
    }

    #[must_use]
    pub const fn machine_label(&self) -> &MachineLabel {
        &self.machine_label
    }
}

#[cfg(test)]
mod tests {
    use super::MachineIdentity;
    use collaboration_protocol::{MAX_MACHINE_LABEL_SCALARS, UuidIdentity};

    const SERVICE_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";

    fn service_id() -> UuidIdentity {
        UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("valid service id")
    }

    #[test]
    fn remote_control_name_precedes_host_name_and_keeps_the_service_id() {
        let identity = MachineIdentity::from_sources(
            service_id(),
            Some("Remote-Control-Name"),
            Some("local-host-name"),
        )
        .expect("machine identity");

        assert_eq!(String::from(identity.service_id().clone()), SERVICE_ID);
        assert_eq!(identity.machine_label().as_str(), "Remote-Control-Name");
    }

    #[test]
    fn host_name_is_used_when_remote_control_name_is_missing() {
        let identity = MachineIdentity::from_sources(service_id(), None, Some("local-host-name"))
            .expect("machine identity");

        assert_eq!(identity.machine_label().as_str(), "local-host-name");
    }

    #[test]
    fn os_hostname_is_used_when_remote_control_name_is_missing() {
        let expected_host_name = nix::unistd::gethostname()
            .ok()
            .and_then(|host_name| host_name.into_string().ok());
        let identity = MachineIdentity::new(service_id(), None).expect("machine identity");

        assert_eq!(
            identity.machine_label().as_str(),
            expected_host_name.as_deref().unwrap_or(SERVICE_ID)
        );
    }

    #[test]
    fn service_id_is_the_last_label_fallback_when_os_name_is_unavailable() {
        let identity =
            MachineIdentity::from_sources(service_id(), None, None).expect("machine identity");

        assert_eq!(identity.machine_label().as_str(), SERVICE_ID);
    }

    #[test]
    fn selected_label_is_bounded_in_unicode_scalars_with_a_truncation_mark() {
        let remote_name = "界".repeat(MAX_MACHINE_LABEL_SCALARS + 20);
        let identity =
            MachineIdentity::from_sources(service_id(), Some(&remote_name), Some("host"))
                .expect("machine identity");

        assert_eq!(
            identity.machine_label().as_str().chars().count(),
            MAX_MACHINE_LABEL_SCALARS
        );
        assert!(identity.machine_label().as_str().ends_with('…'));
    }
}
