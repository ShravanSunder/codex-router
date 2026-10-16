#!/usr/bin/env python3
"""Serve a labelled harness UI from scoped genuine evidence, never a native UI claim."""

import argparse
import json
import typing as t
from html import escape
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from pydantic import BaseModel

from scripts.verification.typed_observations import (
    HarnessClaims,
    HostManifest,
    RouterStarted,
    read_observed_json,
)


class ReportArguments(BaseModel):
    evidence: Path
    port: int


def report_html(evidence: Path) -> bytes:
    claims = HarnessClaims.model_validate_json((evidence / "claims.json").read_text())
    manifest = HostManifest.model_validate_json(
        (evidence / "host-A-ready.json").read_text()
    )
    started = RouterStarted.model_validate_json(
        (evidence / "router-started.json").read_text()
    )
    details = {
        title: read_observed_json((evidence / relative).read_text())
        for title, relative in claims.details.items()
    }
    sections = "".join(
        f"<section><h2>{escape(title)}</h2><pre>{escape(json.dumps(value, indent=2))}</pre></section>"
        for title, value in details.items()
    )
    rows = "".join(
        "<tr>" + "".join(f"<td>{escape(str(value))}</td>" for value in row) + "</tr>"
        for row in claims.rows
    )
    return f"""<!doctype html><html><meta charset="utf-8"><title>Codex Router verification harness</title>
<style>body{{font:17px system-ui;margin:36px;max-width:1180px;color:#17212b;background:white}}h1{{font-size:30px}}header{{border-bottom:3px solid #17212b;padding-bottom:20px}}table{{border-collapse:collapse;width:100%;margin:24px 0}}td,th{{padding:12px;text-align:left;border-bottom:1px solid #bbc4ce}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px;background:#edf3fa;padding:18px}}section{{margin-top:30px}}.gap{{color:#9b3019}}</style>
<header><h1>Codex Router — actual verification evidence</h1><p>HARNESS UI SCREENSHOT · {escape(manifest.machineLabel)} · not a native app screenshot · no delivery acceptance</p><p>Build {escape(manifest.serviceVersion)} / Router {escape(started.binarySHA256[:12])} · source {escape(started.head[:12])}<br>Host A {escape(manifest.serviceId)} · epoch {escape(manifest.serviceEpoch)}</p></header>
<table><tr><th>Claim</th><th>Observation</th><th>Conclusion</th></tr>{rows}</table>
<p>{escape(claims.scope)}</p>{sections}</html>""".encode()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("evidence", type=Path)
    _ = parser.add_argument("--port", type=int, default=0)
    arguments = ReportArguments.model_validate(vars(parser.parse_args()))
    content = report_html(arguments.evidence)
    _ = (arguments.evidence / "harness-report.html").write_bytes(content)

    class ReportHandler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            if self.path != "/":
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            _ = self.wfile.write(content)

        @t.override
        def log_message(self, format: str, *args: object) -> None:
            return

    with HTTPServer(("127.0.0.1", arguments.port), ReportHandler) as server:
        print(
            json.dumps(
                {
                    "url": f"http://127.0.0.1:{server.server_port}/",
                    "label": "harness UI, not native app",
                }
            ),
            flush=True,
        )
        server.serve_forever()
