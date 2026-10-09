//! The largest JSON message a Router carrier socket or stored message accepts.
//!
//! The carrier connections (the Codex ACP and app-server sockets, provider sessions) and the
//! stored-message validators share this bound; the collaboration API bounds its results with
//! the application's result budget instead.
pub const MAX_CONTROL_FRAME_BYTES: usize = 1024 * 1024;
