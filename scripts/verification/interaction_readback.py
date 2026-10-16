#!/usr/bin/env python3
"""Read only scoped typed interaction rows and the revision in one transaction."""

import argparse
import json
import sqlite3
import typing as t
from pathlib import Path

from pydantic import BaseModel, ConfigDict, JsonValue, TypeAdapter

from scripts.verification.typed_observations import (
    CallbackCapture,
    CatalogRecord,
    InteractionSnapshot,
    JsonObject,
    OperationView,
    QuestionView,
    StoredInteraction,
    observed_object,
    read_observed_json,
)


class ReadbackArguments(BaseModel):
    database: Path
    request_id: list[str]
    before: Path | None
    output: Path


def require_advertised_settings(record: object, expected: dict[str, str]) -> JsonObject:
    if not expected:
        raise ValueError(
            "settings not qualified; inspect the actual catalog before model input"
        )
    catalog = CatalogRecord.model_validate(record).settingsCatalog
    for name, value in expected.items():
        if name not in {"mode", "model", "effort"}:
            raise ValueError("setting has no supported CLI selector")
        candidates = [
            option
            for option in catalog.configOptions
            if option.category == name
            and option.id != "thinking"
            and option.currentValue.kind == "select"
        ]
        explicit = [option for option in candidates if option.id == name]
        candidates = explicit or candidates
        if name == "mode" and not candidates:
            current = catalog.currentMode
            choices = catalog.modes
        else:
            if len(candidates) != 1:
                raise ValueError("advertised setting is missing or ambiguous")
            current = candidates[0].currentValue.value
            choices = candidates[0].choices
        if current != value or not any(choice.value == value for choice in choices):
            raise ValueError(
                "actual provider setting differs from selected advertised value"
            )
    return observed_object(observed_object(record)["settingsCatalog"])


def callback_prompt(tools: dict[str, str], command: str) -> str:
    return (
        "This is labelled test automation for typed Question and Approval callbacks. "
        f"First call the actual {tools['question_tool']} tool on its supported {tools['question_method']} route with exactly one single-select question: "
        "Which verification marker should this test write? Offer exactly Blue and Green. "
        "Wait for the actual tool answer; do not merely ask in prose. "
        f"After receiving Blue, call {tools['execution_tool']} exactly once with this exact command: {command}\n"
        "The actual Session approver will select allow once for that private marker action. "
        "Do not call any other tools, delegate, retry, change permissions, or use another path. "
        "After the tool result, reply exactly QUESTION_BLUE_APPROVAL_CALLBACK_COMPLETE and end the turn."
    )


def decode_rpc_body(body: str) -> JsonObject:
    if body.lstrip().startswith("{"):
        result = read_observed_json(body)
    else:
        frames = [
            read_observed_json(line.removeprefix("data: "))
            for line in body.splitlines()
            if line.startswith("data: ")
        ]
        responses = [frame for frame in frames if frame.get("id") == 1]
        if len(responses) != 1:
            raise ValueError("missing or ambiguous correlated RPC response")
        result = responses[0]
    if result.get("id") != 1:
        raise ValueError("wrong RPC response identity")
    return observed_object(result)


def question_response_for(record: object) -> JsonObject:
    view = QuestionView.model_validate(record)
    choices = [field for field in view.fields if field.kind == "singleChoice"]
    if len(choices) != 1:
        raise ValueError("expected one real single-choice field")
    choice = choices[0]
    extras = [field for field in view.fields if field != choice]
    if (
        any(
            field.kind != "text"
            or field.required
            or field.fieldId != choice.fieldId + "_custom"
            for field in extras
        )
        or len(extras) > 1
    ):
        raise ValueError("unexpected additional Question field")
    options = choice.options
    blue = [option for option in options if option.label == "Blue"]
    if (
        len(blue) != 1
        or len(options) != 2
        or {option.label for option in options} != {"Blue", "Green"}
    ):
        raise ValueError("unexpected Question options")
    return {
        "action": "answered",
        "content": {choice.fieldId: {"selectedOptionIds": [blue[0].optionId]}},
    }


