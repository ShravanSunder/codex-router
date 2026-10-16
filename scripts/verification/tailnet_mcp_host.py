"One isolated Host lifetime, guarded profile projection and owned cleanup."

import datetime as dt
import fcntl
import hashlib
import os
import pwd
import re
import signal
import socket
import stat
import subprocess
import time
from collections import abc
from contextlib import contextmanager
from pathlib import Path

from pydantic import (
    JsonValue,
)

from scripts.verification.owned_runtime import (
    observe_owned,
    process_table,
    save_record,
    stop_owned,
)
from scripts.verification.process_ownership import (
    lookup_process_identity,
    same_identity,
    signal_each_owned,
)
from scripts.verification.runtime_qualification import sha256
from scripts.verification.tailnet_mcp_client import (
    observe_http_boundary,
    observe_local_inventory,
    observe_native_readiness,
    tool,
)
from scripts.verification.tailnet_mcp_models import (
    ArtifactQualification,
    CleanupInventory,
    CleanupInventoryObserved,
    CleanupInventoryUnavailable,
    HostPlan,
    HttpsEndpoint,
    LoopbackEndpoint,
    ManifestMcpSelector,
    NativeReadiness,
    require_deadline,
)
from scripts.verification.typed_observations import (
    JsonObject,
    ProcessIdentity,
    read_observed_json,
)


def host_readiness_until(
    allocation_deadline: dt.datetime, monotonic_now: float, utc_now: dt.datetime
) -> float:
    # 2*5s preflight +20s advertisement +5s status +75s MCP +40s native margin.
    # This initial observation cap never extends the caller's allocation.
    return monotonic_now + min(150.0, (allocation_deadline - utc_now).total_seconds())


def validate_fresh_root(root: Path) -> None:
    if (
        root.parent != Path("/private/tmp")
        or root.exists()
        or root.is_symlink()
        or not re.fullmatch(r"[a-zA-Z0-9_-]{1,48}", root.name)
    ):
        raise ValueError("fresh root must be a new direct /private/tmp child")


def require_hash(path: Path, expected: str) -> None:
    if not path.is_absolute() or sha256(path) != expected:
        raise ValueError("artifact hash mismatch")


def command(argv: list[str]) -> bytes:
    completed = subprocess.run(argv, capture_output=True, check=True, timeout=10)
    if len(completed.stdout) + len(completed.stderr) > 4 * 1024 * 1024:
        raise ValueError("subprocess output exceeded evidence bound")
    return completed.stdout


def qualify_artifacts(plan: HostPlan) -> dict[str, Path]:
    require_hash(plan.qualification, plan.qualification_sha256)
    qualified = ArtifactQualification.model_validate_json(
        plan.qualification.read_bytes()
    )
    if qualified.sourceHead != plan.source_head:
        raise ValueError("qualification source mismatch")
    if (
        command(["git", "-C", str(plan.repository), "rev-parse", "HEAD"])
        .decode()
        .strip()
        != plan.source_head
    ):
        raise ValueError("source HEAD mismatch")
    result: dict[str, Path] = {}
    for artifact in qualified.artifacts:
        if artifact.name not in {
            "codex-router",
            "agent-collaboration",
            "agent-sessions",
        }:
            raise ValueError("unexpected qualification artifact")
        require_hash(artifact.path, artifact.sha256)
        if (
            set(artifact.features) - {"default"}
            or artifact.version != f"{artifact.name} {plan.expected_version}"
        ):
            raise ValueError("not the declared normal artifact")
        _ = command(["/usr/bin/codesign", "--verify", "--strict", str(artifact.path)])
        signing = subprocess.run(
            ["/usr/bin/codesign", "-dv", "--verbose=4", str(artifact.path)],
            capture_output=True,
            check=True,
            timeout=10,
        ).stderr.decode()
        if (
            f"Identifier=dev.shravansunder.{artifact.name}.debug" not in signing
            or "TeamIdentifier=974QD84WVC" not in signing
        ):
            raise ValueError("debug signing identity mismatch")
        if (
            command([str(artifact.path), "--version"]).decode().strip()
            != artifact.version
        ):
            raise ValueError("actual artifact version mismatch")
        result[artifact.name] = artifact.path
    if len(qualified.artifacts) != 3 or len(result) != 3:
        raise ValueError("three distinct normal artifacts are required")
    return result


