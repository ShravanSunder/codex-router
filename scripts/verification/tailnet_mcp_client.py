"Real stateless MCP transport and independent destination board readback."

import http.client
import json
import os
import socket
import sqlite3
import ssl
import stat
import typing as t
import urllib.parse
from contextlib import closing
from pathlib import Path

from pydantic import (
    TypeAdapter,
)

from scripts.verification.owned_runtime import (
    save_record,
)
from scripts.verification.tailnet_mcp_models import (
    BoardMarker,
    BoardPlan,
    HttpReply,
    HttpsEndpoint,
    LoopbackEndpoint,
    NativeReadiness,
    NativeStartingInventory,
    ProbeHeaders,
    require_deadline,
)
from scripts.verification.typed_observations import (
    GenerationView,
    JsonObject,
    read_observed_json,
)


def require_private_directory(path: Path) -> None:
    metadata = path.lstat()
    if (
        path.resolve() != path
        or not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or metadata.st_mode & 0o077
    ):
        raise ValueError(
            "evidence/destination directory is not owner-private and canonical"
        )


@t.final
class UnixHttpConnection(http.client.HTTPConnection):
    def __init__(self, path: Path) -> None:
        super().__init__("localhost", timeout=5)
        self.socket_path: Path = path

    @t.override
    def connect(self) -> None:
        connected = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connected.settimeout(5)
        connected.connect(str(self.socket_path))
        self.sock = connected


def http_post(
    target: HttpsEndpoint | LoopbackEndpoint | Path,
    payload: JsonObject,
    probe_headers: ProbeHeaders | None = None,
) -> HttpReply:
    if isinstance(target, Path):
        connection = UnixHttpConnection(target)
    elif isinstance(target, LoopbackEndpoint):
        url = urllib.parse.urlsplit(target.url)
        connection = http.client.HTTPConnection(url.hostname or "", url.port, timeout=5)
    else:
        url = urllib.parse.urlsplit(target.url)
        connection = http.client.HTTPSConnection(
            url.hostname or "",
            url.port or 443,
            timeout=5,
            context=ssl.create_default_context(),
        )
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2025-11-25",
    }
    if isinstance(target, HttpsEndpoint):
        headers["Host"] = target.authority
    if probe_headers is not None:
        if probe_headers.origin is not None:
            headers["Origin"] = probe_headers.origin
        if probe_headers.host_authority is not None:
            headers["Host"] = probe_headers.host_authority
    try:
        connection.request("POST", "/mcp", json.dumps(payload), headers)
        response = connection.getresponse()
        body = response.read(4 * 1024 * 1024 + 1)
        if len(body) > 4 * 1024 * 1024:
            raise ValueError("HTTP response exceeded evidence bound")
        return HttpReply(status=response.status, body=body)
    finally:
        connection.close()


def rpc_result(reply: HttpReply) -> JsonObject:
    envelope = read_observed_json(reply.body)
    if (
        reply.status != 200
        or envelope.get("jsonrpc") != "2.0"
        or envelope.get("id") != 1
        or "error" in envelope
    ):
        raise ValueError("MCP HTTP/error/correlation failure")
    result = read_observed_json(json.dumps(envelope.get("result")))
    if result.get("isError") is True:
        raise ValueError("MCP tool returned an error; no mutation replay")
    return result


def tool(target: HttpsEndpoint | Path, name: str, arguments: JsonObject) -> JsonObject:
    result = rpc_result(
        http_post(
            target,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            },
        )
    )
    return read_observed_json(json.dumps(result.get("structuredContent")))


