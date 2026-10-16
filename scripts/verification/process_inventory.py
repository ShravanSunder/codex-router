#!/usr/bin/env python3
"""Capture or settle one exact Router-verification process inventory."""

import argparse
import json
import os
import signal
import subprocess
from pathlib import Path
from time import monotonic, sleep

from pydantic import BaseModel

from scripts.verification.owned_runtime import (
    RuntimePlan,
    current_identity_fields,
    observe_owned,
    process_table,
    save_record,
    sha256,
)
from scripts.verification.process_ownership import (
    remaining_owned,
    same_identity,
    signal_each_owned,
)
from scripts.verification.typed_observations import (
    CleanupFailure,
    IdentityRecord,
    ProcessIdentity,
    ProcessSignal,
    read_observed_json,
)

__all__ = ["capture_root_inventory", "monotonic", "settle_inventory"]


class InventoryArguments(BaseModel):
    plan: Path
    capture_root: Path | None
    settle_inventory: Path | None
    inventory_output: Path | None


def capture_root_inventory(plan: RuntimePlan, root_record: Path, output: Path) -> None:
    if not root_record.resolve().is_relative_to(
        plan.evidence.resolve()
    ) or not output.resolve().is_relative_to(plan.evidence.resolve()):
        raise ValueError("inventory paths must stay in this run's evidence")
    value = read_observed_json(root_record.read_text())
    identity = ProcessIdentity.model_validate(value.get("identity", value))
    if not same_identity(current_identity_fields(identity.pid), identity.model_dump()):
        raise RuntimeError(
            "recorded root no longer matches; do not acquire descendants"
        )
    known = {identity.pid: identity}
    observe_owned(identity.pid, known)
    save_record(
        output,
        {
            "rootPID": identity.pid,
            "ownerDriverSHA256": sha256(Path(__file__)),
            "identities": {str(pid): item.model_dump() for pid, item in known.items()},
        },
    )
    print(
        json.dumps({"ownedInventory": str(output), "recordedCount": len(known)}),
        flush=True,
    )


def settle_inventory(plan: RuntimePlan, inventory_path: Path) -> None:
    if not inventory_path.resolve().is_relative_to(plan.evidence.resolve()):
        raise ValueError("inventory must belong to this run")
    inventory = read_observed_json(inventory_path.read_text())
    raw_identities = inventory["identities"]
    if not isinstance(raw_identities, dict):
        raise TypeError("recorded inventory is not an object")
    known = {
        int(pid): ProcessIdentity.model_validate(value).as_record()
        for pid, value in raw_identities.items()
    }
    signals: list[ProcessSignal] = []
    errors: list[CleanupFailure] = []
    for chosen_signal, bound in [(signal.SIGTERM, 45), (signal.SIGKILL, 5)]:
        result = signal_each_owned(
            known,
            current_identity_fields,
            lambda pid, number=chosen_signal: os.kill(pid, number),
        )
        signals.extend(
            ProcessSignal(pid=pid, signal=int(chosen_signal)) for pid in result.sent
        )
        errors.extend(
            CleanupFailure(pid=item.pid, error=item.error) for item in result.errors
        )
        remaining: dict[int, IdentityRecord] = dict(known)
        deadline = monotonic() + bound
        while monotonic() < deadline:
            try:
                current = {
                    pid: item.as_record() for pid, item in process_table().items()
                }
                remaining = {
                    pid: current[pid] for pid in remaining_owned(known, current)
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
            sleep(0.1)
        if not remaining:
            break
    save_record(
        inventory_path.with_name(inventory_path.stem + "-settlement.json"),
        {
            "signals": [
                item.model_dump(mode="json", exclude_unset=True) for item in signals
            ],
            "errors": [
                item.model_dump(mode="json", exclude_unset=True) for item in errors
            ],
            "remaining": remaining,
            "stableIdentityExcludesParent": True,
        },
    )
    if remaining or errors:
        raise RuntimeError("corrected inventory cleanup incomplete")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("plan", type=Path)
    actions = parser.add_mutually_exclusive_group(required=True)
    _ = actions.add_argument("--capture-root", type=Path)
    _ = actions.add_argument("--settle-inventory", type=Path)
    _ = parser.add_argument("--inventory-output", type=Path)
    arguments = InventoryArguments.model_validate(vars(parser.parse_args()))
    plan = RuntimePlan.model_validate_json(arguments.plan.read_text())
    if arguments.capture_root:
        if arguments.inventory_output is None:
            parser.error("--capture-root requires --inventory-output")
        capture_root_inventory(plan, arguments.capture_root, arguments.inventory_output)
    else:
        if arguments.settle_inventory is None:
            parser.error("inventory action missing")
        settle_inventory(plan, arguments.settle_inventory)
