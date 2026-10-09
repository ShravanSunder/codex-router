#[test]
fn keeper_result_is_the_protocol_type_without_conversion_or_duplicate() {
    fn keeper_consumer(
        value: codex_router_keeper_protocol::GroupStopResult,
    ) -> codex_router_keeper::GroupStopResult {
        value
    }
    let value = keeper_consumer(codex_router_keeper_protocol::GroupStopResult::Killed);
    assert_eq!(value, codex_router_keeper::GroupStopResult::Killed);
}
