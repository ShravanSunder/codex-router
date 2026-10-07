/// Native app-server exchange action selected by the owning caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppServerProbeAction {
    /// Read the current Remote Control status once.
    Observe,
    /// Retry native transport connection failures within one readiness budget, then observe status.
    WaitForReady,
    /// Request ephemeral Remote Control enablement once, then observe its status.
    EnableAndObserve,
}
