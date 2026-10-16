"""Safety and observation checks for the apparatus; never Tailnet product proof."""

import datetime as dt
import hashlib
import os
import socket
import socketserver
import sqlite3
import subprocess
import sys
import tempfile
import threading
import typing as t
import unittest
from contextlib import closing
from pathlib import Path

from scripts.verification import tailnet_mcp_client as client
from scripts.verification import tailnet_mcp_host as host
from scripts.verification import tailnet_mcp_models as models
from scripts.verification.owned_runtime import process_table
from scripts.verification.typed_observations import (
    JsonObject,
    ProcessIdentity,
    read_observed_json,
)


def harmless_child() -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stdin.buffer.read()"],
        stdin=subprocess.PIPE,
        start_new_session=True,
    )


class LiteralMcpHandler(socketserver.StreamRequestHandler):
    request: socket.socket
    check_authority: t.ClassVar[bool] = False
    proxy_authority: t.ClassVar[str] = "node.tail.ts.net:8443"

    @t.override
    def handle(self) -> None:
        self.request.settimeout(5)
        request_line = self.rfile.readline()
        content_length = 0
        headers: dict[str, str] = {}
        while line := self.rfile.readline():
            if line == b"\r\n":
                break
            if line.lower().startswith(b"content-length:"):
                content_length = int(line.split(b":", 1)[1])
            name, value = line.decode().split(":", 1)
            headers[name.lower()] = value.strip()
        body = self.rfile.read(content_length)
        if request_line != b"POST /mcp HTTP/1.1\r\n" or b'"tools/list"' not in body:
            raise ValueError("apparatus sent the wrong real request")
        payload = b'{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}'
        status = 200
        if self.check_authority:
            authority = headers.get("host", "")
            origin = headers.get("origin")
            local_port = t.cast(tuple[str, int], self.request.getsockname())[1]
            if not (
                authority.startswith("127.0.0.1:")
                or authority == self.proxy_authority
                or (
                    self.proxy_authority.endswith(":443")
                    and authority == self.proxy_authority.removesuffix(":443")
                )
            ) or (origin is not None and origin != f"http://127.0.0.1:{local_port}"):
                status = 403
        _ = self.wfile.write(
            f"HTTP/1.1 {status} Result\r\nContent-Length: {len(payload)}\r\nConnection: close\r\n\r\n".encode()
            + payload
        )


@t.final
class AuthorityMcpHandler(LiteralMcpHandler):
    check_authority: t.ClassVar[bool] = True


@t.final
class Default443AuthorityHandler(LiteralMcpHandler):
    check_authority: t.ClassVar[bool] = True
    proxy_authority: t.ClassVar[str] = "node.tail.ts.net:443"