@contextmanager
def project_profile(
    path: Path, expected_hash: str, port: int
) -> abc.Generator[None, None, None]:
    """Advisory lock plus inode/byte guards; leave any newer owner bytes untouched."""
    descriptor = os.open(path, os.O_RDWR | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "r+b", buffering=0) as profile:
        fcntl.flock(profile, fcntl.LOCK_EX | fcntl.LOCK_NB)
        identity = os.fstat(profile.fileno())
        if (
            identity.st_uid != os.getuid()
            or not stat.S_ISREG(identity.st_mode)
            or identity.st_mode & 0o022
        ):
            raise ValueError("unsafe debug profile owner or mode")
        original = profile.read()
        if hashlib.sha256(original).hexdigest() != expected_hash:
            raise ValueError("debug profile hash mismatch")
        endpoint = re.compile(rb"http://127\.0\.0\.1:[0-9]+/v1")
        if len(endpoint.findall(original)) != 1:
            raise ValueError("debug profile must contain exactly one loopback endpoint")
        projected = endpoint.sub(f"http://127.0.0.1:{port}/v1".encode(), original)

        def replace_guarded(expected: bytes, replacement: bytes) -> None:
            current = path.lstat()
            _ = profile.seek(0)
            if (
                current.st_ino != identity.st_ino
                or current.st_dev != identity.st_dev
                or stat.S_ISLNK(current.st_mode)
                or profile.read() != expected
            ):
                raise RuntimeError("debug profile conflict; newer bytes left untouched")
            _ = profile.seek(0)
            _ = profile.write(replacement)
            _ = profile.truncate()
            os.fsync(profile.fileno())

        replace_guarded(original, projected)
        try:
            yield
        finally:
            replace_guarded(projected, original)


def serve_snapshot(tailscale: Path) -> JsonObject:
    return read_observed_json(command([str(tailscale), "serve", "status", "--json"]))


def contains_port(value: JsonValue, port: int) -> bool:
    return isinstance(value, dict) and any(
        key == str(port) or key.endswith(f":{port}") or contains_port(child, port)
        for key, child in value.items()
    )


def require_serve_during(before: JsonObject, during: JsonObject, port: int) -> None:
    def unchanged_subset(previous: JsonValue, current: JsonValue) -> bool:
        if isinstance(previous, dict) and isinstance(current, dict):
            return all(
                key in current and unchanged_subset(value, current[key])
                for key, value in previous.items()
            )
        return previous == current

    def target_funnel(value: JsonValue) -> bool:
        if not isinstance(value, dict):
            return False
        funnel = value.get("AllowFunnel")
        return (
            isinstance(funnel, dict)
            and any(
                enabled is True and (key == str(port) or key.endswith(f":{port}"))
                for key, enabled in funnel.items()
            )
        ) or any(target_funnel(child) for child in value.values())

    if (
        contains_port(before, port)
        or not contains_port(during, port)
        or not unchanged_subset(before, during)
        or target_funnel(during)
    ):
        raise ValueError(
            "Serve collision, unrelated overwrite, missing exposure or public Funnel"
        )


def require_serve_restored(before: JsonObject, after: JsonObject) -> None:
    if before != after:
        raise ValueError(
            "Serve state was not restored exactly; no reset or blind replay permitted"
        )


def require_http_serve_backend(
    during: JsonObject, local: LoopbackEndpoint, remote: HttpsEndpoint
) -> str:
    foreground = during.get("Foreground")
    if not isinstance(foreground, dict):
        raise TypeError("owned HTTP foreground Serve session absent")
    port = remote.authority.rsplit(":", 1)[1]
    matches: list[str] = []
    for session_id, config in foreground.items():
        if not isinstance(config, dict):
            continue
        tcp, web, funnel = (
            config.get("TCP"),
            config.get("Web"),
            config.get("AllowFunnel", {}),
        )
        if (
            not isinstance(tcp, dict)
            or not isinstance(web, dict)
            or not isinstance(funnel, dict)
        ):
            continue
        listener, endpoint = tcp.get(port), web.get(remote.authority)
        if (
            not isinstance(listener, dict)
            or listener.get("HTTPS") is not True
            or not isinstance(endpoint, dict)
        ):
            continue
        handlers = endpoint.get("Handlers")
        handler = handlers.get("/") if isinstance(handlers, dict) else None
        proxy = handler.get("Proxy") if isinstance(handler, dict) else None
        if (
            isinstance(proxy, str)
            and proxy.rstrip("/") == local.origin
            and funnel.get(remote.authority, False) is False
            and session_id
        ):
            matches.append(session_id)
    if len(matches) != 1:
        raise ValueError(
            "expected exactly one private foreground HTTP backend matching the actual MCP listener"
        )
    return matches[0]


def capture_cleanup_inventory(
    known: dict[int, ProcessIdentity],
    read_processes: abc.Callable[[], dict[int, ProcessIdentity]] = process_table,
) -> CleanupInventory:
    recorded = list(known.values())
    try:
        observed = read_processes()
        if os.getpid() not in observed:
            raise RuntimeError("process inventory omitted the active verifier")
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        return CleanupInventoryUnavailable(
            recorded=recorded, error=type(error).__name__
        )
    return CleanupInventoryObserved(
        recorded=recorded,
        matching=[
            current
            for pid, current in observed.items()
            if pid in known
            and same_identity(current.as_record(), known[pid].as_record())
        ],
    )


