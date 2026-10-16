#!/usr/bin/env python3
"""Retain one qualified debug Router across two owning Host lifetimes."""

import argparse
import datetime as dt
import hashlib
import json
import os
import signal
import socket
import subprocess
import time
import typing as t
import urllib.request
from http.client import HTTPResponse
from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field

from scripts.verification.process_ownership import (
    owned_closure,
    same_identity,
    signal_each_owned,
)
from scripts.verification.typed_observations import (
    BinaryQualification,
    CleanupFailure,
    CleanupResult,
    IdentityRecord,
    JsonObject,
    ProcessIdentity,
    ProcessSignal,
    observed_object,
    read_observed_json,
)


class RuntimePlan(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="forbid")
    repository: Path
    source_head: str
    qualification: Path
    router_root: Path
    evidence: Path
    port: int = Field(gt=0, lt=65536)
    original_port: int = Field(gt=0, lt=65536)
    expected_version: str
    profile: Path
    claude_node: Path
    claude_adapter: Path
    claude_executable: Path
    cursor_executable: Path
    deadline: dt.datetime


class RuntimeArguments(BaseModel):
    plan: Path
    check: bool


def save_record(path: Path, value: object) -> None:
    with path.open("x") as output:
        json.dump(value, output, indent=2)
        _ = output.write("\n")
    path.chmod(0o600)


