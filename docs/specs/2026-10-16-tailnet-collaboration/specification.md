# Tailnet MCP contract

Governing Requirements: [requirements.md](requirements.md), U1–U5. The owner confirmed private exposure, automatic Host option and real two-machine debug tests on 2026-10-16.

## Entities

- E1 Host: the existing Router service identity and runtime epoch; restarting changes epoch, not board ownership.
- E2 Tailnet exposure: one Host invocation’s private HTTPS listener at one node name and port, disabled, starting, available, stopping or failed. Its identity includes the Host invocation and configured port.
- E3 Board: an existing board owned by exactly one Host’s existing board store. Remote use never creates a replica implicitly.
- E4 Remote client: a device permitted to reach E2 by the operator’s existing Tailscale policy. Board actor identity remains existing application attribution, not network authentication.

## Observable obligations

- R1/U1,U5: `host --remote-access tailscale` opts in; absence keeps local-only behavior. `--tailscale-https-port` selects a nonzero port, default8443, only with Tailscale access. Invalid combinations fail before exposure.
- R2/U1,U2: Successful startup reports a private HTTPS MCP URL ending `/mcp`; from an allowed peer, actual MCP tools/list, endpoint inventory and own test-board write/readback succeed against the intended service ID. Test both directions, with distinct service IDs.
- R3/U3,U4: The Host owns only its own exposure. Occupied or public/Funnel-enabled target ports are rejected without overwriting them. No broad Serve reset, persistent background advertisement, ACL change, HTTPS-enable consent or fallback to public/LAN binding is automatic. A missing prerequisite produces an actionable error.
- R4/U3: Startup reports the private URL only after the bound loopback MCP answers for the intended service ID and epoch under the exact proxy Host authority and the owned foreground Serve session maps that node/port to that backend. It does not require the node to connect to its own Tailscale HTTPS address. Normal certificate-validated peer HTTPS and actual remote availability remain the separate R2 proof obligation, never inferred from local health or Serve status. Unexpected exposure exit fails the configured Host through its normal cleanup. Shutdown reaps the exposure process and retires its foreground Serve session, preserving unrelated configuration.
- R5/U4: Unix/local MCP and native/operator/provider transports retain their current behavior. Tailnet exposure forwards the existing collaboration MCP only. Serve forwards to the existing loopback HTTP MCP listener. Only while opted in, the listener additionally accepts the exact validated node DNS name and selected external HTTPS port as Host authority. Unknown names or ports are refused; default mode adds no authority. Existing allowed browser Origins remain unchanged: the tailnet HTTPS Origin is refused and an Origin-free authorized MCP client is accepted. The Host stays unprivileged.
- R6/U2,U4: Remote board changes are durable in the destination Host’s existing store and read back there; no source-machine board/session substitute or database copy is accepted as proof.
- R7/U1,U3: Existing Host restart/replacement carries the selected access mode and port. There is no unattended timer or retry loop introduced by this feature.

Proof: parser and actual subprocess lifecycle tests cover R1,R3,R4,R7; actual loopback Host-authority and Origin rejection/control tests, preserved Unix/local API tests, and real peer HTTPS Origin negative/control prove R5; actual isolated debug Hosts on Sunclaw and Sunbook with current normal artifacts, real Tailscale HTTPS and reciprocal MCP/board operations establish R2,R6 and cleanup. Mocks, a listening port, ping, SSH or a tools/list alone do not establish board readback or native session actions. No production replacement is part of this proof.