def require_service(
    inventory: JsonObject, service_id: str, service_epoch: str | None = None
) -> None:
    if service_epoch is not None and inventory.get("serviceEpoch") != service_epoch:
        raise ValueError("destination service epoch changed")
    endpoints = inventory.get("endpoints")
    if not isinstance(endpoints, list) or not endpoints:
        raise ValueError("endpoint inventory absent")
    for entry in endpoints:
        record = read_observed_json(json.dumps(entry))
        endpoint = read_observed_json(json.dumps(record.get("endpoint")))
        if endpoint.get("serviceId") != service_id:
            raise ValueError("wrong destination service")
    native = [
        read_observed_json(json.dumps(item))
        for item in endpoints
        if read_observed_json(json.dumps(item)).get("endpoint")
        == {"serviceId": service_id, "endpointId": "codex-local"}
    ]
    if (
        len(native) != 1
        or read_observed_json(json.dumps(native[0].get("availability"))).get("state")
        != "available"
    ):
        raise ValueError("native endpoint is not available")
    channels = native[0].get("channels")
    if not isinstance(channels, list) or not any(
        isinstance(item, dict)
        and item.get("kind") == "nativeCodex"
        and isinstance(item.get("generation"), dict)
        for item in channels
    ):
        raise ValueError("native generation missing")
    for channel in channels:
        if isinstance(channel, dict) and channel.get("kind") == "nativeCodex":
            generation = GenerationView.model_validate(channel.get("generation"))
            if service_epoch is not None and generation.serviceEpoch != service_epoch:
                raise ValueError("native endpoint epoch differs from the declared Host")


def observe_native_readiness(
    inventory: JsonObject, service_id: str, service_epoch: str
) -> NativeReadiness:
    endpoints = inventory.get("endpoints")
    native_present = isinstance(endpoints, list) and any(
        isinstance(entry, dict)
        and isinstance(endpoint := entry.get("endpoint"), dict)
        and endpoint.get("endpointId") == "codex-local"
        for entry in endpoints
    )
    if not native_present:
        starting = NativeStartingInventory.model_validate(inventory)
        if starting.serviceEpoch != service_epoch:
            raise ValueError("destination service epoch changed")
        if any(entry.endpoint.serviceId != service_id for entry in starting.endpoints):
            raise ValueError("wrong destination service")
        return NativeReadiness.STARTING
    require_service(inventory, service_id, service_epoch)
    return NativeReadiness.READY


def require_marker(record: JsonObject, marker: BoardMarker, actor: JsonObject) -> None:
    if (
        record.get("messageId") != marker.message_id
        or record.get("topicId") != marker.topic_id
        or record.get("text") != marker.text
        or record.get("actor") != actor
    ):
        raise ValueError("destination board marker/actor mismatch")


def observe_http_boundary(local: LoopbackEndpoint, remote: HttpsEndpoint) -> JsonObject:
    """Drive the real loopback listener with Host and Origin varied independently."""
    payload: JsonObject = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {},
    }
    parsed = urllib.parse.urlsplit(remote.url)
    wrong_port = 1 if (parsed.port or 443) == 65535 else (parsed.port or 443) + 1
    probes = {
        "loopbackControl": (ProbeHeaders(), 200),
        "existingLoopbackOrigin": (ProbeHeaders(origin=local.origin), 200),
        "proxyAuthorityControl": (ProbeHeaders(host_authority=remote.authority), 200),
        "loopbackOriginThroughProxyAuthority": (
            ProbeHeaders(host_authority=remote.authority, origin=local.origin),
            200,
        ),
        "proxyHttpOrigin": (
            ProbeHeaders(
                host_authority=remote.authority, origin=f"http://{remote.authority}"
            ),
            403,
        ),
        "wrongHost": (
            ProbeHeaders(host_authority=f"unexpected.invalid:{parsed.port or 443}"),
            403,
        ),
        "wrongPort": (
            ProbeHeaders(host_authority=f"{parsed.hostname}:{wrong_port}"),
            403,
        ),
        "missingPort": (
            ProbeHeaders(host_authority=parsed.hostname),
            200 if (parsed.port or 443) == 443 else 403,
        ),
        "tailnetOrigin": (
            ProbeHeaders(host_authority=remote.authority, origin=remote.origin),
            403,
        ),
    }
    statuses: JsonObject = {}
    for name, (headers, expected) in probes.items():
        response = http_post(local, payload, headers)
        if response.status != expected:
            raise ValueError(
                f"HTTP Host/Origin boundary failed: {name} returned {response.status}"
            )
        if expected == 200:
            _ = rpc_result(response)
        statuses[name] = response.status
    return statuses


