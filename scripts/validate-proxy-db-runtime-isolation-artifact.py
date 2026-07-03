#!/usr/bin/env python3
"""Validate the proxy DB runtime-isolation smoke artifact."""

import json
import os
import sys
import typing as t


Check = tuple[str, bool]


def artifact_source_metadata(payload: dict[str, t.Any], artifact_path: str) -> dict[str, t.Any]:
    provenance = payload.get("s8_provenance", {})
    return {
        "artifact": os.path.basename(artifact_path),
        "s8_run_id": provenance.get("run_id"),
        "git_head": payload.get("git_head"),
        "runtime_roots": payload.get("runtime_roots", {}),
        "mode": payload.get("mode"),
    }


def combine_artifacts(
    soak_artifact_path: str,
    quota_artifact_path: str,
    combined_artifact_path: str,
) -> None:
    with open(soak_artifact_path, "r", encoding="utf-8") as soak_file:
        soak_payload = json.load(soak_file)
    with open(quota_artifact_path, "r", encoding="utf-8") as quota_file:
        quota_payload = json.load(quota_file)

    combined = dict(soak_payload)
    combined["pressure"] = quota_payload.get("pressure", {})
    combined["signal_ordering"] = quota_payload.get("signal_ordering", {})
    combined["account_selection"] = quota_payload.get("account_selection", {})
    combined["quota_reconnect"] = quota_payload.get("quota_reconnect", {})
    combined["quota_reconnect_progress"] = quota_payload.get("quota_reconnect_progress", {})
    combined["quota_reconnect_upstream"] = quota_payload.get("upstream", {})
    combined["source_artifacts"] = {
        "three_websocket_soak": artifact_source_metadata(soak_payload, soak_artifact_path),
        "quota_reconnect": artifact_source_metadata(quota_payload, quota_artifact_path),
    }

    with open(combined_artifact_path, "w", encoding="utf-8") as combined_file:
        json.dump(combined, combined_file, indent=2)
        combined_file.write("\n")


def sanitized(path: str, repo_root: str) -> str:
    resolved = os.path.realpath(path)
    repo = os.path.realpath(repo_root)
    try:
        relative = os.path.relpath(resolved, repo)
    except ValueError:
        return "<external>"
    if relative == ".":
        return "<repo>"
    if relative.startswith(".."):
        return "<external>"
    return f"<repo>/{relative}"


def is_under_path(path: str, parent: str) -> bool:
    resolved_path = os.path.realpath(path)
    resolved_parent = os.path.realpath(parent)
    try:
        return os.path.commonpath([resolved_path, resolved_parent]) == resolved_parent
    except ValueError:
        return False


def is_number(value: t.Any) -> bool:
    return isinstance(value, int | float) and not isinstance(value, bool)


def source_artifact(payload: dict[str, t.Any], source_name: str) -> dict[str, t.Any]:
    source_artifacts = payload.get("source_artifacts", {})
    artifact = source_artifacts.get(source_name)
    if isinstance(artifact, dict):
        return artifact
    return {}


def source_artifact_pair(payload: dict[str, t.Any]) -> tuple[dict[str, t.Any], dict[str, t.Any]]:
    return (
        source_artifact(payload, "three_websocket_soak"),
        source_artifact(payload, "quota_reconnect"),
    )


def source_artifacts_have_same_nonempty_field(
    payload: dict[str, t.Any],
    field_name: str,
) -> bool:
    soak_source, quota_source = source_artifact_pair(payload)
    soak_value = soak_source.get(field_name)
    quota_value = quota_source.get(field_name)
    return isinstance(soak_value, str) and bool(soak_value) and soak_value == quota_value


def source_artifacts_have_same_runtime_roots(payload: dict[str, t.Any]) -> bool:
    soak_source, quota_source = source_artifact_pair(payload)
    runtime_roots = payload.get("runtime_roots", {})
    return (
        isinstance(soak_source.get("runtime_roots"), dict)
        and isinstance(quota_source.get("runtime_roots"), dict)
        and soak_source.get("runtime_roots") == runtime_roots
        and quota_source.get("runtime_roots") == runtime_roots
    )


