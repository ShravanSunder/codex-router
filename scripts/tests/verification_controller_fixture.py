"""Controlled private-file and harmless-child fixture for verification lifecycle tests."""

import datetime as dt
import hashlib
import json
import os
import selectors
import socket
import subprocess
import sys
import time
import typing as t
from collections import abc
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

from scripts.verification import owned_runtime, process_ownership, runtime_qualification
from scripts.verification.process_ownership import (
    lookup_process_identity,
    same_identity,
)
from scripts.verification.typed_observations import (
    BinaryQualification,
    IdentityRecord,
    JsonObject,
    QualifiedBinary,
)


@contextmanager
def lookup_response(
    result: subprocess.CompletedProcess[str],
) -> abc.Generator[None, None, None]:
    """Keep the actual lookup decoder; stand in only for the external ps result."""

    class LookupCommand:
        @staticmethod
        def run(
            _command: list[str], **_options: object
        ) -> subprocess.CompletedProcess[str]:
            return result

    with patch.object(process_ownership, "subprocess", LookupCommand):
        yield


class LifecycleFixture:
    """Exercise the controller with harmless children and scoped external-I/O fixtures."""

    UTC: t.ClassVar[dt.timezone] = dt.UTC
    TimeoutExpired: t.ClassVar[type[subprocess.TimeoutExpired]] = (
        subprocess.TimeoutExpired
    )
    SubprocessError: t.ClassVar[type[subprocess.SubprocessError]] = (
        subprocess.SubprocessError
    )
    owner_profile: t.ClassVar[bytes] = (
        b'url="http://127.0.0.1:43127/v1"\n# owner edit\n'
    )

    def __init__(self, root: Path, scenario: str, *, mismatch: bool = False) -> None:
        self.scenario: str = scenario
        self.now_value: dt.datetime = dt.datetime.now(dt.UTC)
        self.ready: bool = False
        self.exited: bool = False
        self.prelaunch_expired: bool = False
        self.phases: list[str] = []
        self.launched: list[str] = []
        self.children: list[subprocess.Popen[bytes]] = []
        self.identities: dict[int, IdentityRecord] = {}
        self.datetime: LifecycleFixture = self
        self.original_save: abc.Callable[[Path, object], None] = (
            owned_runtime.save_record
        )
        self.original_consumers: abc.Callable[[Path, int | None], None] = (
            owned_runtime.check_root_consumers
        )
        root_dir = root / "router-root"
        root_dir.mkdir(mode=0o700)
        binary_dir = root / "qualified"
        binary_dir.mkdir()
        binaries: list[JsonObject] = []
        for name in ["codex-router", "agent-collaboration", "agent-sessions"]:
            binary = binary_dir / name
            _ = binary.write_bytes(b"apparatus fixture; never executed")
            binaries.append(
                {
                    "target": name,
                    "path": str(binary),
                    "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                    "testGuardFeatures": [],
                }
            )
        qualification = root / "qualification.json"
        _ = qualification.write_text(json.dumps({"binaries": binaries}))
        profile = root / "debug-profile"
        _ = profile.write_text('url="http://127.0.0.1:43127/v1"\n')
        self.global_bin: Path = root / "global-bin"
        self.global_bin.mkdir()
        claude = self.global_bin / "claude"
        _ = claude.write_bytes(b"owned executable fixture; never executed")
        claude.chmod(0o700)
        planned = root / "different-claude" if mismatch else claude
        if mismatch:
            _ = planned.write_bytes(b"different owned fixture")
            planned.chmod(0o700)
        self.plan: owned_runtime.RuntimePlan = owned_runtime.RuntimePlan(
            repository=root,
            source_head="0" * 40,
            qualification=qualification,
            router_root=root_dir,
            evidence=root / "evidence",
            port=43128,
            original_port=43127,
            expected_version="fixture",
            profile=profile,
            claude_node=Path(sys.executable),
            claude_adapter=root / "unused-adapter",
            claude_executable=planned,
            cursor_executable=Path(sys.executable),
            deadline=self.now_value + dt.timedelta(hours=1),
        )

    def now(self, _zone: dt.tzinfo) -> dt.datetime:
        if self.prelaunch_expired or (self.ready and self.scenario == "deadline"):
            return self.plan.deadline + dt.timedelta(seconds=1)
        if self.ready and self.scenario == "early-exit" and not self.exited:
            child = self.children[1]
            assert child.stdin is not None
            child.stdin.close()
            _ = child.wait(timeout=5)
            self.exited = True
        return self.now_value

    @staticmethod
    def monotonic() -> float:
        return time.monotonic()

    @staticmethod
    def sleep(_seconds: float) -> None:
        pass

    @contextmanager
    def socket(self) -> abc.Generator["LifecycleFixture", None, None]:
        yield self

    def bind(self, _address: tuple[str, int]) -> None:
        pass

    def check_output(self, _command: list[str], **_options: object) -> str:
        return self.plan.source_head

    def run(
        self, command: list[str], **_options: object
    ) -> subprocess.CompletedProcess[str]:
        if command[0] == "/usr/bin/codesign":
            name = Path(command[-1]).name
            return subprocess.CompletedProcess(
                command,
                0,
                stdout="",
                stderr=f"Identifier=dev.shravansunder.{name}.debug\nTeamIdentifier=974QD84WVC\n",
            )
        if command[0] != "/bin/ps":
            raise AssertionError(f"unexpected external command: {command[0]}")
        return subprocess.run(
            command, capture_output=True, text=True, check=True, timeout=5
        )

    def Popen(self, command: list[str], **_options: object) -> subprocess.Popen[bytes]:
        kind = command[1]
        self.launched.append(kind)
        if kind == "host":
            phase = "A" if not self.phases else "B"
            self.phases.append(phase)
            publication = self.plan.router_root / "agent-communication/service.json"
            publication.parent.mkdir(exist_ok=True)
            _ = publication.write_text(json.dumps({"serviceVersion": "fixture"}))
        child = subprocess.Popen(
            [
                sys.executable,
                "-u",
                "-c",
                "import signal,sys; signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)); print('READY',flush=True); sys.stdin.buffer.read()",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        self.children.append(child)
        identity = lookup_process_identity(child.pid)
        assert identity is not None
        self.identities[child.pid] = identity
        assert child.stdout is not None
        with selectors.DefaultSelector() as selector:
            _ = selector.register(child.stdout, selectors.EVENT_READ)
            if not selector.select(5):
                raise AssertionError("harmless fixture child did not signal readiness")
            if child.stdout.readline() != b"READY\n":
                raise AssertionError("wrong harmless fixture readiness")
        return child

    def save(self, path: Path, value: object) -> None:
        self.original_save(path, value)
        if path.name == "router-ready.json" and self.scenario == "profile-beforewrite":
            _ = self.plan.profile.write_bytes(self.owner_profile)
        if path.name == "host-A-ready.json":
            self.ready = True
            if self.scenario == "profile-afterwrite":
                _ = self.plan.profile.write_bytes(self.owner_profile)
            if self.scenario in {
                "stop",
                "reopen",
                "reopen-expired",
                "profile-beforewrite",
                "profile-afterwrite",
            }:
                _ = (
                    self.plan.evidence
                    / (
                        "reopen.json"
                        if self.scenario in {"reopen", "reopen-expired"}
                        else "stop.json"
                    )
                ).write_text("{}")
        if path.name == "host-B-ready.json":
            _ = (self.plan.evidence / "stop.json").write_text("{}")
        if path.name in {"host-A-settlement.json", "host-B-settlement.json"}:
            (self.plan.router_root / "agent-communication/service.json").unlink(
                missing_ok=True
            )
            if self.scenario == "reopen-expired":
                self.scenario = "deadline"

    @staticmethod
    def health(_plan: owned_runtime.RuntimePlan) -> JsonObject:
        return {"binary_version": "fixture"}

    def consumers(self, root: Path, allowed_router: int | None = None) -> None:
        self.original_consumers(root, allowed_router)
        if (
            self.scenario == "inspection-expired"
            and allowed_router is not None
            and not self.phases
        ):
            self.prelaunch_expired = True

    @staticmethod
    def qualified_binaries(path: Path) -> dict[str, QualifiedBinary]:
        # External signing is outside the lifecycle/PATH claims in these fixtures.
        records = BinaryQualification.model_validate_json(path.read_text())
        return {record.target: record for record in records.binaries}

    def drive(self) -> Exception | None:
        originals: dict[str, object] = {
            "subprocess": subprocess,
            "dt": dt,
            "time": time,
            "socket": socket,
            "save_record": owned_runtime.save_record,
            "health_check": owned_runtime.health_check,
            "qualify_normal_binaries": runtime_qualification.qualify_normal_binaries,
            "check_root_consumers": self.original_consumers,
        }
        previous_path = os.environ.get("PATH")
        os.environ["PATH"] = str(self.global_bin)
        for name, replacement in {
            "subprocess": self,
            "dt": self,
            "time": self,
            "socket": self,
            "save_record": self.save,
            "health_check": self.health,
            "qualify_normal_binaries": self.qualified_binaries,
            "check_root_consumers": self.consumers,
        }.items():
            setattr(owned_runtime, name, replacement)
        failure: Exception | None = None
        try:
            owned_runtime.run_lifecycle(self.plan)
        except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
            failure = error
        finally:
            for name, original in originals.items():
                setattr(owned_runtime, name, original)
            if previous_path is None:
                _ = os.environ.pop("PATH", None)
            else:
                os.environ["PATH"] = previous_path
            for child in self.children:
                if child.poll() is None:
                    if not same_identity(
                        lookup_process_identity(child.pid), self.identities[child.pid]
                    ):
                        raise AssertionError(
                            "fixture child identity changed; no signal"
                        )
                    child.terminate()
                _ = child.wait(timeout=5)
                for pipe in [child.stdin, child.stdout, child.stderr]:
                    if pipe is not None:
                        pipe.close()
        return failure
