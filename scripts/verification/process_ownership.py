"""Stable process ownership for the Router verification lifecycle, independent of ancestry."""

import subprocess
from collections import abc

from scripts.verification.typed_observations import (
    IdentityRecord,
    SignalFailure,
    SignalResult,
)

type IdentityFields = abc.Mapping[str, object]


def lookup_process_identity(pid: int) -> IdentityRecord | None:
    result = subprocess.run(
        ["/bin/ps", "-p", str(pid), "-o", "pid=,uid=,pgid=,lstart=,comm="],
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )
    if result.returncode == 1 and not result.stdout.strip():
        return None
    result.check_returncode()
    fields = result.stdout.strip().split(None, 8)
    if len(fields) != 9:
        raise ValueError("incomplete current process identity")
    return {
        "pid": int(fields[0]),
        "uid": int(fields[1]),
        "group": int(fields[2]),
        "birth": " ".join(fields[3:8]),
        "executable": fields[8],
    }


def same_identity(current: IdentityFields | None, recorded: IdentityFields) -> bool:
    keys = ("pid", "uid", "group", "birth", "executable")
    if any(key not in recorded for key in keys):
        raise ValueError("incomplete recorded process identity")
    return current is not None and all(
        current.get(key) == recorded[key] for key in keys
    )


def owned_closure(
    root_pid: int,
    known: abc.Mapping[int, IdentityFields],
    observed: abc.Mapping[int, IdentityFields],
) -> set[int]:
    candidates = {
        pid
        for pid, identity in known.items()
        if same_identity(observed.get(pid), identity)
    }
    # The initial root is a retained, unreaped Popen child; later roots must match.
    if root_pid not in known and root_pid in observed:
        candidates.add(root_pid)
    while True:
        descendants = {
            pid
            for pid, item in observed.items()
            if item.get("parent") in candidates
            and (pid not in known or same_identity(item, known[pid]))
        }
        expanded = candidates | descendants
        if expanded == candidates:
            return expanded
        candidates = expanded


def remaining_owned(
    known: abc.Mapping[int, IdentityFields], observed: abc.Mapping[int, IdentityFields]
) -> dict[int, IdentityFields]:
    return {
        pid: identity
        for pid, identity in observed.items()
        if pid in known and same_identity(identity, known[pid])
    }


def signal_each_owned(
    known: abc.Mapping[int, IdentityFields],
    current_for: abc.Callable[[int], IdentityFields | None],
    send_signal: abc.Callable[[int], None],
) -> SignalResult:
    sent: list[int] = []
    errors: list[SignalFailure] = []
    for pid, identity in known.items():
        try:
            if same_identity(current_for(pid), identity):
                send_signal(pid)
                sent.append(pid)
        except ProcessLookupError:
            continue
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            errors.append(SignalFailure(pid=pid, error=type(error).__name__))
    return SignalResult(sent=sent, errors=errors)