def source_artifacts_are_same_single_execution(payload: dict[str, t.Any]) -> bool:
    soak_source, quota_source = source_artifact_pair(payload)
    soak_artifact = soak_source.get("artifact")
    quota_artifact = quota_source.get("artifact")
    soak_mode = soak_source.get("mode")
    quota_mode = quota_source.get("mode")
    return (
        isinstance(soak_artifact, str)
        and bool(soak_artifact)
        and soak_artifact == quota_artifact
        and soak_mode == "s8-overlap-quota"
        and quota_mode == "s8-overlap-quota"
    )


def source_artifacts_match_validated_artifact(
    payload: dict[str, t.Any],
    artifact_basename: str,
) -> bool:
    soak_source, quota_source = source_artifact_pair(payload)
    soak_artifact = soak_source.get("artifact")
    quota_artifact = quota_source.get("artifact")
    return (
        isinstance(soak_artifact, str)
        and bool(soak_artifact)
        and soak_artifact == artifact_basename
        and quota_artifact == artifact_basename
    )


def quota_signal_during_overlap(payload: dict[str, t.Any]) -> bool:
    upstream = payload.get("upstream", {})
    registry = payload.get("router_websocket_registry", {})
    signal_unix_ms = registry.get("quota_reconnect_signal_unix_ms")
    overlap_started_unix_ms = upstream.get("overlap_started_unix_ms")
    overlap_completed_unix_ms = upstream.get("overlap_completed_unix_ms")
    return (
        is_number(signal_unix_ms)
        and is_number(overlap_started_unix_ms)
        and is_number(overlap_completed_unix_ms)
        and overlap_started_unix_ms <= signal_unix_ms <= overlap_completed_unix_ms
    )


def validation_checks(
    payload: dict[str, t.Any],
    artifact_basename: str,
    repo_root: str,
    router_root: str,
    codex_home: str,
    process_home: str,
    threshold_ms: int,
) -> list[Check]:
    repo_tmp_dev_state = os.path.join(repo_root, "tmp", "dev-state")

    runtime_roots = payload.get("runtime_roots", {})
    pressure = payload.get("pressure", {})
    upstream = payload.get("upstream", {})
    clients = payload.get("clients", {})
    signal_ordering = payload.get("signal_ordering", {})
    account_selection = payload.get("account_selection", {})
    quota_reconnect_progress = payload.get("quota_reconnect_progress", {})
    quota_reconnect_signal_latency_ms = quota_reconnect_progress.get("signal_latency_ms")
    router_signal_count = quota_reconnect_progress.get("router_signal_count")

    return [
        ("runtime_root_mode", runtime_roots.get("mode") == "copied-dev-state"),
        (
            "effective_router_root_under_dev_state",
            is_under_path(router_root, repo_tmp_dev_state),
        ),
        ("router_root", runtime_roots.get("router_root") == sanitized(router_root, repo_root)),
        (
            "router_db",
            runtime_roots.get("router_db")
            == sanitized(os.path.join(router_root, "state.sqlite"), repo_root),
        ),
        (
            "effective_codex_home_under_dev_state",
            is_under_path(codex_home, repo_tmp_dev_state),
        ),
        ("codex_home", runtime_roots.get("codex_home") == sanitized(codex_home, repo_root)),
        (
            "codex_db",
            runtime_roots.get("codex_db")
            == sanitized(os.path.join(codex_home, "state_5.sqlite"), repo_root),
        ),
        (
            "effective_process_home_under_dev_state",
            is_under_path(process_home, repo_tmp_dev_state),
        ),
        (
            "process_home",
            runtime_roots.get("process_home") == sanitized(process_home, repo_root),
        ),
        ("client_count", clients.get("count") == 3),
        ("clients_all_success", clients.get("all_success") is True),
        ("overlap_proven", upstream.get("active_high_water", 0) >= 3),
        ("progress_threshold", upstream.get("real_overlap_duration_ms", 0) >= threshold_ms),
        (
            "quota_reconnect_progress_threshold",
            is_number(quota_reconnect_signal_latency_ms)
            and quota_reconnect_signal_latency_ms <= threshold_ms,
        ),
        (
            "router_signal_count",
            is_number(router_signal_count) and router_signal_count >= 1,
        ),
        (
            "source_artifacts_same_s8_run_id",
            source_artifacts_have_same_nonempty_field(payload, "s8_run_id"),
        ),
        (
            "source_artifacts_same_git_head",
            source_artifacts_have_same_nonempty_field(payload, "git_head"),
        ),
        (
            "source_artifacts_same_runtime_roots",
            source_artifacts_have_same_runtime_roots(payload),
        ),
        (
            "source_artifacts_same_single_execution",
            source_artifacts_are_same_single_execution(payload),
        ),
        (
            "source_artifacts_match_validated_artifact",
            source_artifacts_match_validated_artifact(payload, artifact_basename),
        ),
        ("quota_signal_during_overlap", quota_signal_during_overlap(payload)),
        (
            "signal_before_persistence",
            signal_ordering.get("signal_before_persistence") is True,
        ),
        ("non_reselection", account_selection.get("non_reselection") is True),
        ("copied_db_pressure", pressure.get("copied_db_pressure_proven") is True),
        (
            "copied_db_pressure_mechanism",
            pressure.get("provider_error_observer_delay") is True
            or pressure.get("sqlite_lock_or_maintenance_pressure") is True,
        ),
    ]


