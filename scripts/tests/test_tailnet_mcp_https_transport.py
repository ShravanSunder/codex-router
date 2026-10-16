"""System curl apparatus checks; successful private HTTPS remains a live gate."""

import json
import os
import socket
import socketserver
import ssl
import subprocess
import tempfile
import threading
import typing as t
import unittest
from pathlib import Path

from scripts.verification import tailnet_mcp_client as client
from scripts.verification import tailnet_mcp_models as models


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
            "--connect-timeout": "5",
            "--max-time": "5",
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


if __name__ == "__main__":
    _ = unittest.main()
