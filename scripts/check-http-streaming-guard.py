#!/usr/bin/env python3
"""Guard HTTP/SSE streaming and bounded affinity behavior in release proxy code."""

import re
import sys
from pathlib import Path


REPO = Path.cwd()


def read(relative_path: str) -> str:
    return (REPO / relative_path).read_text(encoding="utf-8")


def function_body(source: str, function_name: str) -> str:
    match = re.search(rf"\n\s*(?:async\s+)?fn\s+{re.escape(function_name)}\b", source)
    if match is None:
        raise AssertionError(f"missing function {function_name}")
    start = match.start()
    brace = source.find("{", start)
    if brace == -1:
        raise AssertionError(f"missing function body for {function_name}")
    depth = 0
    for index in range(brace, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[brace : index + 1]
    raise AssertionError(f"unterminated function body for {function_name}")


def require_contains(label: str, haystack: str, needle: str) -> None:
    if needle not in haystack:
        raise AssertionError(f"{label} missing required marker: {needle}")


def forbid_contains(label: str, haystack: str, needle: str) -> None:
    if needle in haystack:
        raise AssertionError(f"{label} contains forbidden marker: {needle}")


def check_g24() -> None:
    server = read("crates/codex-router-proxy/src/server.rs")
    upstream = read("crates/codex-router-proxy/src/upstream.rs")
    http_sse = read("crates/codex-router-proxy/src/http_sse.rs")

    request_adapter = function_body(server, "hyper_request_to_streaming_proxy_request")
    request_prefix = function_body(server, "bounded_request_metadata_body")
    request_body = function_body(server, "request_metadata_prefix_is_complete_json")
    require_contains("HTTP request adapter", request_adapter, "bounded_request_metadata_body")
    require_contains(
        "HTTP request prefix helper",
        request_prefix,
        "HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES",
    )
    require_contains("HTTP request prefix helper", request_prefix, "body.frame().await")
    require_contains("HTTP request prefix helper", request_prefix, "VecDeque")
    require_contains("HTTP request prefix helper", request_prefix, "PrefixFramesThenIncomingBody::new")
    require_contains("HTTP request complete helper", request_body, "serde_json::from_slice")
    forbid_contains("HTTP request adapter", request_adapter, ".collect()")
    forbid_contains("HTTP request adapter", request_adapter, ".to_bytes()")

    upstream_send = function_body(upstream, "send_streaming_inner")
    require_contains("Hyper upstream transport", upstream, "request: StreamingUpstreamHttpRequest")
    require_contains("Hyper upstream transport", upstream_send, ".body(request.into_body())")
    forbid_contains("Hyper upstream transport", upstream_send, "Full::new")
    forbid_contains("Hyper upstream transport", upstream_send, "copy_from_slice(request.body())")
    forbid_contains("Hyper upstream transport", upstream_send, "request.body().to_vec()")

    require_contains(
        "streaming request DTO",
        http_sse,
        "pub struct StreamingUpstreamHttpRequest",
    )
    require_contains(
        "streaming async transport trait",
        http_sse,
        "request: StreamingUpstreamHttpRequest",
    )


def check_g25() -> None:
    server = read("crates/codex-router-proxy/src/server.rs")
    websocket = read("crates/codex-router-proxy/src/websocket.rs")

    require_contains("HTTP/SSE affinity constants", server, "HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES")
    require_contains("HTTP/SSE affinity constants", server, "HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS")
    affinity_tap = function_body(server, "record_affinity_owner_from_async_body")
    require_contains("HTTP/SSE affinity tap", affinity_tap, "scanned_bytes")
    require_contains("HTTP/SSE affinity tap", affinity_tap, "scanned_events")
    require_contains("HTTP/SSE affinity tap", affinity_tap, "HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES")
    require_contains("HTTP/SSE affinity tap", affinity_tap, "HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS")
    require_contains("HTTP/SSE affinity tap", affinity_tap, "data[..bytes_to_scan]")
    forbid_contains("HTTP/SSE affinity tap", affinity_tap, "buffered.extend_from_slice(data)")

    websocket_pump = function_body(websocket, "pump_upstream_to_local")
    require_contains("WebSocket pump", websocket_pump, "websocket_metadata_text_handle")
    require_contains("WebSocket pump", websocket_pump, "local_write.send(upstream_message).await?")
    require_contains("WebSocket pump", websocket_pump, "record_forwarded_websocket_metadata")
    if websocket_pump.find("local_write.send(upstream_message).await?") > websocket_pump.find(
        "record_forwarded_websocket_metadata"
    ):
        raise AssertionError("WebSocket pump records metadata before forwarding local frame")
    websocket_metadata = function_body(websocket, "websocket_metadata_text_handle")
    require_contains("WebSocket metadata helper", websocket_metadata, "Message::Text")
    require_contains("WebSocket metadata helper", websocket_metadata, "text.clone()")
    forbid_contains("WebSocket metadata helper", websocket_metadata, ".to_string()")
    metadata_recorder = function_body(websocket, "record_forwarded_websocket_metadata")
    require_contains("WebSocket metadata recorder", metadata_recorder, "is_response_completed_text")
    require_contains(
        "WebSocket metadata recorder",
        metadata_recorder,
        "websocket_affinity_owner_record_from_text",
    )


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in {"G-24", "G-25"}:
        print("usage: scripts/check-http-streaming-guard.py G-24|G-25", file=sys.stderr)
        return 2

    try:
        if sys.argv[1] == "G-24":
            check_g24()
        else:
            check_g25()
    except AssertionError as error:
        print(error, file=sys.stderr)
        return 1

    print(f"{sys.argv[1]} HTTP/SSE streaming guard passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
