//! Explicit fresh/fork/adopt selectors; only fresh destinations use the discovered local service ID.
use clap::Args;
use communication_protocol::{
    DestinationPreparation, EndpointId, EndpointRef, NonEmptyText, SessionRef, UuidIdentity,
};
#[derive(Args)]
pub(crate) struct PreparationArguments {
    #[arg(long)]
    pub(crate) schedule_id: String,
    #[arg(long)]
    pub(crate) operation_id: Option<String>,
    /// Create a fresh native thread; preparation does not activate the schedule.
    #[arg(long,required_unless_present_any=["existing","fork_from"],conflicts_with_all=["existing","fork_from"])]
    fresh: bool,
    /// Explicitly adopt an existing exact SessionRef JSON address.
    #[arg(long, conflicts_with = "fork_from")]
    existing: Option<String>,
    /// Fork this exact SessionRef JSON address through the specified completed turn.
    #[arg(long, requires = "through_turn")]
    fork_from: Option<String>,
    #[arg(long, requires = "fork_from")]
    through_turn: Option<String>,
    #[arg(long, default_value = "codex-local")]
    endpoint: String,
    /// Absolute workspace path; native response must confirm it.
    #[arg(long)]
    cwd: String,
}
pub(crate) enum PreparedDestination {
    Fresh {
        endpoint_id: EndpointId,
        cwd: String,
    },
    Exact(DestinationPreparation),
}
impl PreparedDestination {
    pub(crate) fn resolve(self, service_id: UuidIdentity) -> DestinationPreparation {
        match self {
            Self::Fresh { endpoint_id, cwd } => DestinationPreparation::Fresh {
                endpoint: EndpointRef {
                    service_id,
                    endpoint_id,
                },
                cwd,
            },
            Self::Exact(destination) => destination,
        }
    }
}
impl PreparationArguments {
    pub(crate) fn destination(&self) -> Result<PreparedDestination, String> {
        if !std::path::Path::new(&self.cwd).is_absolute() {
            return Err("--cwd must be an absolute workspace path".into());
        }
        if let Some(existing) = &self.existing {
            let target: SessionRef = serde_json::from_str(existing)
                .map_err(|_| "--existing requires exact SessionRef JSON from discovery")?;
            return Ok(PreparedDestination::Exact(
                DestinationPreparation::Existing {
                    target,
                    cwd: self.cwd.clone(),
                },
            ));
        }
        if let Some(source) = &self.fork_from {
            let source: SessionRef = serde_json::from_str(source)
                .map_err(|_| "--fork-from requires exact SessionRef JSON")?;
            let through_turn_id: NonEmptyText = self
                .through_turn
                .clone()
                .ok_or("--fork-from requires --through-turn")?
                .try_into()
                .map_err(|_| "--through-turn requires an exact nonempty completed turn ID")?;
            return Ok(PreparedDestination::Exact(DestinationPreparation::Fork {
                source,
                through_turn_id,
                cwd: self.cwd.clone(),
            }));
        }
        if !self.fresh {
            return Err("Choose --fresh, --existing or --fork-from".into());
        }
        Ok(PreparedDestination::Fresh {
            endpoint_id: self
                .endpoint
                .clone()
                .try_into()
                .map_err(|_| "--endpoint requires a valid endpoint ID")?,
            cwd: self.cwd.clone(),
        })
    }
}