def print_report(
    payload: dict[str, t.Any],
    checks: list[Check],
) -> None:
    runtime_roots = payload.get("runtime_roots", {})
    pressure = payload.get("pressure", {})
    signal_ordering = payload.get("signal_ordering", {})
    account_selection = payload.get("account_selection", {})
    quota_reconnect_progress = payload.get("quota_reconnect_progress", {})
    soak_source, quota_source = source_artifact_pair(payload)

    failed = [name for name, passed in checks if not passed]
    pass_count = len(checks) - len(failed)
    fail_count = len(failed)
    print(f"runtime_root_mode={runtime_roots.get('mode', '<missing>')}")
    print(f"artifact_router_root={runtime_roots.get('router_root', '<missing>')}")
    print(f"artifact_router_db={runtime_roots.get('router_db', '<missing>')}")
    print(f"artifact_codex_home={runtime_roots.get('codex_home', '<missing>')}")
    print(f"artifact_codex_db={runtime_roots.get('codex_db', '<missing>')}")
    print(f"artifact_process_home={runtime_roots.get('process_home', '<missing>')}")
    print(f"pressure_mechanism={pressure.get('pressure_mechanism', '<missing>')}")
    print(f"copied_db_pressure_proven={pressure.get('copied_db_pressure_proven', False)}")
    print(
        "provider_error_observer_delay="
        f"{pressure.get('provider_error_observer_delay', False)}"
    )
    print(
        "sqlite_lock_or_maintenance_pressure="
        f"{pressure.get('sqlite_lock_or_maintenance_pressure', False)}"
    )
    print(f"signal_before_persistence={signal_ordering.get('signal_before_persistence', False)}")
    print(f"non_reselection={account_selection.get('non_reselection', False)}")
    print(
        "quota_reconnect_signal_latency_ms="
        f"{quota_reconnect_progress.get('signal_latency_ms', '<missing>')}"
    )
    print(
        "quota_reconnect_router_signal_count="
        f"{quota_reconnect_progress.get('router_signal_count', '<missing>')}"
    )
    print(f"source_s8_run_id={soak_source.get('s8_run_id', '<missing>')}")
    print(f"quota_source_s8_run_id={quota_source.get('s8_run_id', '<missing>')}")
    print(f"pass_count={pass_count}")
    print(f"fail_count={fail_count}")
    if failed:
        print(f"blocked_reason=artifact missing required S8 proof: {','.join(failed)}")


def validate_payload(
    payload: dict[str, t.Any],
    artifact_basename: str,
    repo_root: str,
    router_root: str,
    codex_home: str,
    process_home: str,
    threshold_ms: int,
    *,
    emit_report: bool,
) -> list[str]:
    checks = validation_checks(
        payload,
        artifact_basename,
        repo_root,
        router_root,
        codex_home,
        process_home,
        threshold_ms,
    )
    if emit_report:
        print_report(payload, checks)
    return [name for name, passed in checks if not passed]