def require_callback_capture(
    capture: object, operation: object, request_ids: list[str]
) -> None:
    capture = CallbackCapture.model_validate(capture)
    operation = OperationView.model_validate(operation)
    observation = capture.observation
    if (
        capture.target != operation.target
        or observation.target != operation.target
        or observation.generation != operation.binding.binding.generation
    ):
        raise ValueError("observer target or provider binding generation mismatch")
    if not observation.attached or observation.endReason != "deadlineReached":
        raise ValueError("observer did not complete its bounded attachment")
    events = capture.events
    sequences = [wrapped.sequence for wrapped in events]
    if sequences != sorted(set(sequences)):
        raise ValueError("duplicate or out-of-order observed event sequences")
    started = [
        wrapped.event for wrapped in events if wrapped.event.kind == "turnStarted"
    ]
    ended = [wrapped.event for wrapped in events if wrapped.event.kind == "turnEnded"]
    if (
        len(started) != 1
        or len(ended) != 1
        or not started[0].inputId
        or not started[0].turnId
        or (operation.inputId is not None and started[0].inputId != operation.inputId)
        or ended[0].turnId != started[0].turnId
        or ended[0].outcome.kind != "ended"
        or ended[0].outcome.stop_reason != "endTurn"
    ):
        raise ValueError("exact input/turn terminal correlation missing")
    accepted = [
        wrapped.event for wrapped in events if wrapped.event.kind == "inputAccepted"
    ]
    if len(accepted) > 1 or any(
        event.inputId != started[0].inputId or event.turnId != started[0].turnId
        for event in accepted
    ):
        raise ValueError("unexpected additional requester input")
    requested = [
        (wrapped.sequence, wrapped.event.interaction)
        for wrapped in events
        if wrapped.event.kind == "interactionRequested"
    ]
    if [interaction.kind for _, interaction in requested] != [
        "question",
        "approval",
    ] or [interaction.request.requestId for _, interaction in requested] != request_ids:
        raise ValueError("actual Question-first request identities/order missing")
    resolved = {
        wrapped.event.requestId: wrapped.sequence
        for wrapped in events
        if wrapped.event.kind == "interactionResolved"
    }
    if (
        set(resolved) != set(request_ids)
        or not requested[0][0]
        < resolved[request_ids[0]]
        < requested[1][0]
        < resolved[request_ids[1]]
    ):
        raise ValueError("actual interaction resolution order missing")
    approval = requested[1][1]
    if approval.request.subject is None:
        raise ValueError("approval tool subject missing")
    approval_tool = approval.request.subject.toolCall.toolCallId
    tool_events = [
        (wrapped.sequence, wrapped.event.item)
        for wrapped in events
        if wrapped.event.kind == "itemStarted" or wrapped.event.kind == "itemUpdated"
    ]
    tools = {item.itemId for _, item in tool_events}
    completed = {
        item.itemId: sequence
        for sequence, item in tool_events
        if item.kind.status == "completed"
    }
    if len(tools) != 2 or set(completed) != tools or approval_tool not in completed:
        raise ValueError("actual tool completion missing or unexpected tool")
    question_tool = next(iter(tools - {approval_tool}))
    if (
        not completed[question_tool] < requested[1][0]
        or not resolved[request_ids[1]] < completed[approval_tool]
    ):
        raise ValueError("actual callback/tool completion order missing")


def read_snapshot(database: Path, request_ids: list[str]) -> JsonObject:
    connection = sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True)
    try:
        _ = connection.execute("BEGIN")
        revision_rows = TypeAdapter(
            list[tuple[int, int]], config=ConfigDict(strict=True)
        ).validate_python(
            t.cast(
                object,
                connection.execute(
                    "SELECT metadata_id, revision FROM interaction_history_revision"
                ).fetchall(),
            )
        )
        if (
            len(revision_rows) != 1
            or revision_rows[0][0] != 1
            or type(revision_rows[0][1]) is not int
            or revision_rows[0][1] < 0
        ):
            raise ValueError("invalid authoritative revision")
        records: list[JsonValue] = []
        for request_id in request_ids:
            raw_row = t.cast(
                object,
                connection.execute(
                    "SELECT request_id,record_json,created_at FROM typed_interaction_history WHERE request_id=?",
                    (request_id,),
                ).fetchone(),
            )
            if raw_row is None:
                raise ValueError("scoped interaction missing")
            row = TypeAdapter(
                tuple[str, str, str], config=ConfigDict(strict=True)
            ).validate_python(raw_row)
            record = read_observed_json(row[1])
            view = StoredInteraction.model_validate(record)
            if view.request is None or view.request.requestId != request_id:
                raise ValueError("request identity mismatch")
            records.append({"requestId": row[0], "record": record, "createdAt": row[2]})
        return {
            "database": str(database.resolve()),
            "revision": revision_rows[0][1],
            "records": records,
        }
    finally:
        connection.close()


def compare_reopen(before: object, after: object) -> JsonObject:
    view = InteractionSnapshot.model_validate(before)
    if not view.records:
        raise ValueError("empty evidence cannot prove reopen")
    terminal = {"question": "answered", "approval": "decided"}
    for item in view.records:
        record = item.record
        if record.state["kind"] != terminal[record.kind]:
            raise ValueError(
                "pending or cancelled record cannot prove callback settlement"
            )
    if before != after:
        raise ValueError(
            "terminal content, creation instant, root or revision changed on owning reopen"
        )
    return {
        "supported": True,
        "terminalRecordCount": len(view.records),
        "revision": view.revision,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--database", type=Path, required=True)
    _ = parser.add_argument("--request-id", action="append", required=True)
    _ = parser.add_argument("--before", type=Path)
    _ = parser.add_argument("--output", type=Path, required=True)
    arguments = ReadbackArguments.model_validate(vars(parser.parse_args()))
    snapshot = read_snapshot(arguments.database, arguments.request_id)
    if arguments.before:
        print(
            json.dumps(
                compare_reopen(
                    read_observed_json(arguments.before.read_text()), snapshot
                )
            )
        )
    with arguments.output.open("x") as output:
        json.dump(snapshot, output, indent=2)
        _ = output.write("\n")
    arguments.output.chmod(0o600)
