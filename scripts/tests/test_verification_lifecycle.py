"""Controlled harmless-process verification apparatus regressions, never app proof."""

import datetime as dt
import hashlib
import json
import os
import selectors
import socket
import subprocess
import sys
import tempfile
import time
import typing as t
import unittest
from collections import abc
from contextlib import contextmanager
from pathlib import Path

from scripts.verification import (
    native_correlation,
    owned_runtime,
    process_inventory,
    runtime_qualification,
)
from scripts.verification.native_correlation import read_carrier_correlation
from scripts.verification.process_ownership import (
    lookup_process_identity,
    same_identity,
)
from scripts.verification.typed_observations import (
    BinaryQualification,
    IdentityRecord,
    JsonObject,
    NativeExpectation,
    ProcessIdentity,
    QualifiedBinary,
    observed_object,
    read_observed_json,
)


class LifecycleFixture:
    """Exercise the controller with harmless children and scoped external-I/O fixtures."""

    UTC: t.ClassVar[dt.timezone] = dt.UTC
    TimeoutExpired: t.ClassVar[type[subprocess.TimeoutExpired]] = (
        subprocess.TimeoutExpired
    )
    SubprocessError: t.ClassVar[type[subprocess.SubprocessError]] = (
        subprocess.SubprocessError
    )

    def __init__(self, root: Path, scenario: str, *, mismatch: bool = False) -> None:
        self.scenario: str = scenario
        self.now_value: dt.datetime = dt.datetime.now(dt.UTC)
        self.ready: bool = False
        self.exited: bool = False
        self.phases: list[str] = []
        self.launched: list[str] = []
        self.children: list[subprocess.Popen[bytes]] = []
        self.identities: dict[int, IdentityRecord] = {}
        self.datetime: LifecycleFixture = self
        self.original_save: abc.Callable[[Path, object], None] = (
            owned_runtime.save_record
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
        if self.ready and self.scenario == "deadline":
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
        if path.name == "host-A-ready.json":
            self.ready = True
            if self.scenario in {"stop", "reopen", "reopen-expired"}:
                _ = (
                    self.plan.evidence
                    / ("stop.json" if self.scenario == "stop" else "reopen.json")
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


class GlobalClaudeQualificationTests(unittest.TestCase):
    @staticmethod
    def executable(directory: Path) -> Path:
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "claude"
        _ = path.write_bytes(b"owned qualification fixture; never executed")
        path.chmod(0o700)
        return path

    def test_canonical_global_binding_records_exact_file_hash(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            executable = self.executable(root / "global-bin")
            planned_alias = root / "planned-claude"
            planned_alias.symlink_to(executable)
            binding = runtime_qualification.qualify_global_claude(
                planned_alias, str(executable.parent)
            )
            self.assertEqual(binding.canonicalPath, executable.resolve())
            self.assertEqual(
                binding.sha256,
                hashlib.sha256(
                    b"owned qualification fixture; never executed"
                ).hexdigest(),
            )

    def test_relative_package_and_canonical_package_alias_entries_are_skipped(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            package = self.executable(root / "project/node_modules/.bin")
            alias = root / "package-alias"
            alias.symlink_to(package.parent, target_is_directory=True)
            global_file = self.executable(root / "global-bin")
            search = os.pathsep.join(
                [
                    "relative/bin",
                    str(package.parent),
                    str(alias),
                    str(global_file.parent),
                ]
            )
            self.assertEqual(
                runtime_qualification.resolve_global_claude(search),
                global_file.resolve(),
            )

    def test_transient_and_sdk_target_aliases_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for index, relative in enumerate(
                [
                    "_npx/fixture",
                    "dlx/fixture",
                    "node_modules/claude-agent-sdk-darwin-arm64",
                    "node_modules/@anthropic-ai+claude-agent-sdk-fixture",
                ]
            ):
                with self.subTest(relative=relative):
                    target = self.executable(root / relative)
                    directory = root / f"alias-{index}"
                    directory.mkdir()
                    (directory / "claude").symlink_to(target)
                    with self.assertRaisesRegex(
                        RuntimeError, "unavailable on Host PATH"
                    ):
                        _ = runtime_qualification.resolve_global_claude(str(directory))

    def test_normal_global_claude_code_package_target_remains_allowed(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = self.executable(
                root / "global/lib/node_modules/@anthropic-ai/claude-code/bin"
            )
            directory = root / "global/bin"
            directory.mkdir()
            (directory / "claude").symlink_to(target)
            self.assertEqual(
                runtime_qualification.resolve_global_claude(str(directory)),
                target.resolve(),
            )

    def test_missing_empty_and_relative_path_do_not_fall_back(self) -> None:
        for search in [None, "", "relative/bin"]:
            with (
                self.subTest(search=search),
                self.assertRaisesRegex(RuntimeError, "unavailable on Host PATH"),
            ):
                _ = runtime_qualification.resolve_global_claude(search)

    def test_nonexecutable_directory_and_broken_candidates_are_skipped(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            nonexecutable = self.executable(root / "nonexecutable")
            nonexecutable.chmod(0o600)
            directory = root / "directory"
            (directory / "claude").mkdir(parents=True)
            broken = root / "broken"
            broken.mkdir()
            (broken / "claude").symlink_to(root / "missing")
            for candidate in [nonexecutable.parent, directory, broken]:
                with (
                    self.subTest(candidate=candidate),
                    self.assertRaisesRegex(RuntimeError, "unavailable on Host PATH"),
                ):
                    _ = runtime_qualification.resolve_global_claude(str(candidate))


@contextmanager
def harmless_child() -> abc.Generator[
    tuple[subprocess.Popen[bytes], ProcessIdentity], None, None
]:
    _ = lookup_process_identity(os.getpid())
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(60)"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    recorded = lookup_process_identity(child.pid)
    assert recorded is not None
    identity = ProcessIdentity.model_validate({**recorded, "parent": os.getpid()})
    try:
        yield child, identity
    finally:
        if child.poll() is None:
            if not same_identity(lookup_process_identity(child.pid), recorded):
                raise RuntimeError("test child identity changed; no signal")
            child.terminate()
        _ = child.wait(timeout=5)


class ControlledLifecycleTests(unittest.TestCase):
    def test_reopen_cannot_launch_host_b_after_cleanup_expires_allocation(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            fixture = LifecycleFixture(Path(folder), "reopen-expired")
            self.assertIsInstance(fixture.drive(), TimeoutError)
            self.assertEqual(fixture.phases, ["A"])
            settlement = read_observed_json(
                (fixture.plan.evidence / "lifecycle-settlement.json").read_text()
            )
            self.assertIsNotNone(settlement["failure"])

    def test_deadline_cannot_launch_unrequested_host_b_or_record_success(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            fixture = LifecycleFixture(Path(folder), "deadline")
            error = fixture.drive()
            self.assertEqual(fixture.phases, ["A"])
            self.assertIsInstance(error, TimeoutError)
            self.assertIn("Host A allocation deadline expired", str(error))
            settlement = read_observed_json(
                (fixture.plan.evidence / "lifecycle-settlement.json").read_text()
            )
            self.assertIsNotNone(settlement["failure"])

    def test_clean_unexpected_exit_cannot_launch_host_b_or_record_success(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            fixture = LifecycleFixture(Path(folder), "early-exit")
            error = fixture.drive()
            self.assertEqual(fixture.phases, ["A"])
            self.assertIsInstance(error, RuntimeError)
            self.assertIn(
                "Host A exited before an explicit lifecycle directive", str(error)
            )
            settlement = read_observed_json(
                (fixture.plan.evidence / "lifecycle-settlement.json").read_text()
            )
            self.assertIsNotNone(settlement["failure"])

    def test_claude_path_mismatch_rejects_before_any_requester(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            fixture = LifecycleFixture(Path(folder), "stop", mismatch=True)
            error = fixture.drive()
            self.assertEqual(fixture.launched, [])
            self.assertIsInstance(error, RuntimeError)
            self.assertIn(
                "planned Claude executable differs from qualified Host PATH selection",
                str(error),
            )

    def test_explicit_reopen_then_stop_is_the_only_two_host_journey(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            fixture = LifecycleFixture(Path(folder), "reopen")
            self.assertIsNone(fixture.drive())
            self.assertEqual(fixture.phases, ["A", "B"])
            settlement = read_observed_json(
                (fixture.plan.evidence / "lifecycle-settlement.json").read_text()
            )
            self.assertIsNone(settlement["failure"])
            binding = observed_object(
                read_observed_json(
                    (fixture.plan.evidence / "driver-binding.json").read_text()
                )["claudeGlobalExecutable"]
            )
            self.assertEqual(
                binding["canonicalPath"], str(fixture.plan.claude_executable.resolve())
            )
            self.assertEqual(
                binding["sha256"],
                hashlib.sha256(b"owned executable fixture; never executed").hexdigest(),
            )

    def test_native_metadata_failure_still_closes_and_reaps_owned_pipe(self) -> None:
        _ = lookup_process_identity(os.getpid())
        child = subprocess.Popen(
            [sys.executable, "-c", "import sys; sys.stdin.buffer.read()"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            bufsize=0,
        )
        original = native_correlation.lookup_process_identity

        def denied(_pid: int) -> IdentityRecord | None:
            raise PermissionError("controlled metadata failure")

        try:
            native_correlation.lookup_process_identity = denied
            with self.assertRaises(PermissionError):
                _ = read_carrier_correlation(
                    child,
                    NativeExpectation(
                        thread_id="fixture", turn_id="fixture", push_id="fixture"
                    ),
                )
        finally:
            native_correlation.lookup_process_identity = original
        self.assertEqual(child.returncode, 0)

    def test_recorded_harmless_child_is_released(self) -> None:
        with harmless_child() as (child, identity):
            result = owned_runtime.stop_owned(child, {child.pid: identity})
            self.assertEqual(result.remaining, {})
            self.assertEqual(result.errors, [])
            self.assertIsNotNone(child.poll())

    def test_denied_observation_retains_unverified_inventory_without_signal(
        self,
    ) -> None:
        with harmless_child() as (child, identity):
            original = owned_runtime.process_table

            def denied() -> dict[int, ProcessIdentity]:
                raise PermissionError("controlled unavailable observation")

            try:
                owned_runtime.process_table = denied
                result = owned_runtime.stop_owned(child, {child.pid: identity})
            finally:
                owned_runtime.process_table = original
            self.assertIn(child.pid, result.remaining)
            self.assertEqual(result.signals, [])
            self.assertTrue(result.errors)
            self.assertIsNone(child.poll())

    def test_skipped_inventory_poll_never_defaults_to_success(self) -> None:
        with (
            harmless_child() as (child, identity),
            tempfile.TemporaryDirectory() as folder,
        ):
            evidence = Path(folder)
            # Only the inventory action is exercised; this plan never launches an app.
            plan = owned_runtime.RuntimePlan(
                repository=evidence,
                source_head="0" * 40,
                qualification=evidence / "unused",
                router_root=evidence / "unused-root",
                evidence=evidence,
                port=43128,
                original_port=43127,
                expected_version="test-only",
                profile=evidence / "unused-profile",
                claude_node=Path(sys.executable),
                claude_adapter=evidence / "unused-adapter",
                claude_executable=Path(sys.executable),
                cursor_executable=Path(sys.executable),
                deadline=dt.datetime.now(dt.UTC),
            )
            inventory = evidence / "owned.json"
            owned_runtime.save_record(
                inventory, {"identities": {str(child.pid): identity.model_dump()}}
            )
            original = process_inventory.monotonic
            ticks = iter([0.0, 100.0, 200.0, 300.0])
            try:
                process_inventory.monotonic = lambda: next(ticks)
                with self.assertRaisesRegex(RuntimeError, "cleanup incomplete"):
                    process_inventory.settle_inventory(plan, inventory)
            finally:
                process_inventory.monotonic = original
            settlement = read_observed_json(
                (evidence / "owned-settlement.json").read_text()
            )
            self.assertIn(str(child.pid), observed_object(settlement["remaining"]))

    def test_actual_local_pipe_drives_only_synthetic_read_protocol(self) -> None:
        # This is a local apparatus fixture, never native/provider acceptance evidence.
        code = """import json, sys
for line in sys.stdin:
    message = json.loads(line)
    if message.get("id") == 1:
        print(json.dumps({"id": 1, "result": {}}), flush=True)
    elif message.get("id") == 2:
        thread = {"id": "fixture-thread", "turns": [{"id": "fixture-turn", "items": [{"type": "userMessage", "id": "fixture-native-item", "clientId": "fixture-push", "content": [{"type": "text", "text": "fixture"}]}]}]}
        print(json.dumps({"id": 2, "result": {"thread": thread}}), flush=True)
"""
        _ = lookup_process_identity(os.getpid())
        child = subprocess.Popen(
            [sys.executable, "-u", "-c", code],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            bufsize=0,
        )
        result = read_carrier_correlation(
            child,
            NativeExpectation(
                thread_id="fixture-thread",
                turn_id="fixture-turn",
                push_id="fixture-push",
            ),
        )
        self.assertTrue(result["supported"])
        self.assertEqual(child.returncode, 0)
