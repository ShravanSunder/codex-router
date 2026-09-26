"""Ordered stdio JSON-RPC fixture agent, launched only by Rust tests."""

import json
import sys


def fail(step_number, expected, actual):
    print(
        f"ACP fixture step {step_number} mismatch\n"
        f"expected: {json.dumps(expected, sort_keys=True)}\n"
        f"actual:   {json.dumps(actual, sort_keys=True)}",
        file=sys.stderr,
        flush=True,
    )
    sys.exit(1)


def contains_expected(actual, expected):
    if isinstance(expected, dict):
        return isinstance(actual, dict) and all(
            key in actual and contains_expected(actual[key], value)
            for key, value in expected.items()
        )
    if isinstance(expected, list):
        return isinstance(actual, list) and len(actual) == len(expected) and all(
            contains_expected(item, expected_item)
            for item, expected_item in zip(actual, expected)
        )
    return actual == expected


def read_message(step_number):
    line = sys.stdin.readline()
    if not line:
        fail(step_number, "JSON-RPC message", "EOF")
    try:
        return json.loads(line)
    except json.JSONDecodeError as error:
        fail(step_number, "valid JSON-RPC message", f"invalid JSON: {error}")


def send_message(message):
    print(json.dumps(message), flush=True)


steps = json.loads(sys.argv[1])
request_ids = {}
for step_number, step in enumerate(steps, start=1):
    action = step["action"]
    if action == "expect_request":
        actual = read_message(step_number)
        expected = {"jsonrpc": "2.0", "method": step["method"], "params": step["params"]}
        if not contains_expected(actual, expected) or "id" not in actual:
            fail(step_number, expected, actual)
        request_ids[step["requestName"]] = actual["id"]
    elif action == "expect_message":
        actual = read_message(step_number)
        if not contains_expected(actual, step["message"]):
            fail(step_number, step["message"], actual)
    elif action == "respond":
        request_name = step["requestName"]
        if request_name not in request_ids:
            fail(step_number, {"capturedRequest": request_name}, request_ids)
        send_message(
            {"jsonrpc": "2.0", "id": request_ids.pop(request_name), "result": step["result"]}
        )
    elif action == "send":
        send_message(step["message"])
    else:
        fail(step_number, "known fixture action", step)

# Keep the agent process alive until the client drops the connection. This
# lets a test observe the completed exchange without a synthetic provider loss.
sys.stdin.read()
