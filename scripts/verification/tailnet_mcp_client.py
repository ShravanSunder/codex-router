"Real stateless MCP transport and independent destination board readback."

import base64
import http.client
import json
import os
import re
import socket
import sqlite3
import stat
import subprocess
import typing as t
import urllib.parse
from contextlib import closing
from functools import cache
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


@cache
def require_system_curl() -> None:
    version = subprocess.check_output(["/usr/bin/curl", "-q", "--version"], timeout=5)
    parsed = re.match(rb"curl (\d+)\.(\d+)\.(\d+) ", version)
    if parsed is None or tuple(int(part) for part in parsed.groups()) < (8, 4, 0):
        raise ValueError("system curl >=8.4 required for bounded unknown-length bodies")


def curl_post_arguments(target: HttpsEndpoint, headers: dict[str, str]) -> list[str]:
    arguments = [
        "/usr/bin/curl",
        "-q",
        "--silent",
        "--show-error",
        "--globoff",
        "--http1.1",
        "--proto",
        "=https",
        "--noproxy",
        "*",
        "--proxy",
        "",
        "--no-location",
        "--max-redirs",
        "0",
        "--retry",
        "0",
        "--connect-timeout",
        "5",
        "--max-time",
        "5",
        "--max-filesize",
        str(4 * 1024 * 1024),
        "--request",
        "POST",
        "--data-binary",
        "@-",
        "--header",
        "User-Agent:",
        "--header",
        "Expect:",
        "--write-out",
        "\n%{http_code}",
        "--url",
        target.url,
    ]
    for name, value in headers.items():
        if not re.fullmatch(r"[A-Za-z0-9-]+", name) or any(
            character in value for character in "\r\n\0"
        ):
            raise ValueError("invalid HTTP probe header")
        arguments.extend(["--header", f"{name}: {value}"])
    return arguments


def curl_http_reply(completed: subprocess.CompletedProcess[bytes]) -> HttpReply:
    if completed.returncode != 0:
        raise RuntimeError(
            f"HTTPS curl failed (exit {completed.returncode}): "
            + completed.stderr[:4096].decode(errors="replace")
        )
    body, delimiter, status = completed.stdout.rpartition(b"\n")
    if delimiter != b"\n" or not re.fullmatch(rb"[1-5][0-9]{2}", status):
        raise ValueError("HTTPS curl returned no valid HTTP status")
    if len(body) > 4 * 1024 * 1024:
        raise ValueError("HTTP response exceeded evidence bound")
    return HttpReply(status=int(status), body=body)


def https_post(
    target: HttpsEndpoint, body: bytes, headers: dict[str, str]
) -> HttpReply:
    require_system_curl()
    arguments = curl_post_arguments(target, headers)
    # Use ordinary system trust; do not inherit custom CA or TLS-secret outputs.
    environment = {
        name: value
        for name, value in os.environ.items()
        if name
        not in {
            "CURL_CA_BUNDLE",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "SSLKEYLOGFILE",
            "CURL_SSL_BACKEND",
        }
    }
    return curl_http_reply(
        subprocess.run(
            arguments,
            input=body,
            capture_output=True,
            timeout=6,
            check=False,
            env=environment,
        )
    )


def http_post(
    target: HttpsEndpoint | LoopbackEndpoint | Path,
    payload: JsonObject,
    probe_headers: ProbeHeaders | None = None,
) -> HttpReply:
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
    body = json.dumps(payload)
    if isinstance(target, HttpsEndpoint):
        return https_post(target, body.encode(), headers)
    if isinstance(target, Path):
        connection = UnixHttpConnection(target)
    else:
        url = urllib.parse.urlsplit(target.url)
        connection = http.client.HTTPConnection(url.hostname or "", url.port, timeout=5)
    try:
        connection.request("POST", "/mcp", body, headers)
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


def tool_payload(name: str, arguments: JsonObject) -> JsonObject:
    return {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }


def tool_result(reply: HttpReply) -> JsonObject:
    result = rpc_result(reply)
    return read_observed_json(json.dumps(result.get("structuredContent")))


def tool(target: HttpsEndpoint | Path, name: str, arguments: JsonObject) -> JsonObject:
    return tool_result(http_post(target, tool_payload(name, arguments)))


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
        or record.get("placement")
        != {"kind": "thread", "rootMessageId": marker.root_message_id}
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


def run_peer(
    plan: BoardPlan,
    request: t.Callable[
        [HttpsEndpoint, JsonObject, ProbeHeaders | None], HttpReply
    ] = http_post,
) -> None:
    """One write only: save intent first, then preserve unknown outcomes without replay."""
    require_deadline(plan.deadline)
    require_private_directory(plan.evidence)

    def request_tool(name: str, arguments: JsonObject) -> JsonObject:
        return tool_result(request(plan.https, tool_payload(name, arguments), None))

    require_service(
        request_tool("endpoints_list", {}),
        plan.destination_service_id,
        plan.destination_service_epoch,
    )
    payload: JsonObject = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {},
    }
    tools = rpc_result(request(plan.https, payload, None))
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
        negative = request(plan.https, payload, ProbeHeaders(origin=origin))
        if negative.status != 403:
            raise ValueError("remote Origin was not rejected")
        origin_statuses[origin] = negative.status
    thread_root = request_tool(
        "board_message_show", {"messageId": plan.marker.root_message_id}
    )
    if (
        thread_root.get("messageId") != plan.marker.root_message_id
        or thread_root.get("topicId") != plan.marker.topic_id
        or thread_root.get("placement")
        != {"kind": "topic", "topicId": plan.marker.topic_id}
        or thread_root.get("actor") != plan.actor
    ):
        raise ValueError("caller-seeded thread root/topic/actor mismatch")
    post_arguments: JsonObject = {
        "messageId": plan.marker.message_id,
        "placement": {"kind": "thread", "rootMessageId": plan.marker.root_message_id},
        "actor": plan.actor,
        "text": plan.marker.text,
        "references": [],
    }
    post_payload = tool_payload("board_message_post", post_arguments)
    save_record(
        plan.evidence / "peer-intent.json",
        {
            "serviceId": plan.destination_service_id,
            "marker": plan.marker.model_dump(),
            "tools": tools,
            "originStatuses": origin_statuses,
            "threadRoot": thread_root,
            "postArguments": post_arguments,
        },
    )
    try:
        response = request(plan.https, post_payload, None)
    except Exception as failure:
        save_record(
            plan.evidence / "peer-post-transport-error.json",
            {
                "arguments": post_arguments,
                "error": str(failure),
                "errorType": type(failure).__name__,
                "outcome": "unknown; do not replay",
            },
        )
        raise
    save_record(
        plan.evidence / "peer-post-response.json",
        {
            "arguments": post_arguments,
            "httpStatus": response.status,
            "bodyBase64": base64.b64encode(response.body).decode("ascii"),
        },
    )
    posted = tool_result(response)
    require_marker(
        read_observed_json(json.dumps(posted.get("message"))), plan.marker, plan.actor
    )
    observed = request_tool("board_message_show", {"messageId": plan.marker.message_id})
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
                "SELECT m.message_id,m.topic_id,m.text,i.kind,i.service_id,i.endpoint_id,i.session_id,m.root_id FROM board_messages m JOIN board_identities i ON i.identity_key=m.actor_key WHERE m.message_id=?",
                (marker.message_id,),
            ).fetchone(),
        )
    stored = TypeAdapter(tuple[str, str, str, str, str, str, str, str]).validate_python(
        row
    )
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
            "placement": {"kind": "thread", "rootMessageId": stored[7]},
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
