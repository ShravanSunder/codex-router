use super::TokenGeneration;
use super::WebSocketRevocationRegistry;

#[test]
fn registry_snapshot_tracks_active_high_water_and_cleanup() {
    let registry = WebSocketRevocationRegistry::new();

    let first_session = registry.register_cancellation(TokenGeneration::new(1));
    let second_session = registry.register_cancellation(TokenGeneration::new(1));
    assert_eq!(registry.snapshot().active_sessions, 2);
    assert_eq!(registry.snapshot().high_water_sessions, 2);
    assert_eq!(registry.snapshot().registered_sessions, 2);
    assert_eq!(registry.snapshot().closed_sessions, 0);
    assert_eq!(registry.snapshot().completed_response_sessions, 0);

    second_session.note_response_completed();
    assert_eq!(registry.snapshot().completed_response_sessions, 1);

    drop(first_session);
    assert_eq!(registry.snapshot().active_sessions, 1);
    assert_eq!(registry.snapshot().high_water_sessions, 2);
    assert_eq!(registry.snapshot().closed_sessions, 1);

    drop(second_session);
    assert_eq!(registry.snapshot().active_sessions, 0);
    assert_eq!(registry.snapshot().high_water_sessions, 2);
    assert_eq!(registry.snapshot().registered_sessions, 2);
    assert_eq!(registry.snapshot().closed_sessions, 2);
}

#[test]
fn registry_revokes_only_stale_generation_cancellations() {
    let registry = WebSocketRevocationRegistry::new();
    let stale_session = registry.register_cancellation(TokenGeneration::new(1));
    let active_session = registry.register_cancellation(TokenGeneration::new(2));

    registry.close_all_except(TokenGeneration::new(2));

    assert!(stale_session.cancellation().is_cancelled());
    assert!(!active_session.cancellation().is_cancelled());
}