class TailnetGuardTests(unittest.TestCase):
    def test_manifest_mcp_accepts_actual_transport_and_url_wire_shape(self) -> None:
        manifest: JsonObject = {
            "version": 3,
            "mcp": {"transport": "streamableHttp", "url": "http://127.0.0.1:8788/mcp"},
        }
        self.assertEqual(
            host.parse_manifest_mcp(manifest).url, "http://127.0.0.1:8788/mcp"
        )

    def test_manifest_mcp_rejects_missing_wrong_transport_and_unknown_fields(
        self,
    ) -> None:
        selectors: list[JsonObject] = [
            {"url": "http://127.0.0.1:8788/mcp"},
            {"transport": "unixJsonLines", "url": "http://127.0.0.1:8788/mcp"},
            {
                "transport": "streamableHttp",
                "url": "http://127.0.0.1:8788/mcp",
                "unknown": True,
            },
        ]
        for selector in selectors:
            with self.subTest(selector=selector), self.assertRaises(ValueError):
                _ = host.parse_manifest_mcp({"mcp": selector})

    def test_backend_only_accepts_explicit_loopback_http(self) -> None:
        for url in [
            "https://127.0.0.1:1234/mcp",
            "http://100.100.100.1:1234/mcp",
            "http://node.ts.net:1234/mcp",
            "http://127.0.0.1/mcp",
            "http://127.0.0.1:1234/other",
        ]:
            with self.subTest(url=url), self.assertRaises(ValueError):
                _ = models.LoopbackEndpoint(url=url)
        _ = models.LoopbackEndpoint(url="http://127.0.0.1:1234/mcp")
        _ = models.LoopbackEndpoint(url="http://[::1]:1234/mcp")
        self.assertEqual(
            models.HttpsEndpoint(url="https://node.ts.net/mcp").authority,
            "node.ts.net:443",
        )

    def test_root_rejects_existing_nested_and_alias_paths(self) -> None:
        for root in [Path.home(), Path("/private/tmp/x/y"), Path("/tmp/x")]:
            with self.subTest(root=root), self.assertRaises(ValueError):
                host.validate_fresh_root(root)
        with (
            tempfile.TemporaryDirectory(dir="/private/tmp") as folder,
            self.assertRaises(ValueError),
        ):
            host.validate_fresh_root(Path(folder))

    def test_selected_ports_are_nonproduction_and_distinct(self) -> None:
        for port in [0, 65536, 19741, 18787, 43127]:
            with self.subTest(port=port), self.assertRaises(ValueError):
                models.validate_ports(port, 8443)
        models.validate_ports(43128, 8443)
        with self.assertRaises(ValueError):
            models.validate_ports(43128, 43128)

    def test_hash_mismatch_fails_before_executing_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            binary = Path(folder) / "binary"
            _ = binary.write_bytes(b"not executed")
            with self.assertRaisesRegex(ValueError, "hash"):
                host.require_hash(binary, "0" * 64)

    def test_https_endpoint_rejects_public_plaintext_and_redirect_targets(self) -> None:
        for url in [
            "http://node.ts.net/mcp",
            "https://public.example/mcp",
            "https://node.ts.net/not-mcp",
            "https://user@node.ts.net/mcp",
            "https://node.ts.net/mcp?token=x",
        ]:
            with self.subTest(url=url), self.assertRaises(ValueError):
                _ = models.HttpsEndpoint(url=url)
        _ = models.HttpsEndpoint(url="https://node.tail.ts.net:8443/mcp")

    def test_profile_conflict_is_left_untouched(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            profile = Path(folder) / "profile"
            original = b'base_url = "http://127.0.0.1:18787/v1"\n'
            _ = profile.write_bytes(original)
            with (
                self.assertRaisesRegex(RuntimeError, "profile conflict"),
                host.project_profile(
                    profile, hashlib.sha256(original).hexdigest(), 43128
                ),
            ):
                _ = profile.write_bytes(b"newer owner bytes")
            self.assertEqual(profile.read_bytes(), b"newer owner bytes")

    def test_profile_restores_exact_bytes_after_body_error(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            profile = Path(folder) / "profile"
            original = b'# retain spacing\nbase_url = "http://127.0.0.1:18787/v1"\n'
            _ = profile.write_bytes(original)
            with (
                self.assertRaisesRegex(RuntimeError, "body failed"),
                host.project_profile(
                    profile, hashlib.sha256(original).hexdigest(), 43128
                ),
            ):
                self.assertIn(b":43128/v1", profile.read_bytes())
                raise RuntimeError("body failed")
            self.assertEqual(profile.read_bytes(), original)

    def test_profile_refuses_symlink_and_initial_hash_conflict(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            profile = Path(folder) / "profile"
            _ = profile.write_bytes(b'base_url = "http://127.0.0.1:18787/v1"')
            alias = Path(folder) / "alias"
            alias.symlink_to(profile)
            for path in [profile, alias]:
                with (
                    self.subTest(path=path),
                    self.assertRaises((ValueError, OSError)),
                    host.project_profile(path, "0" * 64, 43128),
                ):
                    self.fail("unsafe profile accepted")

    def test_private_evidence_rejects_alias_and_shared_directory(self) -> None:
        with tempfile.TemporaryDirectory(dir="/private/tmp") as folder:
            private = Path(folder)
            client.require_private_directory(private)
            alias = private / "alias"
            alias.symlink_to(private, target_is_directory=True)
            with self.assertRaises(ValueError):
                client.require_private_directory(alias)
            private.chmod(0o755)
            with self.assertRaises(ValueError):
                client.require_private_directory(private)


class TailnetObservationTests(unittest.TestCase):
    def test_only_same_invocation_disabled_provider_startup_may_remain_pending(
        self,
    ) -> None:
        raw = """{"serviceEpoch":"epoch","sequence":2,"endpoints":[
          {"endpoint":{"serviceId":"destination","endpointId":"claude-local"},"label":"Claude Code",
           "availability":{"state":"unavailable","observedAt":"2026-10-16T00:00:00Z","reason":"disabled in providers.json","fix":"enable provider"},"channels":[]},
          {"endpoint":{"serviceId":"destination","endpointId":"cursor-local"},"label":"Cursor",
           "availability":{"state":"unavailable","observedAt":"2026-10-16T00:00:00Z","reason":"disabled in providers.json","fix":"enable provider"},"channels":[]}
        ]}"""
        starting = read_observed_json(raw)
        self.assertEqual(
            client.observe_native_readiness(starting, "destination", "epoch"),
            models.NativeReadiness.STARTING,
        )
        invalid: list[JsonObject] = [
            {**starting, "serviceEpoch": "wrong"},
            {**starting, "sequence": "bad"},
            {**starting, "endpoints": []},
            {**starting, "unknown": True},
            read_observed_json(raw.replace("destination", "wrong-service")),
            read_observed_json(
                raw.replace("disabled in providers.json", "backend retired")
            ),
            read_observed_json(raw.replace("2026-10-16T00:00:00Z", "invalid-time")),
        ]
        for inventory in invalid:
            with self.subTest(inventory=inventory), self.assertRaises(ValueError):
                _ = client.observe_native_readiness(inventory, "destination", "epoch")

    def test_http_probe_distinguishes_host_authority_from_browser_origin(self) -> None:
        for handler in [
            AuthorityMcpHandler,
            Default443AuthorityHandler,
            LiteralMcpHandler,
        ]:
            with socketserver.TCPServer(("127.0.0.1", 0), handler) as server:
                endpoint = models.LoopbackEndpoint(
                    url=f"http://127.0.0.1:{server.server_address[1]}/mcp"
                )
                remote = models.HttpsEndpoint(
                    url=f"https://{handler.proxy_authority}/mcp"
                )
                thread = threading.Thread(
                    target=server.serve_forever,
                    kwargs={"poll_interval": 0.01},
                    daemon=True,
                )
                thread.start()
                try:
                    if handler != LiteralMcpHandler:
                        statuses = client.observe_http_boundary(endpoint, remote)
                        self.assertEqual(statuses["proxyAuthorityControl"], 200)
                        self.assertEqual(statuses["tailnetOrigin"], 403)
                        self.assertEqual(statuses["wrongPort"], 403)
                        self.assertEqual(
                            statuses["missingPort"],
                            200 if handler == Default443AuthorityHandler else 403,
                        )
                    else:
                        with self.assertRaisesRegex(ValueError, "proxyHttpOrigin"):
                            _ = client.observe_http_boundary(endpoint, remote)
                finally:
                    server.shutdown()
                    thread.join(timeout=5)
                self.assertFalse(thread.is_alive())

    def test_serve_backend_must_be_the_owned_foreground_loopback_http_route(
        self,
    ) -> None:
        local = models.LoopbackEndpoint(url="http://127.0.0.1:43129/mcp")
        remote = models.HttpsEndpoint(url="https://node.tail.ts.net:8443/mcp")
        for proxy in [
            "http://127.0.0.1:43129",
            "unix:/tmp/socket",
            "http://127.0.0.1:9999",
        ]:
            status: JsonObject = {
                "Foreground": {
                    "owned": {
                        "TCP": {"8443": {"HTTPS": True}},
                        "Web": {
                            "node.tail.ts.net:8443": {
                                "Handlers": {"/": {"Proxy": proxy}}
                            }
                        },
                    }
                }
            }
            if proxy == "http://127.0.0.1:43129":
                self.assertEqual(
                    host.require_http_serve_backend(status, local, remote), "owned"
                )
            else:
                with self.assertRaises(ValueError):
                    _ = host.require_http_serve_backend(status, local, remote)

    def test_real_unix_transport_reaches_literal_mcp_response(self) -> None:
        with tempfile.TemporaryDirectory(dir="/private/tmp") as folder:
            endpoint = Path(folder) / "mcp.sock"
            with socketserver.UnixStreamServer(
                str(endpoint), LiteralMcpHandler
            ) as server:
                server.timeout = 5
                thread = threading.Thread(target=server.handle_request, daemon=True)
                thread.start()
                try:
                    response = client.http_post(
                        endpoint,
                        {
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "tools/list",
                            "params": {},
                        },
                    )
                    self.assertEqual(client.rpc_result(response), {"tools": []})
                finally:
                    thread.join(timeout=5)
                self.assertFalse(thread.is_alive())

    def test_inventory_rejects_wrong_service_and_missing_native_generation(
        self,
    ) -> None:
        native: JsonObject = {
            "endpoint": {"serviceId": "destination", "endpointId": "codex-local"},
            "availability": {"state": "available"},
            "channels": [
                {
                    "kind": "nativeCodex",
                    "generation": {"serviceEpoch": "epoch", "generation": 1},
                }
            ],
        }
        inventory: JsonObject = {"endpoints": [native], "serviceEpoch": "epoch"}
        client.require_service(inventory, "destination")
        self.assertEqual(
            client.observe_native_readiness(inventory, "destination", "epoch"),
            models.NativeReadiness.READY,
        )
        with self.assertRaisesRegex(ValueError, "service epoch"):
            _ = client.observe_native_readiness(
                inventory, "destination", "different-epoch"
            )
        with self.assertRaises(ValueError):
            _ = client.observe_native_readiness(inventory, "other-service", "epoch")
        native["channels"] = [{"kind": "nativeCodex"}]
        with self.assertRaises(ValueError):
            _ = client.observe_native_readiness(inventory, "destination", "epoch")
        native["availability"] = {"state": "unavailable"}
        with self.assertRaises(ValueError):
            _ = client.observe_native_readiness(inventory, "destination", "epoch")

    def test_serve_comparison_rejects_overwrite_funnel_and_stale_session(self) -> None:
        before: JsonObject = {"TCP": {"443": {"HTTPS": True}}}
        during: JsonObject = {
            "TCP": {"443": {"HTTPS": True}, "8443": {"HTTPS": True}},
            "Foreground": {"owned": {"Web": {"node:8443": {"Handlers": {}}}}},
        }
        host.require_serve_during(before, during, 8443)
        changes: list[JsonObject] = [
            {"TCP": {"443": {"HTTPS": False}}},
            {"AllowFunnel": {"node:8443": True}, **during},
        ]
        for changed in changes:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                host.require_serve_during(before, changed, 8443)
        with self.assertRaisesRegex(ValueError, "restored"):
            host.require_serve_restored(before, during)
        host.require_serve_restored(before, before)

    def test_mcp_error_and_wrong_correlation_never_count_as_result(self) -> None:
        for body in [
            b'{"jsonrpc":"2.0","id":2,"result":{}}',
            b'{"jsonrpc":"2.0","id":1,"error":{"code":-1}}',
            b'{"jsonrpc":"2.0","id":1,"result":{"isError":true}}',
        ]:
            with self.subTest(body=body), self.assertRaises(ValueError):
                _ = client.rpc_result(models.HttpReply(status=200, body=body))
        self.assertEqual(
            client.rpc_result(
                models.HttpReply(
                    status=200, body=b'{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}'
                )
            ),
            {"tools": []},
        )

    def test_local_readback_requires_the_exact_message_and_actor(self) -> None:
        marker = models.BoardMarker(
            topic_id="topic", message_id="message", text="unique"
        )
        actor: JsonObject = {
            "kind": "session",
            "session": {"sessionId": "real-commissioner"},
        }
        record: JsonObject = {
            "topicId": "topic",
            "messageId": "message",
            "text": "unique",
            "actor": actor,
        }
        client.require_marker(record, marker, actor)
        for field in ["messageId", "topicId", "text", "actor"]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                client.require_marker({**record, field: "wrong"}, marker, actor)

    def test_cleanup_reaps_a_real_owned_child_when_body_raises(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            child = harmless_child()
            try:
                with (
                    self.assertRaisesRegex(RuntimeError, "body failed"),
                    host.owned_child(child, Path(folder)),
                ):
                    raise RuntimeError("body failed")
                self.assertIsNotNone(child.poll())
                self.assertTrue((Path(folder) / "host-settlement.json").exists())
                snapshot_path = Path(folder) / "host-before-fallback.json"
                self.assertTrue(
                    snapshot_path.exists(), "pre-fallback evidence was not retained"
                )
                snapshot = read_observed_json(snapshot_path.read_bytes())
                self.assertEqual(snapshot["kind"], "observed")
                self.assertEqual(snapshot["recorded"], snapshot["matching"])
                self.assertTrue(snapshot["matching"])
            finally:
                if child.stdin is not None:
                    child.stdin.close()
                _ = child.wait(timeout=5)

    def test_deadline_requires_aware_future_time(self) -> None:
        for deadline in [
            dt.datetime(2000, 1, 1),
            dt.datetime(2000, 1, 1, tzinfo=dt.UTC),
        ]:
            with self.subTest(deadline=deadline), self.assertRaises(ValueError):
                models.require_deadline(deadline)

    def test_unobservable_cleanup_is_unknown_instead_of_empty(self) -> None:
        identity = process_table()[os.getpid()]

        def denied_process_read() -> dict[int, ProcessIdentity]:
            raise PermissionError("deliberate observation boundary failure")

        def empty_process_read() -> dict[int, ProcessIdentity]:
            return {}

        for observation in [denied_process_read, empty_process_read]:
            snapshot = host.capture_cleanup_inventory(
                {identity.pid: identity}, observation
            )
            self.assertEqual(snapshot.kind, "unavailable")
            self.assertNotIn("matching", snapshot.model_dump())
            self.assertEqual(snapshot.recorded, [identity])

    def test_exited_owned_child_has_an_independent_empty_pre_fallback_snapshot(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as folder:
            child = harmless_child()
            try:
                with host.owned_child(child, Path(folder)):
                    if child.stdin is None:
                        self.fail("fixture pipe absent")
                    child.stdin.close()
                    self.assertEqual(child.wait(timeout=5), 0)
                snapshot = models.CleanupInventoryObserved.model_validate_json(
                    (Path(folder) / "host-before-fallback.json").read_bytes()
                )
                self.assertTrue(snapshot.recorded)
                self.assertEqual(snapshot.matching, [])
            finally:
                if child.stdin is not None:
                    child.stdin.close()
                _ = child.wait(timeout=5)

    def test_evidence_write_failure_cannot_skip_owned_child_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            (Path(folder) / "host-before-fallback.json").mkdir()
            child = harmless_child()
            try:
                with (
                    self.assertRaisesRegex(
                        RuntimeError, "cleanup evidence unavailable"
                    ),
                    host.owned_child(child, Path(folder)),
                ):
                    pass
                self.assertIsNotNone(child.poll())
            finally:
                if child.stdin is not None:
                    child.stdin.close()
                _ = child.wait(timeout=5)

    def test_protected_process_mismatch_is_rejected_without_signalling(self) -> None:
        identity = process_table()[os.getpid()]
        host.require_protected([identity])
        with self.assertRaisesRegex(RuntimeError, "protected process"):
            host.require_protected(
                [identity.model_copy(update={"birth": "wrong birth"})]
            )

    def test_composed_error_reaps_child_before_restoring_exact_profile(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            evidence = Path(folder)
            profile = evidence / "profile"
            original = b'base_url = "http://127.0.0.1:18787/v1"\n'
            _ = profile.write_bytes(original)
            child = harmless_child()
            try:
                with (
                    self.assertRaisesRegex(RuntimeError, "composed error"),
                    host.project_profile(
                        profile, hashlib.sha256(original).hexdigest(), 43128
                    ),
                    host.owned_child(child, evidence),
                ):
                    raise RuntimeError("composed error")
                self.assertIsNotNone(child.poll())
                self.assertEqual(profile.read_bytes(), original)
            finally:
                if child.stdin is not None:
                    child.stdin.close()
                _ = child.wait(timeout=5)

    def test_independent_sql_readback_rejects_missing_or_changed_stored_marker(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as folder:
            database = Path(folder) / "board.sqlite"
            marker = models.BoardMarker(
                topic_id="topic", message_id="message", text="unique"
            )
            actor: JsonObject = {
                "kind": "session",
                "session": {
                    "endpoint": {"serviceId": "service", "endpointId": "codex-local"},
                    "sessionId": "commissioner",
                },
            }
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute(
                    "CREATE TABLE board_messages(message_id TEXT,topic_id TEXT,text TEXT,actor_key TEXT)"
                )
                _ = connection.execute(
                    "CREATE TABLE board_identities(identity_key TEXT,kind TEXT,service_id TEXT,endpoint_id TEXT,session_id TEXT)"
                )
                _ = connection.execute(
                    "INSERT INTO board_identities VALUES('actor','session','service','codex-local','commissioner')"
                )
                _ = connection.execute(
                    "INSERT INTO board_messages VALUES('message','topic','unique','actor')"
                )
            client.read_stored_marker(database, marker, actor)
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
