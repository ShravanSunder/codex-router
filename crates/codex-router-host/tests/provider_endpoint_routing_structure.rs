//! Keep endpoint identity in Host composition and route using bound facts.

#[test]
fn routing_owners_do_not_branch_on_endpoint_id_strings() {
    let route_sources = [
        (
            "provider_acp_delivery_route",
            include_str!("../src/provider_acp_delivery_route.rs"),
        ),
        (
            "provider_acp_route_claim",
            include_str!("../src/provider_acp_route_claim.rs"),
        ),
        (
            "provider_acp_scheduled_runs",
            include_str!("../src/provider_acp_scheduled_runs.rs"),
        ),
        (
            "claude_code_peer_delivery_route",
            include_str!("../src/claude_code_peer_delivery_route.rs"),
        ),
    ];
    for (owner, source) in route_sources {
        let production = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        for endpoint_id in ["\"claude-local\"", "\"cursor-local\"", "\"codex-local\""] {
            assert!(
                !production.contains(endpoint_id),
                "{owner} branches on endpoint ID {endpoint_id}; use composition or binding facts"
            );
        }
    }
}
