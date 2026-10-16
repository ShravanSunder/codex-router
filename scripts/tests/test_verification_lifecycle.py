"""Controlled harmless-process verification apparatus regressions, never app proof."""

import datetime as dt
import os
import subprocess
import sys
import tempfile
import unittest
from collections import abc
from contextlib import contextmanager
from pathlib import Path

from scripts.verification import native_correlation, owned_runtime, process_inventory
from scripts.verification.native_correlation import read_carrier_correlation
from scripts.verification.process_ownership import (
    lookup_process_identity,
    same_identity,
)
from scripts.verification.typed_observations import (
    IdentityRecord,
    NativeExpectation,
    ProcessIdentity,
    observed_object,
    read_observed_json,
)


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
