"""Tailnet HTTPS/board apparatus checks; private live journeys remain separate."""

import base64
import datetime as dt
import json
import os
import socket
import socketserver
import sqlite3
import ssl
import subprocess
import tempfile
import threading
import typing as t
import unittest
from contextlib import closing
from pathlib import Path

from scripts.verification import tailnet_mcp_client as client
from scripts.verification import tailnet_mcp_host as host
from scripts.verification import tailnet_mcp_models as models
from scripts.verification.typed_observations import JsonObject, read_observed_json


class CurlMcpHandler(socketserver.StreamRequestHandler):
    request: socket.socket
    response_status: t.ClassVar[int] = 200
    response_size: t.ClassVar[int] = 0
    explicit_length: t.ClassVar[bool] = True

    @t.override
    def handle(self) -> None:
        self.request.settimeout(5)
        request_line = self.rfile.readline()
        headers: dict[str, str] = {}
        while line := self.rfile.readline():
            if line == b"\r\n":
                break
            name, value = line.decode().split(":", 1)
            headers[name.lower()] = value.strip()
        body = self.rfile.read(int(headers.get("content-length", "0")))
        if (
            request_line != b"POST /mcp HTTP/1.1\r\n"
            or body != b'{"method": "tools/list"}'
            or headers.get("content-type") != "application/json"
            or headers.get("accept") != "application/json, text/event-stream"
            or headers.get("mcp-protocol-version") != "2025-11-25"
            or "user-agent" in headers
            or "expect" in headers
        ):
            raise ValueError("curl changed the real request body, framing or headers")
        status = self.response_status
        if headers.get("host") != "node.tail.ts.net:8443" or "origin" in headers:
            status = 403
        payload = (
            b"x" * self.response_size
            if self.response_size
            else b'{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}'
        )
        length = f"Content-Length: {len(payload)}\r\n" if self.explicit_length else ""
        try:
            _ = self.wfile.write(
                f"HTTP/1.1 {status} Result\r\n{length}Location: /mcp\r\nConnection: close\r\n\r\n".encode()
                + payload
            )
        except (BrokenPipeError, ConnectionResetError):
            if not self.response_size:
                raise