@contextmanager
def owned_child(
    child: subprocess.Popen[bytes], evidence: Path
) -> abc.Generator[dict[int, ProcessIdentity], None, None]:
    known: dict[int, ProcessIdentity] = {}
    try:
        observe_owned(child.pid, known)
        if child.pid not in known:
            raise RuntimeError("owned Host identity unavailable")
        save_record(
            evidence / "host-started.json", known[child.pid].model_dump(mode="json")
        )
        yield known
    finally:
        try:
            observe_owned(child.pid, known)
            snapshot = capture_cleanup_inventory(known)
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            snapshot = CleanupInventoryUnavailable(
                recorded=list(known.values()), error=type(error).__name__
            )
        saved_snapshot = True
        try:
            save_record(
                evidence / "host-before-fallback.json", snapshot.model_dump(mode="json")
            )
        except OSError:
            saved_snapshot = False
        root = known.get(child.pid)
        if root is not None and child.poll() is None:
            requested = signal_each_owned(
                {child.pid: root.as_record()},
                lookup_process_identity,
                lambda pid: os.kill(pid, signal.SIGTERM),
            )
            if not requested.errors:
                try:
                    _ = child.wait(timeout=45)
                except subprocess.TimeoutExpired:
                    pass
        result = stop_owned(child, known)
        save_record(evidence / "host-settlement.json", result.model_dump(mode="json"))
        if result.remaining or result.errors:
            raise RuntimeError("owned cleanup incomplete; inspect settlement")
        if not saved_snapshot or snapshot.kind == "unavailable":
            raise RuntimeError(
                "cleanup evidence unavailable; owned cleanup was still attempted"
            )


def require_protected(protected: list[ProcessIdentity]) -> None:
    observed = process_table()
    for identity in protected:
        current = observed.get(identity.pid)
        if not same_identity(
            current.as_record() if current else None, identity.as_record()
        ):
            raise RuntimeError("protected process changed or became unobservable")


def parse_manifest_mcp(manifest: JsonObject) -> LoopbackEndpoint:
    return ManifestMcpSelector.model_validate(manifest.get("mcp"))