def run_peer(plan: BoardPlan) -> None:
    """One write only: save intent first, then preserve unknown outcomes without replay."""
    require_deadline(plan.deadline)
    require_private_directory(plan.evidence)
    require_service(
        tool(plan.https, "endpoints_list", {}),
        plan.destination_service_id,
        plan.destination_service_epoch,
    )
    payload: JsonObject = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {},
    }
    tools = rpc_result(http_post(plan.https, payload))
    available = tools.get("tools")
    tool_names: set[str] = set()
    if isinstance(available, list):
        for item in available:
            if isinstance(item, dict):
                name = item.get("name")
                if isinstance(name, str):
                    tool_names.add(name)
    if not {
        "board_message_post",
        "board_message_show",
        "endpoints_list",
    }.issubset(tool_names):
        raise ValueError("required real MCP tools missing")
    origin_statuses: JsonObject = {}
    for origin in [plan.https.origin, "https://invalid.example"]:
        negative = http_post(plan.https, payload, ProbeHeaders(origin=origin))
        if negative.status != 403:
            raise ValueError("remote Origin was not rejected")
        origin_statuses[origin] = negative.status
    save_record(
        plan.evidence / "peer-intent.json",
        {
            "serviceId": plan.destination_service_id,
            "marker": plan.marker.model_dump(),
            "tools": tools,
            "originStatuses": origin_statuses,
        },
    )
    posted = tool(
        plan.https,
        "board_message_post",
        {
            "messageId": plan.marker.message_id,
            "placement": {"kind": "topic", "topicId": plan.marker.topic_id},
            "actor": plan.actor,
            "text": plan.marker.text,
            "references": [],
        },
    )
    require_marker(
        read_observed_json(json.dumps(posted.get("message"))), plan.marker, plan.actor
    )
    observed = tool(
        plan.https, "board_message_show", {"messageId": plan.marker.message_id}
    )
    require_marker(observed, plan.marker, plan.actor)
    save_record(
        plan.evidence / "peer-result.json", {"posted": posted, "readback": observed}
    )


def read_stored_marker(database: Path, marker: BoardMarker, actor: JsonObject) -> None:
    with closing(
        sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)
    ) as connection:
        row = t.cast(
            object,
            connection.execute(
                "SELECT m.message_id,m.topic_id,m.text,i.kind,i.service_id,i.endpoint_id,i.session_id FROM board_messages m JOIN board_identities i ON i.identity_key=m.actor_key WHERE m.message_id=?",
                (marker.message_id,),
            ).fetchone(),
        )
    stored = TypeAdapter(tuple[str, str, str, str, str, str, str]).validate_python(row)
    expected: JsonObject = {
        "kind": stored[3],
        "session": {
            "endpoint": {"serviceId": stored[4], "endpointId": stored[5]},
            "sessionId": stored[6],
        },
    }
    require_marker(
        {
            "messageId": stored[0],
            "topicId": stored[1],
            "text": stored[2],
            "actor": expected,
        },
        marker,
        actor,
    )


def run_readback(plan: BoardPlan) -> None:
    require_deadline(plan.deadline)
    require_private_directory(plan.destination_root)
    require_private_directory(plan.evidence)
    manifest = read_observed_json((plan.service / "service.json").read_bytes())
    if (
        manifest.get("serviceId") != plan.destination_service_id
        or manifest.get("serviceEpoch") != plan.destination_service_epoch
    ):
        raise ValueError("independent destination manifest mismatch")
    require_service(
        tool(plan.service / "control.sock", "endpoints_list", {}),
        plan.destination_service_id,
        plan.destination_service_epoch,
    )
    observed = tool(
        plan.service / "control.sock",
        "board_message_show",
        {"messageId": plan.marker.message_id},
    )
    require_marker(observed, plan.marker, plan.actor)
    database = plan.service / "project-board.sqlite"
    if (
        not database.is_file()
        or database.is_symlink()
        or database.stat().st_uid != os.getuid()
    ):
        raise ValueError("destination board store binding absent")
    read_stored_marker(database, plan.marker, plan.actor)
    save_record(
        plan.evidence / "destination-readback.json",
        {
            "serviceId": plan.destination_service_id,
            "store": str(database),
            "readback": observed,
            "restartPersistence": "not exercised",
        },
    )
