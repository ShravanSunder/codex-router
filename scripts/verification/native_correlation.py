#!/usr/bin/env python3
"""Read one stored native thread; correlate clientId within the accepted turn."""

import argparse
import hashlib
import json
import os
import selectors
import subprocess
import time
from pathlib import Path

from pydantic import BaseModel

from scripts.verification.process_ownership import (
    lookup_process_identity,
    same_identity,
)
from scripts.verification.typed_observations import (
    IdentityRecord,
    JsonObject,
    NativeCorrelation,
    NativeExpectation,
    NativeMatch,
    NativeThread,
    observed_object,
    read_observed_json,
)

__all__ = [
    "correlate_message",
    "lookup_process_identity",
    "read_carrier_correlation",
    "read_correlation",
]


class CorrelationArguments(BaseModel):
    cli: Path
    service_directory: Path
    thread_id: str
    turn_id: str
    push_id: str
    output: Path


def correlate_message(thread: object, expected: object) -> JsonObject:
    view = NativeThread.model_validate(thread)
    expectation = NativeExpectation.model_validate(expected)
    matches: list[NativeMatch] = []
    for turn in view.turns:
        for item in turn.items:
            if (
                item.type == "userMessage"
                and item.clientId is not None
                and item.clientId == expectation.push_id
            ):
                texts: list[str] = []
                for part in item.content:
                    if part.type == "text":
                        if part.text is None:
                            raise ValueError("native text content missing")
                        texts.append(part.text)
                matches.append(
                    NativeMatch(
                        nativeId=item.id,
                        clientId=item.clientId,
                        turnId=turn.id,
                        textSHA256=hashlib.sha256(
                            "\n".join(texts).encode()
                        ).hexdigest(),
                    )
                )
    return observed_object(
        NativeCorrelation(
            threadId=view.id,
            matchingCount=len(matches),
            matches=matches,
            supported=view.id == expectation.thread_id
            and len(matches) == 1
            and matches[0].turnId == expectation.turn_id,
        ).model_dump(mode="json")
    )


def read_correlation(arguments: CorrelationArguments) -> JsonObject:
    _ = lookup_process_identity(os.getpid())
    argv = [
        str(arguments.cli),
        "native",
        "--endpoint",
        "codex-local",
        "--service-directory",
        str(arguments.service_directory),
    ]
    child = subprocess.Popen(
        argv,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
        bufsize=0,
    )
    return read_carrier_correlation(
        child,
        NativeExpectation(
            thread_id=arguments.thread_id,
            turn_id=arguments.turn_id,
            push_id=arguments.push_id,
        ),
    )


def read_carrier_correlation(
    child: subprocess.Popen[bytes], expected: NativeExpectation
) -> JsonObject:
    identity: IdentityRecord | None = None
    native_stdin, native_stdout = child.stdin, child.stdout
    if native_stdin is None or native_stdout is None:
        raise RuntimeError("native carrier pipes missing")
    pending = bytearray()
    deadline = time.monotonic() + 40
    selector = selectors.DefaultSelector()
    _ = selector.register(native_stdout, selectors.EVENT_READ)

    def request(method: str, params: JsonObject, request_id: int) -> JsonObject:
        _ = native_stdin.write(
            (
                json.dumps({"id": request_id, "method": method, "params": params})
                + "\n"
            ).encode()
        )
        native_stdin.flush()
        while time.monotonic() < deadline:
            if b"\n" not in pending:
                if not selector.select(0.2):
                    continue
                chunk = os.read(native_stdout.fileno(), 65536)
                if not chunk:
                    raise RuntimeError("native carrier closed")
                pending.extend(chunk)
                if len(pending) > 64 * 1024 * 1024:
                    raise RuntimeError("native capture cap")
                continue
            newline = pending.index(b"\n")
            frame = read_observed_json(bytes(pending[:newline]))
            del pending[: newline + 1]
            if "method" in frame:
                if "id" in frame:
                    raise RuntimeError("unexpected native callback; no decision made")
                continue
            if frame.get("id") != request_id or "error" in frame:
                raise RuntimeError(
                    "native read rejected or response correlation mismatch"
                )
            return observed_object(frame["result"])
        raise TimeoutError("native read deadline")

    try:
        identity = lookup_process_identity(child.pid)
        _ = request(
            "initialize",
            {
                "clientInfo": {
                    "name": "agent_collaboration",
                    "title": "Verification read-only correlation",
                    "version": "0.1.95",
                },
                "capabilities": {"experimentalApi": True},
            },
            1,
        )
        _ = native_stdin.write(b'{"method":"initialized"}\n')
        native_stdin.flush()
        response = request(
            "thread/read", {"threadId": expected.thread_id, "includeTurns": True}, 2
        )
        return correlate_message(
            response["thread"],
            {
                "thread_id": expected.thread_id,
                "turn_id": expected.turn_id,
                "push_id": expected.push_id,
            },
        )
    finally:
        selector.close()
        try:
            native_stdin.close()
        except BrokenPipeError:
            pass
        try:
            _ = child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            if identity is None or not same_identity(
                lookup_process_identity(child.pid), identity
            ):
                raise RuntimeError(
                    "native carrier identity unavailable; no signal"
                ) from None
            child.terminate()
            _ = child.wait(timeout=5)
        native_stdout.close()
        if child.stderr is not None:
            child.stderr.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--cli", type=Path, required=True)
    _ = parser.add_argument("--service-directory", type=Path, required=True)
    _ = parser.add_argument("--thread-id", required=True)
    _ = parser.add_argument("--turn-id", required=True)
    _ = parser.add_argument("--push-id", required=True)
    _ = parser.add_argument("--output", type=Path, required=True)
    arguments = CorrelationArguments.model_validate(vars(parser.parse_args()))
    result = read_correlation(arguments)
    with arguments.output.open("x") as output:
        json.dump(result, output, indent=2)
        _ = output.write("\n")
    arguments.output.chmod(0o600)
    print(json.dumps(result))
    raise SystemExit(0 if result["supported"] else 1)