class TailnetHttpsTransportTests(unittest.TestCase):
    def test_curl_request_policy_preserves_authority_origin_and_safe_defaults(
        self,
    ) -> None:
        client.require_system_curl()
        target = models.HttpsEndpoint(url="https://node.tail.ts.net:8443/mcp")
        headers = {"Host": target.authority, "Origin": "https://foreign.invalid"}
        arguments = client.curl_post_arguments(target, headers)
        self.assertEqual(arguments[:2], ["/usr/bin/curl", "-q"])
        for option, value in {
            "--proto": "=https",
            "--noproxy": "*",
            "--proxy": "",
            "--retry": "0",
            "--max-redirs": "0",
            "--connect-timeout": "65",
            "--max-time": "75",
            "--max-filesize": "4194304",
            "--data-binary": "@-",
            "--url": target.url,
        }.items():
            self.assertEqual(arguments[arguments.index(option) + 1], value)
        self.assertIn("--no-location", arguments)
        self.assertIn("Host: node.tail.ts.net:8443", arguments)
        self.assertIn("Origin: https://foreign.invalid", arguments)
        self.assertTrue(
            {
                "--insecure",
                "--cacert",
                "--capath",
                "--location",
                "--compressed",
            }.isdisjoint(arguments)
        )
        for value in ["https://foreign.invalid\r\nInjected: yes", "bad\0value"]:
            with self.assertRaisesRegex(ValueError, "header"):
                _ = client.curl_post_arguments(target, {"Origin": value})

    def test_curl_keeps_http_errors_and_rejects_transport_or_malformed_results(
        self,
    ) -> None:
        for status in [200, 302, 403]:
            reply = client.curl_http_reply(
                subprocess.CompletedProcess(
                    [], 0, b"body\n" + str(status).encode(), b""
                )
            )
            self.assertEqual((reply.status, reply.body), (status, b"body"))
        for code in [1, 28, 60, 63]:
            with (
                self.subTest(code=code),
                self.assertRaisesRegex(RuntimeError, f"exit {code}"),
            ):
                _ = client.curl_http_reply(
                    subprocess.CompletedProcess(
                        [], code, b"partial\n200", b"literal failure"
                    )
                )
        for output in [
            b"",
            b"body\n000",
            b"body\n20x",
            b"x" * (4194304 + 1) + b"\n200",
        ]:
            with self.assertRaises(ValueError):
                _ = client.curl_http_reply(
                    subprocess.CompletedProcess([], 0, output, b"")
                )

    def test_real_curl_http_fixture_checks_post_origin_redirect_and_size(self) -> None:
        class RedirectHandler(CurlMcpHandler):
            response_status: t.ClassVar[int] = 302

        class SizedHandler(CurlMcpHandler):
            response_size: t.ClassVar[int] = 4194305

        class UnknownSizeHandler(SizedHandler):
            explicit_length: t.ClassVar[bool] = False

        # HTTP-only fixtures qualify curl framing and limits, not HTTPS acceptance.
        for handler, origin, expected in [
            (CurlMcpHandler, None, 200),
            (CurlMcpHandler, "https://node.tail.ts.net:8443", 403),
            (CurlMcpHandler, "https://foreign.invalid", 403),
            (RedirectHandler, None, 302),
            (SizedHandler, None, 63),
            (UnknownSizeHandler, None, 63),
        ]:
            with (
                self.subTest(handler=handler, origin=origin),
                socketserver.TCPServer(("127.0.0.1", 0), handler) as server,
            ):
                target = models.HttpsEndpoint(url="https://node.tail.ts.net:8443/mcp")
                headers = {
                    "Host": target.authority,
                    "Content-Type": "application/json",
                    "Accept": "application/json, text/event-stream",
                    "MCP-Protocol-Version": "2025-11-25",
                }
                if origin is not None:
                    headers["Origin"] = origin
                arguments = client.curl_post_arguments(target, headers)
                arguments[arguments.index("=https")] = "=http"
                arguments[arguments.index(target.url)] = (
                    f"http://127.0.0.1:{server.server_address[1]}/mcp"
                )
                server.timeout = 5
                thread = threading.Thread(target=server.handle_request, daemon=True)
                thread.start()
                try:
                    completed = subprocess.run(
                        arguments,
                        input=json.dumps({"method": "tools/list"}).encode(),
                        env={
                            **os.environ,
                            "ALL_PROXY": "http://127.0.0.1:1",
                            "HTTP_PROXY": "http://127.0.0.1:1",
                            "HTTPS_PROXY": "http://127.0.0.1:1",
                            "NO_PROXY": "",
                            "no_proxy": "",
                        },
                        capture_output=True,
                        timeout=6,
                        check=False,
                    )
                    self.assertLessEqual(len(completed.stdout), 4194304 + 4)
                    if expected == 63:
                        self.assertEqual(completed.returncode, 63)
                        with self.assertRaisesRegex(RuntimeError, "exit 63"):
                            _ = client.curl_http_reply(completed)
                    else:
                        reply = client.curl_http_reply(completed)
                        self.assertEqual(reply.status, expected)
                        self.assertIn(b'"tools":[]', reply.body)
                finally:
                    thread.join(timeout=6)
                self.assertFalse(thread.is_alive())

    def test_real_curl_rejects_untrusted_local_tls_without_a_trust_override(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as folder:
            key, certificate = Path(folder) / "key.pem", Path(folder) / "cert.pem"
            _ = subprocess.run(
                [
                    "/usr/bin/openssl",
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-keyout",
                    str(key),
                    "-out",
                    str(certificate),
                    "-days",
                    "1",
                    "-subj",
                    "/CN=node.tail.ts.net",
                ],
                capture_output=True,
                check=True,
                timeout=10,
            )
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(certificate, key)
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                listener.listen(1)
                listener.settimeout(5)
                port = t.cast(tuple[str, int], listener.getsockname())[1]

                def reject_handshake() -> None:
                    connection = listener.accept()[0]
                    with connection:
                        connection.settimeout(5)
                        try:
                            with context.wrap_socket(connection, server_side=True):
                                pass
                        except ssl.SSLError:
                            pass

                target = models.HttpsEndpoint(
                    url=f"https://node.tail.ts.net:{port}/mcp"
                )
                arguments = client.curl_post_arguments(
                    target, {"Host": target.authority}
                )
                arguments.extend(["--resolve", f"node.tail.ts.net:{port}:127.0.0.1"])
                thread = threading.Thread(target=reject_handshake, daemon=True)
                thread.start()
                try:
                    completed = subprocess.run(
                        arguments,
                        input=b"{}",
                        capture_output=True,
                        check=False,
                        timeout=6,
                    )
                    self.assertEqual(completed.returncode, 60)
                    with self.assertRaisesRegex(RuntimeError, "exit 60"):
                        _ = client.curl_http_reply(completed)
                finally:
                    thread.join(timeout=6)
                self.assertFalse(thread.is_alive())


def board_marker() -> models.BoardMarker:
    return models.BoardMarker(
        topic_id="01900000-0000-7000-8000-000000000001",
        root_message_id="01900000-0000-7000-8000-000000000002",
        message_id="01900000-0000-7000-8000-000000000003",
        text="unique",
    )


def board_actor() -> JsonObject:
    return read_observed_json("""{"kind":"session","session":{
      "endpoint":{"serviceId":"01900000-0000-7000-8000-000000000010","endpointId":"codex-local"},
      "sessionId":"01900000-0000-7000-8000-000000000011"}}
    """)


def board_plan(evidence: Path) -> models.BoardPlan:
    return models.BoardPlan(
        destination_root=evidence,
        evidence=evidence,
        https=models.HttpsEndpoint(url="https://node.tail.ts.net:8443/mcp"),
        destination_service_id="destination",
        destination_service_epoch="epoch",
        actor=board_actor(),
        marker=board_marker(),
        deadline=dt.datetime.now(dt.UTC) + dt.timedelta(hours=1),
    )


class BoardPeerFixture:
    def __init__(self, plan: models.BoardPlan, mode: str) -> None:
        self.plan: models.BoardPlan = plan
        self.mode: str = mode
        self.posts: int = 0

    def __call__(
        self,
        target: models.HttpsEndpoint,
        payload: JsonObject,
        headers: models.ProbeHeaders | None,
    ) -> models.HttpReply:
        if target != self.plan.https:
            raise ValueError("fixture reached wrong destination")
        if payload.get("method") == "tools/list":
            status = 403 if headers is not None and headers.origin is not None else 200
            result: JsonObject = {
                "tools": [
                    {"name": name}
                    for name in [
                        "endpoints_list",
                        "board_message_post",
                        "board_message_show",
                    ]
                ]
            }
        else:
            params = read_observed_json(json.dumps(payload.get("params")))
            name = params.get("name")
            arguments = read_observed_json(json.dumps(params.get("arguments")))
            status = 200
            marker = self.plan.marker
            record: JsonObject = {
                "messageId": marker.message_id,
                "topicId": marker.topic_id,
                "placement": {
                    "kind": "thread",
                    "rootMessageId": marker.root_message_id,
                },
                "actor": self.plan.actor,
                "text": marker.text,
            }
            if name == "endpoints_list":
                result = read_observed_json("""{"serviceEpoch":"epoch","endpoints":[
                  {"endpoint":{"serviceId":"destination","endpointId":"codex-local"},
                   "availability":{"state":"available"},
                   "channels":[{"kind":"nativeCodex","generation":{"serviceEpoch":"epoch","generation":1}}]}
                ]}""")
            elif name == "board_message_post":
                self.posts += 1
                expected: JsonObject = {
                    "messageId": marker.message_id,
                    "placement": {
                        "kind": "thread",
                        "rootMessageId": marker.root_message_id,
                    },
                    "actor": self.plan.actor,
                    "text": marker.text,
                    "references": [],
                }
                intent = read_observed_json(
                    (self.plan.evidence / "peer-intent.json").read_bytes()
                )
                if arguments != expected or intent.get("postArguments") != expected:
                    raise ValueError(
                        "wrong placement or missing durable arguments before post"
                    )
                if self.mode == "transport":
                    raise TimeoutError("literal ambiguous transport failure")
                if self.mode == "invalid":
                    return models.HttpReply(status=502, body=b"invalid MCP body\xff")
                if self.mode == "error":
                    return models.HttpReply(
                        status=200,
                        body=b'{"jsonrpc":"2.0","id":1,"result":{"isError":true,"content":[{"type":"text","text":"literal refusal"}]}}',
                    )
                result = {"structuredContent": {"message": record}}
            elif name == "board_message_show":
                if arguments.get("messageId") == marker.root_message_id:
                    record = {
                        "messageId": marker.root_message_id,
                        "topicId": marker.topic_id,
                        "placement": {"kind": "topic", "topicId": marker.topic_id},
                        "actor": self.plan.actor,
                    }
                    if self.mode == "wrong-root":
                        record["topicId"] = "01900000-0000-7000-8000-000000000009"
                elif self.mode == "wrong-thread":
                    record["placement"] = {
                        "kind": "thread",
                        "rootMessageId": "01900000-0000-7000-8000-000000000009",
                    }
                result = {"structuredContent": record}
            else:
                raise ValueError("unexpected fixture tool")
            if name == "endpoints_list":
                result = {"structuredContent": result}
        return models.HttpReply(
            status=status,
            body=json.dumps({"jsonrpc": "2.0", "id": 1, "result": result}).encode(),
        )


class TailnetBoardPeerTests(unittest.TestCase):
    def test_deadline_requires_aware_future_time(self) -> None:
        past = dt.datetime(2000, 1, 1)
        for deadline in [past, past.replace(tzinfo=dt.UTC)]:
            with self.subTest(deadline=deadline), self.assertRaises(ValueError):
                models.require_deadline(deadline)

    def test_initial_readiness_covers_cold_start_without_extending_allocation(
        self,
    ) -> None:
        utc_now = dt.datetime(2026, 10, 16, tzinfo=dt.UTC)
        for remaining, expected_until in [
            (1000, 1150),
            (150, 1150),
            (45, 1045),
            (0, 1000),
            (-1, 999),
        ]:
            with self.subTest(remaining=remaining):
                until = host.host_readiness_until(
                    utc_now + dt.timedelta(seconds=remaining), 1000, utc_now
                )
                self.assertEqual(until, expected_until)
                self.assertLessEqual(until, 1000 + remaining)
                if remaining == 1000:
                    # A valid late initial observation may cross the old120s cap.
                    self.assertLess(1125, until)

    def test_allocation_expiry_after_preflight_prevents_the_only_write(self) -> None:
        with tempfile.TemporaryDirectory(
            dir=Path(tempfile.gettempdir()).resolve()
        ) as folder:
            plan = board_plan(Path(folder))
            fixture = BoardPeerFixture(plan, "success")
            original = models.require_deadline

            def allocation_clock(deadline: dt.datetime) -> None:
                if (plan.evidence / "peer-intent.json").exists():
                    deadline = dt.datetime(2000, 1, 1, tzinfo=dt.UTC)
                original(deadline)

            with self.assertRaisesRegex(ValueError, "expired"):
                client.run_peer(plan, fixture, allocation_clock)
            self.assertEqual(fixture.posts, 0)
            self.assertFalse((plan.evidence / "peer-post-response.json").exists())

    def test_marker_ids_require_canonical_rfc_uuidv7(self) -> None:
        valid = board_marker().model_dump()
        for field in ["topic_id", "message_id", "root_message_id"]:
            for invalid in [
                "topic",
                "01900000-0000-4000-8000-000000000001",
                "01900000-0000-7000-0000-000000000001",
                "01900000-0000-7ABC-8000-000000000001",
            ]:
                with (
                    self.subTest(field=field, invalid=invalid),
                    self.assertRaises(ValueError),
                ):
                    _ = models.BoardMarker.model_validate({**valid, field: invalid})

    def test_peer_uses_seeded_thread_retains_failures_and_never_replays(self) -> None:
        for mode in [
            "success",
            "error",
            "invalid",
            "transport",
            "wrong-thread",
            "wrong-root",
        ]:
            with (
                self.subTest(mode=mode),
                tempfile.TemporaryDirectory(
                    dir=Path(tempfile.gettempdir()).resolve()
                ) as folder,
            ):
                plan = board_plan(Path(folder))
                fixture = BoardPeerFixture(plan, mode)
                if mode == "success":
                    client.run_peer(plan, fixture)
                    self.assertTrue((plan.evidence / "peer-result.json").exists())
                else:
                    with self.assertRaises((ValueError, TimeoutError)):
                        client.run_peer(plan, fixture)
                    self.assertFalse((plan.evidence / "peer-result.json").exists())
                if mode == "wrong-root":
                    self.assertEqual(fixture.posts, 0)
                    self.assertFalse((plan.evidence / "peer-intent.json").exists())
                    continue
                self.assertEqual(fixture.posts, 1)
                if mode == "transport":
                    self.assertTrue(
                        (plan.evidence / "peer-post-transport-error.json").exists()
                    )
                else:
                    raw = read_observed_json(
                        (plan.evidence / "peer-post-response.json").read_bytes()
                    )
                    encoded = raw.get("bodyBase64")
                    if not isinstance(encoded, str):
                        self.fail("raw MCP response bytes were not retained")
                    body = base64.b64decode(encoded, validate=True)
                    intent = read_observed_json(
                        (plan.evidence / "peer-intent.json").read_bytes()
                    )
                    self.assertEqual(raw["arguments"], intent["postArguments"])
                    if mode == "error":
                        self.assertIn(b'"isError":true', body)
                    elif mode == "invalid":
                        self.assertEqual(
                            (raw["httpStatus"], body), (502, b"invalid MCP body\xff")
                        )
                with self.assertRaises(FileExistsError):
                    client.run_peer(plan, fixture)
                self.assertEqual(fixture.posts, 1)

    def test_local_readback_requires_the_exact_message_and_actor(self) -> None:
        marker = board_marker()
        actor = board_actor()
        record: JsonObject = {
            "topicId": marker.topic_id,
            "messageId": marker.message_id,
            "placement": {"kind": "thread", "rootMessageId": marker.root_message_id},
            "text": "unique",
            "actor": actor,
        }
        client.require_marker(record, marker, actor)
        for field in ["messageId", "topicId", "text", "actor", "placement"]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                client.require_marker({**record, field: "wrong"}, marker, actor)

    def test_independent_sql_readback_rejects_missing_or_changed_stored_marker(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as folder:
            database = Path(folder) / "board.sqlite"
            marker = board_marker()
            actor = board_actor()
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute(
                    "CREATE TABLE board_messages(message_id TEXT,topic_id TEXT,text TEXT,actor_key TEXT,root_id TEXT)"
                )
                _ = connection.execute(
                    "CREATE TABLE board_identities(identity_key TEXT,kind TEXT,service_id TEXT,endpoint_id TEXT,session_id TEXT)"
                )
                _ = connection.execute(
                    "INSERT INTO board_identities VALUES('actor','session',?,'codex-local',?)",
                    (
                        "01900000-0000-7000-8000-000000000010",
                        "01900000-0000-7000-8000-000000000011",
                    ),
                )
                _ = connection.execute(
                    "INSERT INTO board_messages VALUES(?,?,?,'actor',?)",
                    (
                        marker.message_id,
                        marker.topic_id,
                        marker.text,
                        marker.root_message_id,
                    ),
                )
            client.read_stored_marker(database, marker, actor)
            for wrong_root in [None, "01900000-0000-7000-8000-000000000009"]:
                with closing(sqlite3.connect(database)) as connection, connection:
                    _ = connection.execute(
                        "UPDATE board_messages SET root_id=?", (wrong_root,)
                    )
                with self.assertRaises(ValueError):
                    client.read_stored_marker(database, marker, actor)
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute(
                    "UPDATE board_messages SET root_id=?", (marker.root_message_id,)
                )
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute("UPDATE board_messages SET text='wrong'")
            with self.assertRaises(ValueError):
                client.read_stored_marker(database, marker, actor)
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute("DELETE FROM board_messages")
            with self.assertRaises(ValueError):
                client.read_stored_marker(database, marker, actor)


if __name__ == "__main__":
    _ = unittest.main()
