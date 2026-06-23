# R16 Contract, Boundaries, And Planning Lane

Verdict: needs revision

Coverage: reviewed spec boundary, refresh lifecycle, route result, runtime
selection, auth, proof, and workflow-gate sections plus current state/proxy/
WebSocket/local-auth code anchors.

Accepted findings:

- Blocker: refresh overlay lacks one authoritative state read API and DTO shape.
- Blocker: WebSocket local-auth and first-frame auth-smuggling ownership is
  contradictory.
- Important: proxy selection lacks an authoritative DTO carrying the canonical
  route-result envelope.
- Important: successful affinity hit side effects on weighted-deficit and
  holds are not defined.
- Question: review-to-plan transition needs an authoritative parent receipt.

Reducer route: spec-creation-swarm.