def sha256(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def process_table() -> dict[int, ProcessIdentity]:
    completed = subprocess.run(
        ["/bin/ps", "-axo", "pid=,ppid=,uid=,pgid=,lstart=,comm="],
        capture_output=True,
        text=True,
        check=True,
        timeout=5,
    )
    records: dict[int, ProcessIdentity] = {}
    for line in completed.stdout.splitlines():
        fields = line.split(None, 9)
        if len(fields) == 10:
            identity = ProcessIdentity(
                pid=int(fields[0]),
                parent=int(fields[1]),
                uid=int(fields[2]),
                group=int(fields[3]),
                birth=" ".join(fields[4:9]),
                executable=fields[9],
            )
            records[identity.pid] = identity
    return records


def observe_owned(root_pid: int, known: dict[int, ProcessIdentity]) -> None:
    observed = process_table()
    candidates = owned_closure(
        root_pid,
        {pid: item.model_dump() for pid, item in known.items()},
        {pid: item.model_dump() for pid, item in observed.items()},
    )
    for pid in candidates:
        if pid in observed:
            item = observed[pid]
            if item.uid != os.getuid():
                raise RuntimeError("owned process UID changed")
            if pid in known and not same_identity(
                item.model_dump(), known[pid].model_dump()
            ):
                raise RuntimeError("owned PID reused")
            known[pid] = item


def current_identity_fields(pid: int) -> IdentityRecord | None:
    current = process_table().get(pid)
    return current.as_record() if current is not None else None


def stop_owned(
    child: subprocess.Popen[bytes], known: dict[int, ProcessIdentity]
) -> CleanupResult:
    """Observation failure cannot skip independent cleanup of recorded children."""
    errors: list[CleanupFailure] = []
    signals: list[ProcessSignal] = []
    try:
        observe_owned(child.pid, known)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        errors.append(CleanupFailure(stage="observe", error=type(error).__name__))
    for signal_number, wait_seconds in [(signal.SIGTERM, 45), (signal.SIGKILL, 5)]:
        try:
            _ = process_table()
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            return CleanupResult(
                exit=child.poll(),
                signals=signals,
                errors=[
                    *errors,
                    CleanupFailure(stage="freshIdentity", error=type(error).__name__),
                ],
                remaining={
                    pid: "unverified; access required before signalling"
                    for pid in {child.pid, *known}
                },
            )
        ordered = dict(sorted(known.items(), key=lambda pair: pair[0] != child.pid))
        result = signal_each_owned(
            {pid: item.model_dump() for pid, item in ordered.items()},
            current_identity_fields,
            lambda pid, chosen_signal=signal_number: os.kill(pid, chosen_signal),
        )
        signals.extend(
            ProcessSignal(pid=pid, signal=int(signal_number)) for pid in result.sent
        )
        errors.extend(
            CleanupFailure(pid=item.pid, error=item.error) for item in result.errors
        )
        try:
            _ = child.wait(timeout=wait_seconds)
        except subprocess.TimeoutExpired:
            continue
        # If a clock jump skips polling, keep known inventory unresolved, never empty.
        remaining: dict[int, IdentityRecord | str] = {
            pid: item.as_record() for pid, item in known.items()
        }
        until = time.monotonic() + 5
        while time.monotonic() < until:
            try:
                remaining = {
                    pid: item.as_record()
                    for pid, item in process_table().items()
                    if pid in known
                    and same_identity(item.as_record(), known[pid].as_record())
                }
            except (
                OSError,
                ValueError,
                RuntimeError,
                subprocess.SubprocessError,
            ) as error:
                errors.append(
                    CleanupFailure(
                        stage="settlementObservation", error=type(error).__name__
                    )
                )
                break
            if not remaining:
                break
            time.sleep(0.1)
        if not remaining:
            break
    try:
        final_remaining: dict[int, IdentityRecord | str] = {
            pid: item.as_record()
            for pid, item in process_table().items()
            if pid in known and same_identity(item.as_record(), known[pid].as_record())
        }
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        final_remaining = {
            pid: "unverified; final observation failed" for pid in {child.pid, *known}
        }
        errors.append(
            CleanupFailure(stage="finalObservation", error=type(error).__name__)
        )
    return CleanupResult(
        exit=child.poll(), signals=signals, errors=errors, remaining=final_remaining
    )


def check_root_consumers(root: Path, allowed_router: int | None = None) -> None:
    if (
        root.is_symlink()
        or root.stat().st_uid != os.getuid()
        or root.stat().st_mode & 0o022
    ):
        raise RuntimeError("debug root ownership mismatch")
    if (root / "secrets/debug-plaintext.marker").exists():
        raise RuntimeError("plaintext debug secrets are forbidden for this journey")
    for path in root.rglob("*"):
        if path.is_file() and ".sqlite" in path.name:
            result = subprocess.run(
                ["/usr/sbin/lsof", "-t", "--", str(path)],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            if result.returncode not in (0, 1):
                raise RuntimeError("database consumer lookup failed")
            for value in result.stdout.split():
                if (
                    int(value) != allowed_router
                    or path.parent != root
                    or not path.name.startswith("state.sqlite")
                ):
                    raise RuntimeError("debug root has an unowned consumer")
    if (
        list(root.rglob("*.sock"))
        or (root / "agent-communication/service.json").exists()
    ):
        raise RuntimeError("debug root still publishes another Host")


def health_check(plan: RuntimePlan) -> JsonObject:
    opened = t.cast(
        object,
        urllib.request.urlopen(f"http://127.0.0.1:{plan.port}/healthz", timeout=1),
    )
    if not isinstance(opened, HTTPResponse):
        raise TypeError("health response is not HTTP")
    with opened as response:
        health = read_observed_json(response.read())
    if health["binary_version"] != plan.expected_version:
        raise RuntimeError("Router version mismatch")
    return observed_object(health)


def verify_ready(plan: RuntimePlan) -> JsonObject:
    recorded = ProcessIdentity.model_validate_json(
        (plan.evidence / "router-identity.json").read_text()
    )
    if not same_identity(
        next(
            (
                item.model_dump()
                for pid, item in process_table().items()
                if pid == recorded.pid
            ),
            None,
        ),
        recorded.model_dump(),
    ):
        raise RuntimeError("live Router process identity mismatch")
    started = read_observed_json((plan.evidence / "router-started.json").read_text())
    if sha256(Path(recorded.executable)) != started["binarySHA256"]:
        raise RuntimeError("live Router binary changed")
    manifest = read_observed_json(
        (plan.router_root / "agent-communication/service.json").read_text()
    )
    if (
        manifest["serviceVersion"] != plan.expected_version
        or manifest["routerProxyEndpoint"] != f"127.0.0.1:{plan.port}"
    ):
        raise RuntimeError("published Host root, version or Router endpoint mismatch")
    return {
        "pid": recorded.pid,
        "health": health_check(plan),
        "manifest": manifest,
        "credentialUsability": "not established by health; genuine provider journey is required",
    }


def run_lifecycle(plan: RuntimePlan) -> None:
    if plan.deadline.tzinfo is None or dt.datetime.now(dt.UTC) >= plan.deadline:
        raise RuntimeError("expired runtime allocation")
    head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=plan.repository, text=True
    ).strip()
    if head != plan.source_head:
        raise RuntimeError("source target mismatch")
    # Fail before creating a credential requester if process observation is denied.
    _ = process_table()
    if plan.router_root.resolve() == Path.home() / ".codex-router":
        raise RuntimeError("production Router root is outside verification scope")
    qualification = BinaryQualification.model_validate_json(
        plan.qualification.read_text()
    )
    binaries = {item.target: item for item in qualification.binaries}
    for name in ["codex-router", "agent-collaboration", "agent-sessions"]:
        item = binaries[name]
        if sha256(Path(item.path)) != item.sha256 or item.testGuardFeatures:
            raise RuntimeError("binary hash or normal-build mismatch")
        _ = subprocess.run(
            ["/usr/bin/codesign", "--verify", "--strict", item.path],
            capture_output=True,
            check=True,
        )
        signature = subprocess.run(
            ["/usr/bin/codesign", "-dv", "--verbose=4", item.path],
            capture_output=True,
            text=True,
            check=True,
        ).stderr
        if (
            f"Identifier=dev.shravansunder.{name}.debug" not in signature
            or "TeamIdentifier=974QD84WVC" not in signature
        ):
            raise RuntimeError("debug signing identity mismatch")
    check_root_consumers(plan.router_root)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", plan.port))
    plan.evidence.mkdir(mode=0o700, parents=True, exist_ok=True)
    save_record(
        plan.evidence / "driver-binding.json",
        {
            "driverSHA256": sha256(Path(__file__)),
            "plan": plan.model_dump(mode="json"),
            "qualificationSHA256": sha256(plan.qualification),
            "observedSource": head,
        },
    )
    workspace = plan.evidence / "workspace"
    workspace.mkdir(mode=0o700, exist_ok=True)
    native_directory = plan.evidence / "native"
    native_directory.mkdir(mode=0o700, exist_ok=True)
    profile_original = plan.profile.read_bytes()
    original_endpoint = f"http://127.0.0.1:{plan.original_port}/v1".encode()
    if profile_original.count(original_endpoint) != 1:
        raise RuntimeError("debug profile original endpoint mismatch")
    temporary_profile = profile_original.replace(
        original_endpoint, f"http://127.0.0.1:{plan.port}/v1".encode()
    )
    binary = binaries["codex-router"].path
    router = None
    router_known: dict[int, ProcessIdentity] = {}
    settlements: list[JsonObject] = []
    failure = None
    profile_changed = False
    try:
        with (
            (plan.evidence / "router.stdout").open("xb") as stdout,
            (plan.evidence / "router.stderr").open("xb") as stderr,
        ):
            router = subprocess.Popen(
                [
                    binary,
                    "serve",
                    "--port",
                    str(plan.port),
                    "--state-db",
                    str(plan.router_root / "state.sqlite"),
                    "--secret-root",
                    str(plan.router_root / "secrets"),
                    "--require-debug-isolation",
                    "--disable-background-quota-refresh",
                ],
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
        save_record(
            plan.evidence / "router-started.json",
            {
                "pid": router.pid,
                "binarySHA256": binaries["codex-router"].sha256,
                "root": str(plan.router_root),
                "head": head,
            },
        )
        observe_owned(router.pid, router_known)
        save_record(
            plan.evidence / "router-identity.json",
            router_known[router.pid].model_dump(),
        )
        print(
            json.dumps(
                {"routerPID": router.pid, "waitingForKeychainOrReadiness": True}
            ),
            flush=True,
        )
        while (
            router.poll() is None
            and dt.datetime.now(dt.UTC) < plan.deadline
            and not (plan.evidence / "stop.json").exists()
        ):
            try:
                _ = health_check(plan)
                break
            except (OSError, ValueError):
                time.sleep(0.2)
        else:
            raise RuntimeError("Router did not reach readiness")
        save_record(
            plan.evidence / "router-ready.json",
            {"pid": router.pid, "health": health_check(plan)},
        )
        _ = plan.profile.write_bytes(temporary_profile)
        profile_changed = True
        save_record(
            plan.evidence / "profile-binding.json",
            {
                "originalSHA256": hashlib.sha256(profile_original).hexdigest(),
                "temporarySHA256": hashlib.sha256(temporary_profile).hexdigest(),
                "sourceAnchor": "crates/codex-native-integration/src/debug_profile_projection.rs:107",
            },
        )
        for phase in ["A", "B"]:
            check_root_consumers(plan.router_root, router.pid)
            host = None
            known: dict[int, ProcessIdentity] = {}
            try:
                environment = os.environ.copy()
                environment.update(
                    {
                        "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET": str(
                            native_directory / "app-server.sock"
                        ),
                        "NODE_COMPILE_CACHE": str(plan.evidence / "node-cache"),
                        "DIRENV_DISABLE": "1",
                        "CURSOR_AGENT_DISABLE_DEBUG_LOG": "1",
                        "CLAUDE_CODE_EXECUTABLE": str(plan.claude_executable),
                        "RUST_LOG": "off",
                    }
                )
                argv = [
                    binary,
                    "host",
                    "--router-root",
                    str(plan.router_root),
                    "--port",
                    str(plan.port),
                    "--mcp-bind",
                    "127.0.0.1:0",
                    "--require-debug-isolation",
                    "--cursor-acp-executable",
                    str(plan.cursor_executable),
                ]
                for argument in [
                    "--disable-auto-update",
                    "--disable-project-configs",
                    "--workspace",
                    str(workspace),
                    "acp",
                ]:
                    argv.extend(["--cursor-acp-arguments", argument])
                argv.extend(
                    [
                        "--claude-acp-executable",
                        str(plan.claude_node),
                        "--claude-acp-arguments",
                        str(plan.claude_adapter),
                    ]
                )
                with (
                    (plan.evidence / f"host-{phase}.stdout").open("xb") as stdout,
                    (plan.evidence / f"host-{phase}.stderr").open("xb") as stderr,
                ):
                    host = subprocess.Popen(
                        argv,
                        env=environment,
                        stdout=stdout,
                        stderr=stderr,
                        start_new_session=True,
                    )
                observe_owned(host.pid, known)
                save_record(
                    plan.evidence / f"host-{phase}-started.json",
                    {
                        "pid": host.pid,
                        "identity": known[host.pid].model_dump(),
                        "argv": argv,
                    },
                )
                readiness_end = time.monotonic() + 120
                while host.poll() is None and time.monotonic() < readiness_end:
                    observe_owned(host.pid, known)
                    manifest_path = (
                        plan.router_root / "agent-communication/service.json"
                    )
                    if manifest_path.exists():
                        manifest = read_observed_json(manifest_path.read_text())
                        if manifest["serviceVersion"] == plan.expected_version:
                            save_record(
                                plan.evidence / f"host-{phase}-ready.json", manifest
                            )
                            print(
                                json.dumps(
                                    {"hostPhase": phase, "pid": host.pid, "ready": True}
                                ),
                                flush=True,
                            )
                            break
                    time.sleep(0.2)
                else:
                    raise RuntimeError("Host readiness not reached")
                while host.poll() is None and dt.datetime.now(dt.UTC) < plan.deadline:
                    observe_owned(host.pid, known)
                    if (plan.evidence / "stop.json").exists() or (
                        phase == "A" and (plan.evidence / "reopen.json").exists()
                    ):
                        break
                    time.sleep(0.2)
            finally:
                if host is not None:
                    result = stop_owned(host, known)
                    save_record(
                        plan.evidence / f"host-{phase}-settlement.json",
                        result.model_dump(mode="json", exclude_unset=True),
                    )
                    settlements.append(
                        result.model_dump(mode="json", exclude_unset=True)
                    )
                    if result.remaining or result.exit != 0 or result.errors:
                        raise RuntimeError("Host cleanup incomplete")
            check_root_consumers(plan.router_root, router.pid)
            if (plan.evidence / "stop.json").exists():
                break
    except BaseException as error:
        failure = {"class": type(error).__name__, "message": str(error)}
        raise
    finally:
        router_result = None
        if router is not None:
            try:
                router_result = stop_owned(router, router_known).model_dump(
                    mode="json", exclude_unset=True
                )
            except (
                OSError,
                ValueError,
                RuntimeError,
                subprocess.SubprocessError,
            ) as error:
                router_result = {
                    "pid": router.pid,
                    "exit": router.poll(),
                    "cleanupUnknown": True,
                    "error": type(error).__name__,
                }
        if profile_changed:
            if plan.profile.read_bytes() == temporary_profile:
                _ = plan.profile.write_bytes(profile_original)
            else:
                failure = {
                    "class": "ConcurrentProfileChange",
                    "message": "original preserved in memory; newer profile left untouched",
                }
        save_record(
            plan.evidence / "lifecycle-settlement.json",
            {
                "failure": failure,
                "hosts": settlements,
                "router": router_result,
                "profileRestoredExactly": plan.profile.read_bytes() == profile_original,
                "evidencePreserved": True,
            },
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("plan", type=Path)
    _ = parser.add_argument(
        "--check",
        action="store_true",
        help="Read-only exact live identity, hash, health and Host publication check",
    )
    arguments = RuntimeArguments.model_validate(vars(parser.parse_args()))
    plan = RuntimePlan.model_validate_json(arguments.plan.read_text())
    if arguments.check:
        print(json.dumps(verify_ready(plan)))
    else:
        run_lifecycle(plan)
