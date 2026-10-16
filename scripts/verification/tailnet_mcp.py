"""Local apparatus for one isolated Host and explicit peer board operations.

Run as ``python -m scripts.verification.tailnet_mcp <action> <plan.json>``.
Actions: host, peer, readback. The caller owns SSH and reciprocal sequencing.
The host writes host-ready.json, then waits for evidence/stop.json or deadline.
Peer requires an explicitly supplied destination topic and unique marker; it never
creates a project or board. No failed/uncertain mutation is retried. Readback runs
locally at the destination and reads its owner-only socket and board database.
Unit tests qualify this apparatus only. Live proof needs a final target/lease.
"""

import argparse
import signal
import typing as t
from pathlib import Path

from scripts.verification.tailnet_mcp_client import run_peer, run_readback
from scripts.verification.tailnet_mcp_host import run_host
from scripts.verification.tailnet_mcp_models import BoardPlan, DriverArguments, HostPlan


def terminate_run(_number: int, _frame: object) -> t.NoReturn:
    raise KeyboardInterrupt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("action", choices=["host", "peer", "readback"])
    _ = parser.add_argument("plan", type=Path)
    arguments = DriverArguments.model_validate(vars(parser.parse_args()))
    _ = signal.signal(signal.SIGTERM, terminate_run)
    data = arguments.plan.read_bytes()
    if arguments.action == "host":
        run_host(HostPlan.model_validate_json(data))
    elif arguments.action == "peer":
        run_peer(BoardPlan.model_validate_json(data))
    else:
        run_readback(BoardPlan.model_validate_json(data))


if __name__ == "__main__":
    main()
