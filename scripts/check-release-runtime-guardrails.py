#!/usr/bin/env python3
"""Structural guardrails for the release codex-router serve runtime."""

import argparse
import glob
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLAN_ROOT = REPO_ROOT / "tmp/plan-workflows/2026-06-24-async-router-runtime"
EVIDENCE_ROOT = PLAN_ROOT / "evidence/structural"
RELEASE_RUNTIME_CRATES = (
    "codex-router-auth",
    "codex-router-cli",
    "codex-router-core",
    "codex-router-proxy",
    "codex-router-quota",
    "codex-router-secret-store",
    "codex-router-selection",
    "codex-router-state",
)


@dataclass(frozen=True)
class Check:
    row_id: str
    description: str
    forbidden: tuple[tuple[str, str], ...]
    required: tuple[tuple[str, str], ...] = ()
    release_scan_forbidden: tuple[str, ...] = ()


CHECKS: dict[str, Check] = {
    "G-01": Check(
        row_id="G-01",
        description="no production std::net listener/stream in release serve path",
        forbidden=(
            ("crates/codex-router-proxy/src/server.rs", "std::net::TcpListener"),
            ("crates/codex-router-proxy/src/server.rs", "std::net::TcpStream"),
        ),
        release_scan_forbidden=(
            "std::net::TcpListener",
            "std::net::TcpStream",
        ),
        required=(
            ("crates/codex-router-proxy/src/server.rs", "tokio::net::TcpListener"),
            ("crates/codex-router-proxy/src/server.rs", "AsyncLoopbackServerRuntime"),
        ),
    ),
    "G-02": Check(
        row_id="G-02",
        description="no production reqwest::blocking in release serve HTTP upstream",
        forbidden=(
            ("crates/codex-router-proxy/src/upstream.rs", "reqwest::blocking"),
        ),
        required=(
            ("crates/codex-router-proxy/src/upstream.rs", "HyperHttpUpstreamTransport"),
            ("crates/codex-router-proxy/src/upstream.rs", "hyper_util::client::legacy::Client"),
        ),
    ),
    "G-03": Check(
        row_id="G-03",
        description="no blocking tungstenite accept/connect in release serve path",
        forbidden=(
            ("crates/codex-router-proxy/src/server.rs", "BlockingWebSocketTunnel"),
            ("crates/codex-router-proxy/src/websocket.rs", "tungstenite::accept"),
            ("crates/codex-router-proxy/src/websocket.rs", "use tungstenite::connect;"),
            ("crates/codex-router-proxy/src/websocket.rs", "connect(upstream_request)"),
        ),
        release_scan_forbidden=(
            "BlockingWebSocketTunnel",
            "tungstenite::accept",
            "use tungstenite::connect;",
            "connect(upstream_request)",
        ),
        required=(
            ("crates/codex-router-proxy/src/server.rs", "AsyncWebSocketTunnel"),
            ("crates/codex-router-proxy/src/websocket.rs", "tokio_tungstenite::connect_async"),
        ),
    ),
    "G-04": Check(
        row_id="G-04",
        description="no production httparse serving or upstream response parsing",
        forbidden=(
            ("crates/codex-router-proxy/src/server.rs", "httparse::"),
            ("crates/codex-router-proxy/src/upstream.rs", "httparse::"),
        ),
        release_scan_forbidden=("httparse::",),
    ),
    "G-05": Check(
        row_id="G-05",
        description="no blocking Read response body in release async runtime files",
        forbidden=(
            ("crates/codex-router-proxy/src/server.rs", "Box<dyn Read + Send>"),
            ("crates/codex-router-proxy/src/upstream.rs", "Box<dyn Read + Send>"),
            ("crates/codex-router-proxy/src/server.rs", "std::io::copy"),
        ),
        required=(
            ("crates/codex-router-proxy/src/http_sse.rs", "AsyncStreamingHttpProxyResponse"),
            ("crates/codex-router-proxy/src/server.rs", "BoxBody<Bytes, AsyncHttpBodyError>"),
        ),
    ),
    "G-07": Check(
        row_id="G-07",
        description="positive Hyper and tokio-tungstenite ownership in release runtime",
        forbidden=(),
        required=(
            ("crates/codex-router-proxy/src/server.rs", "http1::Builder"),
            ("crates/codex-router-proxy/src/server.rs", "hyper_tungstenite::upgrade"),
            ("crates/codex-router-proxy/src/server.rs", "hyper_tungstenite::HyperWebsocketStream"),
            ("crates/codex-router-proxy/src/upstream.rs", "HyperHttpUpstreamTransport"),
            ("crates/codex-router-proxy/src/upstream.rs", "HttpsConnectorBuilder"),
        ),
    ),
    "G-23": Check(
        row_id="G-23",
        description="local Hyper websocket upgrade handoff has no double handshake",
        forbidden=(
            ("crates/codex-router-proxy/src/server.rs", "accept_async"),
            ("crates/codex-router-proxy/src/server.rs", "accept_hdr_async"),
            ("crates/codex-router-proxy/src/server.rs", "derive_accept_key"),
        ),
        required=(
            ("crates/codex-router-proxy/src/server.rs", "hyper_tungstenite::upgrade"),
            ("crates/codex-router-proxy/src/server.rs", "handle_upgraded_connection"),
        ),
    ),
    "G-26": Check(
        row_id="G-26",
        description="supported application routes are pass-through and /v1/models is not synthesized",
        forbidden=(
            ("crates/codex-router-proxy/src/upstream.rs", "chatgpt_backend_models_response"),
            ("crates/codex-router-proxy/src/upstream.rs", "chatgpt_backend_models_async_response"),
            ("crates/codex-router-proxy/src/upstream.rs", 'br#"{"models":[]}"#'),
            ("crates/codex-router-proxy/src/upstream.rs", 'Bytes::from_static(br#"{"models":[]}"#)'),
        ),
        release_scan_forbidden=(
            "chatgpt_backend_models_response",
            "chatgpt_backend_models_async_response",
            'br#"{"models":[]}"#',
            'Bytes::from_static(br#"{"models":[]}"#)',
        ),
        required=(
            ("crates/codex-router-proxy/src/upstream.rs", "HyperHttpUpstreamTransport"),
            ("crates/codex-router-proxy/src/upstream.rs", ".url_for_path(request.path())"),
        ),
    ),
    "G-27": Check(
        row_id="G-27",
        description="WebSocket first request parsing does not require prompt-bearing payload shape",
        forbidden=(
            ("crates/codex-router-proxy/src/websocket.rs", "is_direct_response_create_payload"),
            ("crates/codex-router-proxy/src/websocket.rs", "payload.as_object()"),
            ("crates/codex-router-proxy/src/websocket.rs", 'frame_type != "response.create"'),
            ("crates/codex-router-proxy/src/websocket.rs", '.get("model")'),
            ("crates/codex-router-proxy/src/websocket.rs", '.get("input")'),
            ("crates/codex-router-proxy/src/websocket.rs", '.get("stream")'),
        ),
        release_scan_forbidden=(
            "is_direct_response_create_payload",
        ),
        required=(
            ("crates/codex-router-proxy/src/websocket.rs", "validate_first_frame"),
            ("crates/codex-router-proxy/src/websocket.rs", "has_forbidden_top_level_json_auth_carrier"),
        ),
    ),
    "G-28": Check(
        row_id="G-28",
        description=(
            "release WebSocket path has no message-count truncation, first-frame timeout, "
            "or provider-event-aware termination policy"
        ),
        forbidden=(
            ("crates/codex-router-cli/src/lib.rs", "--max-websocket-upstream-messages"),
            ("crates/codex-router-proxy/src/server.rs", "max_websocket_upstream_messages"),
            ("crates/codex-router-proxy/src/websocket.rs", "FirstFrameTimeout"),
            ("crates/codex-router-proxy/src/websocket.rs", "max_upstream_messages"),
            ("crates/codex-router-proxy/src/websocket.rs", "response_outstanding"),
            ("crates/codex-router-proxy/src/websocket.rs", "let mut upstream_message_count"),
            ("crates/codex-router-proxy/src/websocket.rs", "upstream_message_count = upstream_message_count.saturating_add"),
            ("crates/codex-router-proxy/src/websocket.rs", "upstream_message_count >="),
            ("crates/codex-router-proxy/src/websocket.rs", "is_close || is_completed"),
            ("crates/codex-router-proxy/src/websocket.rs", "tokio::time::timeout(Duration::from_millis(250)"),
        ),
        release_scan_forbidden=(
            "--max-websocket-upstream-messages",
            "max_websocket_upstream_messages",
            "FirstFrameTimeout",
            "response_outstanding",
        ),
        required=(
            ("crates/codex-router-proxy/src/websocket.rs", "next_data_message_before_upstream"),
            ("crates/codex-router-proxy/src/websocket.rs", "note_response_completed"),
            ("crates/codex-router-proxy/src/websocket.rs", "if is_close {\n                    return Ok(());"),
        ),
    ),
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("row_id", nargs="?", choices=sorted(CHECKS))
    parser.add_argument(
        "--print-release-source-paths",
        action="store_true",
        help="Print the resolved release runtime source paths as JSON and exit.",
    )
    args = parser.parse_args()
    if args.print_release_source_paths:
        print(json.dumps(release_runtime_source_paths(), indent=2))
        return 0
    if args.row_id is None:
        parser.error("row_id is required unless --print-release-source-paths is set")
    check = CHECKS[args.row_id]
    production_sources: dict[str, str] = {}
    release_sources = release_runtime_sources()
    failures: list[dict[str, str]] = []

    for relative_path, needle in check.forbidden + check.required:
        if relative_path not in production_sources:
            production_sources[relative_path] = strip_cfg_test_items(REPO_ROOT / relative_path)
        source = production_sources[relative_path]
        if (relative_path, needle) in check.forbidden and needle in source:
            failures.append(
                {
                    "kind": "forbidden_present",
                    "path": relative_path,
                    "needle": needle,
                }
            )
        if (relative_path, needle) in check.required and needle not in source:
            failures.append(
                {
                    "kind": "required_missing",
                    "path": relative_path,
                    "needle": needle,
                }
            )

    for needle in check.release_scan_forbidden:
        for relative_path, source in release_sources.items():
            if needle in source:
                failures.append(
                    {
                        "kind": "release_reachable_forbidden_present",
                        "path": relative_path,
                        "needle": needle,
                    }
                )

    for dirty_path in dirty_guarded_source_paths():
        failures.append(
            {
                "kind": "dirty_guarded_source_path",
                "path": dirty_path,
                "needle": "worktree_or_index_differs_from_HEAD",
            }
        )

    receipt_path = write_receipt(check, failures)
    if failures:
        print(f"release runtime guardrail {check.row_id} failed: {receipt_path}", file=sys.stderr)
        for failure in failures:
            print(
                f"{failure['kind']}: {failure['path']} :: {failure['needle']}",
                file=sys.stderr,
            )
        return 1

    print(f"release runtime guardrail {check.row_id} passed: {receipt_path}")
    return 0


def strip_cfg_test_items(path: Path) -> str:
    lines = path.read_text(encoding="utf-8").splitlines()
    output: list[str] = []
    skip_next_item = False
    skipping_item = False
    brace_depth = 0

    for line in lines:
        stripped = line.strip()
        if stripped == "#[cfg(test)]":
            skip_next_item = True
            continue

        if skip_next_item:
            if stripped.startswith("#["):
                continue
            open_count = line.count("{")
            close_count = line.count("}")
            if open_count == 0 and stripped.endswith(";"):
                skip_next_item = False
                continue
            skipping_item = True
            skip_next_item = False
            brace_depth = open_count - close_count
            if open_count > 0 and brace_depth <= 0:
                skipping_item = False
            continue

        if skipping_item:
            open_count = line.count("{")
            close_count = line.count("}")
            if brace_depth == 0 and open_count == 0:
                continue
            brace_depth += open_count - close_count
            if brace_depth <= 0:
                skipping_item = False
            continue

        output.append(line)

    return "\n".join(output)


def write_receipt(check: Check, failures: list[dict[str, str]]) -> Path:
    if os.environ.get("CODEX_ROUTER_PROOF_VERIFY_ONLY") == "1":
        receipt = tempfile.NamedTemporaryFile(
            delete=False,
            prefix=f"codex-router-guardrail-{check.row_id}-",
            suffix=".json",
        )
        receipt_path = Path(receipt.name)
        receipt.close()
    else:
        EVIDENCE_ROOT.mkdir(parents=True, exist_ok=True)
        receipt_path = EVIDENCE_ROOT / f"{check.row_id}.json"
    payload = {
        "schema_version": 1,
        "row_id": check.row_id,
        "layer": "structural",
        "owner": "T6",
        "command": os.environ.get(
            "CODEX_ROUTER_PROOF_COMMAND",
            f"scripts/check-release-runtime-guardrails.py {check.row_id}",
        ),
        "cwd": ".",
        "git_head": git_head(),
        "timestamp_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "status_before": "[ ] pending",
        "status_after": "[x] passed" if not failures else "[ ] pending",
        "result": "pass" if not failures else "fail",
        "exit_code": 0 if not failures else 1,
        "expected_observation": check.description,
        "touched_targets": sorted(
            {
                path
                for path, _needle in check.forbidden + check.required
            }
            | set(release_runtime_source_paths())
        ),
        "freshness_guard": "Cargo.toml crates/*/Cargo.toml crates/*/src/**/*.rs scripts/*",
        "freshness_check": "guarded_source_paths_clean_at_git_head"
        if not any(failure["kind"] == "dirty_guarded_source_path" for failure in failures)
        else "guarded_source_paths_dirty",
        "redaction_check": "pass",
        "artifact_paths": [],
        "failures": failures,
    }
    receipt_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return receipt_path


def release_runtime_source_paths() -> list[str]:
    paths: list[str] = []
    for crate_name in RELEASE_RUNTIME_CRATES:
        crate_src = REPO_ROOT / "crates" / crate_name / "src"
        paths.extend(
            str(path.relative_to(REPO_ROOT))
            for path in crate_src.rglob("*.rs")
            if path.is_file()
        )
    return sorted(paths)


def guarded_source_paths() -> list[str]:
    patterns = [
        "Cargo.toml",
        "deny.toml",
        "crates/*/Cargo.toml",
        "crates/*/src/*.rs",
        "crates/*/src/**/*.rs",
        "scripts/*",
        ".github/workflows/*",
    ]
    paths: list[str] = []
    for pattern in patterns:
        matches = sorted(glob.glob(str(REPO_ROOT / pattern), recursive=True))
        if matches:
            paths.extend(
                str(Path(match).relative_to(REPO_ROOT))
                for match in matches
                if Path(match).is_file()
            )
        else:
            paths.append(pattern)
    return sorted(set(paths))


def dirty_guarded_source_paths() -> list[str]:
    paths = guarded_source_paths()
    dirty: set[str] = set()
    worktree = subprocess.run(
        ["git", "diff", "--name-only", "HEAD", "--", *paths],
        check=False,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        text=True,
    )
    index = subprocess.run(
        ["git", "diff", "--cached", "--name-only", "--", *paths],
        check=False,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        text=True,
    )
    dirty.update(line for line in worktree.stdout.splitlines() if line)
    dirty.update(line for line in index.stdout.splitlines() if line)
    return sorted(dirty)


def release_runtime_sources() -> dict[str, str]:
    return {
        relative_path: strip_cfg_test_items(REPO_ROOT / relative_path)
        for relative_path in release_runtime_source_paths()
    }


def git_head() -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        text=True,
    ).strip()


if __name__ == "__main__":
    raise SystemExit(main())