def valid_self_test_payload(repo_root: str) -> dict[str, t.Any]:
    router_root = os.path.join(repo_root, "tmp", "dev-state", "codex-router")
    codex_home = os.path.join(repo_root, "tmp", "dev-state", "codex")
    process_home = os.path.join(repo_root, "tmp", "dev-state", "sentinel-home")
    return {
        "runtime_roots": {
            "mode": "copied-dev-state",
            "router_root": sanitized(router_root, repo_root),
            "router_db": sanitized(os.path.join(router_root, "state.sqlite"), repo_root),
            "codex_home": sanitized(codex_home, repo_root),
            "codex_db": sanitized(os.path.join(codex_home, "state_5.sqlite"), repo_root),
            "process_home": sanitized(process_home, repo_root),
        },
        "pressure": {
            "pressure_mechanism": "provider-error-observer-delay",
            "copied_db_pressure_proven": True,
            "provider_error_observer_delay": True,
            "sqlite_lock_or_maintenance_pressure": False,
        },
        "upstream": {
            "active_high_water": 3,
            "real_overlap_duration_ms": 750,
            "overlap_started_unix_ms": 1_000,
            "overlap_completed_unix_ms": 2_000,
        },
        "router_websocket_registry": {
            "quota_reconnect_signal_unix_ms": 1_500,
        },
        "quota_reconnect_progress": {
            "signal_latency_ms": 750,
            "router_signal_count": 1,
        },
        "clients": {
            "count": 3,
            "all_success": True,
        },
        "signal_ordering": {
            "signal_before_persistence": True,
        },
        "account_selection": {
            "non_reselection": True,
        },
        "source_artifacts": {
            "three_websocket_soak": {
                "artifact": "s8-overlap-quota.json",
                "s8_run_id": "self-test-run",
                "git_head": "abc123",
                "mode": "s8-overlap-quota",
                "runtime_roots": {
                    "mode": "copied-dev-state",
                    "router_root": sanitized(router_root, repo_root),
                    "router_db": sanitized(os.path.join(router_root, "state.sqlite"), repo_root),
                    "codex_home": sanitized(codex_home, repo_root),
                    "codex_db": sanitized(os.path.join(codex_home, "state_5.sqlite"), repo_root),
                    "process_home": sanitized(process_home, repo_root),
                },
            },
            "quota_reconnect": {
                "artifact": "s8-overlap-quota.json",
                "s8_run_id": "self-test-run",
                "git_head": "abc123",
                "mode": "s8-overlap-quota",
                "runtime_roots": {
                    "mode": "copied-dev-state",
                    "router_root": sanitized(router_root, repo_root),
                    "router_db": sanitized(os.path.join(router_root, "state.sqlite"), repo_root),
                    "codex_home": sanitized(codex_home, repo_root),
                    "codex_db": sanitized(os.path.join(codex_home, "state_5.sqlite"), repo_root),
                    "process_home": sanitized(process_home, repo_root),
                },
            },
        },
    }


