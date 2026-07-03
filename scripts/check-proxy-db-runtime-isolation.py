#!/usr/bin/env python3
"""Structural checks for proxy DB runtime isolation boundaries."""

import argparse
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent


def source_text(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text(encoding="utf-8")


def production_text(relative_path: str) -> str:
    text = source_text(relative_path)
    marker = "\n#[cfg(test)]\nmod tests"
    if marker in text:
        return text.split(marker, maxsplit=1)[0]
    return text


def fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def indented_rust_item(text: str, marker: str) -> str:
    start = text.find(marker)
    if start == -1:
        fail(f"missing Rust item marker {marker}")
    next_markers = [
        text.find("\n    async fn ", start + len(marker)),
        text.find("\n    fn ", start + len(marker)),
        text.find("\n    pub fn ", start + len(marker)),
    ]
    next_starts = [index for index in next_markers if index != -1]
    end = min(next_starts) if next_starts else len(text)
    return text[start:end]


def check_no_admission_maintenance() -> None:
    forbidden = (
        "refresh_active_session_rollups_for_interval",
        "purge_active_session_rollups_before",
    )
    for relative_path in (
        "crates/codex-router-proxy/src/account_selection.rs",
        "crates/codex-router-proxy/src/websocket.rs",
        "crates/codex-router-proxy/src/server.rs",
    ):
        text = production_text(relative_path)
        for token in forbidden:
            if token in text:
                fail(f"{relative_path} contains admission/socket maintenance call {token}")
    print("PASS no-admission-maintenance")


def check_no_hot_path_sqlite_open() -> None:
    for relative_path in (
        "crates/codex-router-proxy/src/account_selection.rs",
        "crates/codex-router-proxy/src/websocket.rs",
    ):
        text = production_text(relative_path)
        for token in ("AsyncSqliteStateStore::open", "open_read_only", "PRAGMA user_version"):
            if token in text:
                fail(f"{relative_path} contains hot-path SQLite open/schema token {token}")
    server_text = production_text("crates/codex-router-proxy/src/server.rs")
    for marker in (
        "async fn handle_hyper_connection(",
        "async fn handle_hyper_request(",
        "async fn handle_hyper_websocket_request(",
        "async fn handle_hyper_http_request(",
        "async fn hyper_request_to_streaming_proxy_request(",
    ):
        item_text = indented_rust_item(server_text, marker)
        for token in ("AsyncSqliteStateStore::open", "open_read_only", "PRAGMA user_version"):
            if token in item_text:
                fail(f"server.rs hot-path item {marker} contains SQLite open/schema token {token}")
    print("PASS no-hot-path-sqlite-open")


def check_no_raw_provider_body_actor_command() -> None:
    text = production_text("crates/codex-router-proxy/src/db_write_actor.rs")
    forbidden = ("Vec<u8>", "body:", "provider_error_body", "raw_provider_body")
    for token in forbidden:
        if token in text:
            fail(f"db_write_actor command surface contains raw provider body token {token}")
    required = (
        "DbWriteCommand::ProviderQuotaExhausted",
        "classification: ProviderErrorClassification",
        "observed_unix_seconds",
    )
    for token in required:
        if token not in text:
            fail(f"db_write_actor command surface missing derived field token {token}")
    print("PASS no-raw-provider-body-actor-command")


def check_no_raw_provider_body_observer_boundary() -> None:
    text = production_text("crates/codex-router-proxy/src/provider_error.rs")
    forbidden = (
        "observe_provider_error<'a>(\n        &'a self,\n        account_id: AccountId,\n        route_band: RouteBand,\n        body: Vec<u8>,",
        "body: Vec<u8>",
        "raw_provider_body",
    )
    for token in forbidden:
        if token in text:
            fail(f"provider_error observer boundary contains raw provider body token {token}")
    required = (
        "classification: ProviderErrorClassification",
        "observed_unix_seconds",
    )
    for token in required:
        if token not in text:
            fail(f"provider_error observer boundary missing derived field token {token}")
    print("PASS no-raw-provider-body-observer-boundary")


def check_runtime_owns_maintenance_actor() -> None:
    text = production_text("crates/codex-router-proxy/src/server.rs")
    server_required = (
        "use crate::maintenance_actor::MaintenanceActor;",
        "maintenance_actor: MaintenanceActor",
        "MaintenanceActor::start_on_handle(",
        "self.maintenance_actor.shutdown().await",
    )
    for token in server_required:
        if token not in text:
            fail(f"LoopbackRouterRuntime does not own live MaintenanceActor token {token}")
    server_producer_required = (
        "MaintenanceHint::CleanupStaleActiveClients",
        "MaintenanceHint::RefreshActiveSessionRollups",
        "MaintenanceHint::ApplyActiveSessionRetention",
        "MaintenanceHint::CompactActiveSessionHistory",
        ".try_enqueue(MaintenanceHint::",
    )
    for token in server_producer_required:
        if token not in text:
            fail(f"LoopbackRouterRuntime does not enqueue live MaintenanceActor work token {token}")
    maintenance_text = production_text("crates/codex-router-proxy/src/maintenance_actor.rs")
    maintenance_required = (
        "MaintenanceHint::CleanupStaleActiveClients",
        "MaintenanceHint::RefreshActiveSessionRollups",
        "MaintenanceHint::ApplyActiveSessionRetention",
        "MaintenanceHint::CompactActiveSessionHistory",
        "active_client_counts_for_route_band(",
        "refresh_active_session_rollups_for_interval(",
        "purge_active_session_rollups_before(",
        "compact_active_session_history(",
    )
    for token in maintenance_required:
        if token not in maintenance_text:
            fail(f"MaintenanceActor surface missing required maintenance token {token}")
    print("PASS runtime-owns-maintenance-actor")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--no-admission-maintenance", action="store_true")
    parser.add_argument("--no-hot-path-sqlite-open", action="store_true")
    parser.add_argument("--no-raw-provider-body-actor-command", action="store_true")
    parser.add_argument("--no-raw-provider-body-observer-boundary", action="store_true")
    parser.add_argument("--runtime-owns-maintenance-actor", action="store_true")
    args = parser.parse_args()

    ran = False
    if args.no_admission_maintenance:
        ran = True
        check_no_admission_maintenance()
    if args.no_hot_path_sqlite_open:
        ran = True
        check_no_hot_path_sqlite_open()
    if args.no_raw_provider_body_actor_command:
        ran = True
        check_no_raw_provider_body_actor_command()
    if args.no_raw_provider_body_observer_boundary:
        ran = True
        check_no_raw_provider_body_observer_boundary()
    if args.runtime_owns_maintenance_actor:
        ran = True
        check_runtime_owns_maintenance_actor()
    if not ran:
        parser.error("select at least one check")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
