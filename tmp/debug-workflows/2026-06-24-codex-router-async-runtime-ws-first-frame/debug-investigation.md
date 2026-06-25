Debug investigation
Date: 2026-06-24
Bug: live Codex through codex-router emits repeated FirstFrameTimeout / Broken pipe

Initial evidence from user:
- router logs: websocket closed before upstream open: FirstFrameTimeout repeated
- client: stream disconnected before completion: failed to send websocket request: IO error: Broken pipe (os error 32)


Evidence update 2026-06-24T23:05:56Z
- User reports live router emits repeated FirstFrameTimeout and client Broken pipe.
- Local router PID 87793 had active local and upstream established sockets, so old single-lane accept bug is not the whole issue.
- DeepWiki/openai-codex says Codex can preconnect WebSocket and hold it idle before sending response.create; router must not impose an invented 250ms first-frame deadline.