def run_self_test() -> int:
    repo_root = os.path.realpath(os.getcwd())
    router_root = os.path.join(repo_root, "tmp", "dev-state", "codex-router")
    codex_home = os.path.join(repo_root, "tmp", "dev-state", "codex")
    process_home = os.path.join(repo_root, "tmp", "dev-state", "sentinel-home")
    threshold_ms = 750
    artifact_basename = "s8-overlap-quota.json"

    cases: list[tuple[str, dict[str, t.Any], str | None, tuple[str, str, str]]] = []
    valid_payload = valid_self_test_payload(repo_root)
    cases.append(
        (
            "accepts_provider_error_observer_delay_pressure_mechanism",
            valid_payload,
            None,
            (router_root, codex_home, process_home),
        )
    )

    sqlite_pressure_payload = json.loads(json.dumps(valid_payload))
    sqlite_pressure_payload["pressure"]["pressure_mechanism"] = "sqlite-lock-or-maintenance"
    sqlite_pressure_payload["pressure"]["provider_error_observer_delay"] = False
    sqlite_pressure_payload["pressure"]["sqlite_lock_or_maintenance_pressure"] = True
    cases.append(
        (
            "accepts_sqlite_lock_or_maintenance_pressure_mechanism",
            sqlite_pressure_payload,
            None,
            (router_root, codex_home, process_home),
        )
    )

    missing_mechanism_payload = json.loads(json.dumps(valid_payload))
    missing_mechanism_payload["pressure"]["provider_error_observer_delay"] = False
    missing_mechanism_payload["pressure"]["sqlite_lock_or_maintenance_pressure"] = False
    cases.append(
        (
            "rejects_copied_db_pressure_without_approved_mechanism",
            missing_mechanism_payload,
            "copied_db_pressure_mechanism",
            (router_root, codex_home, process_home),
        )
    )

    false_green_quota_progress_payload = json.loads(json.dumps(valid_payload))
    false_green_quota_progress_payload["quota_reconnect_progress"] = {
        "signal_latency_ms": threshold_ms + 1,
    }
    cases.append(
        (
            "rejects_soak_overlap_when_quota_reconnect_progress_exceeds_threshold",
            false_green_quota_progress_payload,
            "quota_reconnect_progress_threshold",
            (router_root, codex_home, process_home),
        )
    )

    zero_router_signal_payload = json.loads(json.dumps(valid_payload))
    zero_router_signal_payload["quota_reconnect_progress"]["router_signal_count"] = 0
    cases.append(
        (
            "rejects_zero_router_signal_count",
            zero_router_signal_payload,
            "router_signal_count",
            (router_root, codex_home, process_home),
        )
    )

    mismatched_source_run_payload = json.loads(json.dumps(valid_payload))
    mismatched_source_run_payload["source_artifacts"] = {
        "three_websocket_soak": {
            "artifact": "soak.json",
            "s8_run_id": "run-a",
            "git_head": "abc123",
            "runtime_roots": valid_payload["runtime_roots"],
        },
        "quota_reconnect": {
            "artifact": "quota.json",
            "s8_run_id": "run-b",
            "git_head": "abc123",
            "runtime_roots": valid_payload["runtime_roots"],
        },
    }
    cases.append(
        (
            "rejects_mismatched_source_artifact_run_ids",
            mismatched_source_run_payload,
            "source_artifacts_same_s8_run_id",
            (router_root, codex_home, process_home),
        )
    )

    split_source_artifact_payload = json.loads(json.dumps(valid_payload))
    split_source_artifact_payload["source_artifacts"]["three_websocket_soak"][
        "artifact"
    ] = "three-websocket-soak.json"
    split_source_artifact_payload["source_artifacts"]["three_websocket_soak"][
        "mode"
    ] = "soak"
    split_source_artifact_payload["source_artifacts"]["quota_reconnect"][
        "artifact"
    ] = "quota-reconnect.json"
    split_source_artifact_payload["source_artifacts"]["quota_reconnect"][
        "mode"
    ] = "quota-reconnect"
    cases.append(
        (
            "rejects_split_source_artifacts",
            split_source_artifact_payload,
            "source_artifacts_same_single_execution",
            (router_root, codex_home, process_home),
        )
    )

    wrong_validated_artifact_payload = json.loads(json.dumps(valid_payload))
    wrong_validated_artifact_payload["source_artifacts"]["three_websocket_soak"][
        "artifact"
    ] = "some-other-s8-overlap-quota.json"
    wrong_validated_artifact_payload["source_artifacts"]["quota_reconnect"][
        "artifact"
    ] = "some-other-s8-overlap-quota.json"
    cases.append(
        (
            "rejects_source_artifacts_that_do_not_match_validated_artifact",
            wrong_validated_artifact_payload,
            "source_artifacts_match_validated_artifact",
            (router_root, codex_home, process_home),
        )
    )

    signal_after_overlap_payload = json.loads(json.dumps(valid_payload))
    signal_after_overlap_payload["router_websocket_registry"][
        "quota_reconnect_signal_unix_ms"
    ] = 2_001
    cases.append(
        (
            "rejects_quota_signal_after_overlap_window",
            signal_after_overlap_payload,
            "quota_signal_during_overlap",
            (router_root, codex_home, process_home),
        )
    )

    mismatched_source_git_head_payload = json.loads(json.dumps(valid_payload))
    mismatched_source_git_head_payload["source_artifacts"] = {
        "three_websocket_soak": {
            "artifact": "soak.json",
            "s8_run_id": "self-test-run",
            "git_head": "abc123",
            "runtime_roots": valid_payload["runtime_roots"],
        },
        "quota_reconnect": {
            "artifact": "quota.json",
            "s8_run_id": "self-test-run",
            "git_head": "def456",
            "runtime_roots": valid_payload["runtime_roots"],
        },
    }
    cases.append(
        (
            "rejects_mismatched_source_artifact_git_heads",
            mismatched_source_git_head_payload,
            "source_artifacts_same_git_head",
            (router_root, codex_home, process_home),
        )
    )

    outside_router_root = os.path.join(repo_root, "tmp", "not-dev-state", "codex-router")
    outside_router_payload = json.loads(json.dumps(valid_payload))
    outside_router_payload["runtime_roots"]["router_root"] = sanitized(
        outside_router_root,
        repo_root,
    )
    outside_router_payload["runtime_roots"]["router_db"] = sanitized(
        os.path.join(outside_router_root, "state.sqlite"),
        repo_root,
    )
    cases.append(
        (
            "rejects_effective_router_root_outside_tmp_dev_state",
            outside_router_payload,
            "effective_router_root_under_dev_state",
            (outside_router_root, codex_home, process_home),
        )
    )

    outside_codex_home = os.path.join(repo_root, "tmp", "not-dev-state", "codex")
    outside_codex_payload = json.loads(json.dumps(valid_payload))
    outside_codex_payload["runtime_roots"]["codex_home"] = sanitized(
        outside_codex_home,
        repo_root,
    )
    outside_codex_payload["runtime_roots"]["codex_db"] = sanitized(
        os.path.join(outside_codex_home, "state_5.sqlite"),
        repo_root,
    )
    cases.append(
        (
            "rejects_effective_codex_home_outside_tmp_dev_state",
            outside_codex_payload,
            "effective_codex_home_under_dev_state",
            (router_root, outside_codex_home, process_home),
        )
    )

    outside_process_home = os.path.join(repo_root, "tmp", "not-dev-state", "sentinel-home")
    outside_process_payload = json.loads(json.dumps(valid_payload))
    outside_process_payload["runtime_roots"]["process_home"] = sanitized(
        outside_process_home,
        repo_root,
    )
    cases.append(
        (
            "rejects_effective_process_home_outside_tmp_dev_state",
            outside_process_payload,
            "effective_process_home_under_dev_state",
            (router_root, codex_home, outside_process_home),
        )
    )

    failures: list[str] = []
    for case_name, payload, expected_failure, roots in cases:
        failed = validate_payload(
            payload,
            artifact_basename,
            repo_root,
            roots[0],
            roots[1],
            roots[2],
            threshold_ms,
            emit_report=False,
        )
        if expected_failure is None:
            if failed:
                failures.append(f"{case_name}: expected pass, failed={','.join(failed)}")
        elif expected_failure not in failed:
            failures.append(
                f"{case_name}: expected {expected_failure}, failed={','.join(failed)}"
            )

    if failures:
        for failure in failures:
            print(f"self_test_failure={failure}")
        return 1
    print("self_test=ok")
    return 0


def usage() -> str:
    return (
        "usage: validate-proxy-db-runtime-isolation-artifact.py "
        "<artifact> <repo-root> <router-root> <codex-home> <process-home> <threshold-ms>\n"
        "       validate-proxy-db-runtime-isolation-artifact.py --self-test"
    )


def main() -> int:
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        return run_self_test()

    if len(sys.argv) != 7:
        print(usage(), file=sys.stderr)
        return 2

    artifact_path, repo_root, router_root, codex_home, process_home, threshold_ms_raw = sys.argv[
        1:
    ]
    threshold_ms = int(threshold_ms_raw)

    with open(artifact_path, "r", encoding="utf-8") as artifact_file:
        payload = json.load(artifact_file)

    failed = validate_payload(
        payload,
        os.path.basename(artifact_path),
        repo_root,
        router_root,
        codex_home,
        process_home,
        threshold_ms,
        emit_report=True,
    )
    if failed:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