def run_host(plan: HostPlan) -> None:
    if os.geteuid() == 0:
        raise ValueError("the verification Host must remain unprivileged")
    normal_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    if Path.home() != normal_home:
        raise ValueError("normal owner HOME must be preserved")
    validate_fresh_root(plan.router_root)
    binaries = qualify_artifacts(plan)
    require_protected(plan.protected)
    if os.environ.get("CODEX_HOME", str(Path.home() / ".codex")) != str(
        Path.home() / ".codex"
    ):
        raise ValueError("normal Codex home must be preserved")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", plan.port))
    before = serve_snapshot(plan.tailscale)
    if contains_port(before, plan.https_port):
        raise ValueError("selected Serve port occupied")
    plan.router_root.mkdir(mode=0o700)
    plan.evidence.mkdir(mode=0o700)
    native = plan.router_root / "native"
    native.mkdir(mode=0o700)
    save_record(
        plan.evidence / "driver-binding.json",
        {
            "driverSHA256": sha256(Path(__file__)),
            "plan": plan.model_dump(mode="json"),
            "driverFiles": {
                name: sha256(Path(__file__).with_name(name))
                for name in [
                    "tailnet_mcp.py",
                    "tailnet_mcp_models.py",
                    "tailnet_mcp_client.py",
                    "tailnet_mcp_host.py",
                    "owned_runtime.py",
                    "process_ownership.py",
                    "runtime_qualification.py",
                    "typed_observations.py",
                ]
            },
        },
    )
    providers: JsonObject = {
        "version": 1,
        "providers": {
            name: {"enabled": False, "executable": None, "arguments": []}
            for name in ["claude", "cursor"]
        },
    }
    save_record(plan.router_root / "providers.json", providers)
    save_record(plan.evidence / "serve-before.json", before)
    profile = Path.home() / ".codex/codex-router-debug.config.toml"
    failure: str | None = None
    cleanup_errors: list[str] = []
    try:
        with project_profile(profile, plan.profile_sha256, plan.port):
            environment = os.environ.copy()
            environment["CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET"] = str(
                native / "app-server.sock"
            )
            argv = [
                str(binaries["codex-router"]),
                "host",
                "--router-root",
                str(plan.router_root),
                "--port",
                str(plan.port),
                "--mcp-bind",
                "127.0.0.1:0",
                "--require-debug-isolation",
                "--remote-access",
                "tailscale",
                "--tailscale-https-port",
                str(plan.https_port),
            ]
            require_deadline(plan.deadline)
            with (
                (plan.evidence / "host.stdout").open("xb") as stdout,
                (plan.evidence / "host.stderr").open("xb") as stderr,
            ):
                child = subprocess.Popen(
                    argv,
                    env=environment,
                    stdout=stdout,
                    stderr=stderr,
                    start_new_session=True,
                )
            with owned_child(child, plan.evidence) as known:
                if (
                    Path(known[child.pid].executable).resolve()
                    != binaries["codex-router"].resolve()
                ):
                    raise ValueError("actual Host image differs from declared binary")
                ready = False
                invocation: tuple[str, str] | None = None
                until = host_readiness_until(
                    plan.deadline, time.monotonic(), dt.datetime.now(dt.UTC)
                )
                while time.monotonic() < until:
                    require_deadline(plan.deadline)
                    if (plan.evidence / "stop.json").exists():
                        raise RuntimeError("explicit stop before readiness")
                    if child.poll() is not None:
                        raise RuntimeError("Host exited before readiness")
                    observe_owned(child.pid, known)
                    manifest_path = plan.service / "service.json"
                    if manifest_path.exists():
                        manifest = read_observed_json(manifest_path.read_bytes())
                        if (
                            manifest.get("serviceVersion") != plan.expected_version
                            or manifest.get("routerProxyEndpoint")
                            != f"127.0.0.1:{plan.port}"
                        ):
                            raise ValueError("Host manifest binding mismatch")
                        service_id = manifest.get("serviceId")
                        service_epoch = manifest.get("serviceEpoch")
                        if (
                            not isinstance(service_id, str)
                            or not service_id
                            or not isinstance(service_epoch, str)
                            or not service_epoch
                        ):
                            raise ValueError("Host service identity absent")
                        if invocation is None:
                            invocation = (service_id, service_epoch)
                        elif invocation != (service_id, service_epoch):
                            raise ValueError("Host invocation changed during readiness")
                        local = parse_manifest_mcp(manifest)
                        inventory = tool(
                            plan.service / "control.sock", "endpoints_list", {}
                        )
                        if (
                            observe_native_readiness(
                                inventory, service_id, service_epoch
                            )
                            == NativeReadiness.STARTING
                        ):
                            time.sleep(0.1)
                            continue
                        during = serve_snapshot(plan.tailscale)
                        require_serve_during(before, during, plan.https_port)
                        serve_session = require_http_serve_backend(
                            during, local, plan.https
                        )
                        boundary = observe_http_boundary(local, plan.https)
                        local_inventory = observe_local_inventory(
                            local, plan.https, (service_id, service_epoch)
                        )
                        save_record(plan.evidence / "serve-during.json", during)
                        save_record(
                            plan.evidence / "host-ready.json",
                            {
                                "manifest": manifest,
                                "inventory": inventory,
                                "localInventory": local_inventory,
                                "https": plan.https.url,
                                "peerTls": "unobserved; requires the separate peer journey",
                                "httpBoundary": boundary,
                                "ownedServeSession": serve_session,
                                "nativeActions": "none; endpoint/generation readiness only",
                            },
                        )
                        ready = True
                        break
                    time.sleep(0.1)
                if not ready:
                    raise TimeoutError("Host readiness deadline")
                while not (plan.evidence / "stop.json").exists():
                    require_deadline(plan.deadline)
                    if child.poll() is not None:
                        raise RuntimeError("Host exited without an explicit stop")
                    observe_owned(child.pid, known)
                    require_protected(plan.protected)
                    time.sleep(0.2)
            settlement = read_observed_json(
                (plan.evidence / "host-settlement.json").read_bytes()
            )
            if settlement.get("exit") != 0 or settlement.get("signals"):
                raise RuntimeError(
                    "Host required forced/descendant cleanup or exited with failure"
                )
    except BaseException as error:
        failure = f"{type(error).__name__}: {error}"
        raise
    finally:

        def check_serve() -> None:
            after = serve_snapshot(plan.tailscale)
            save_record(plan.evidence / "serve-after.json", after)
            require_serve_restored(before, after)

        for check in [check_serve, lambda: require_protected(plan.protected)]:
            try:
                check()
            except (
                OSError,
                ValueError,
                RuntimeError,
                subprocess.SubprocessError,
            ) as error:
                cleanup_errors.append(f"{type(error).__name__}: {error}")
        restored = False
        try:
            restored = sha256(profile) == plan.profile_sha256
        except OSError:
            cleanup_errors.append("debug profile became unreadable")
        save_record(
            plan.evidence / "lifecycle-settlement.json",
            {
                "failure": failure,
                "cleanupErrors": cleanup_errors,
                "profileRestoredExactly": restored,
            },
        )
        if cleanup_errors:
            raise RuntimeError("lifecycle cleanup incomplete; inspect settlement")
