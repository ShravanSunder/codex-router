#!/usr/bin/env python3
"""One genuine Question then one allow-once private marker Approval; no replay."""

import argparse
import hashlib
import json
import time
from collections import abc
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from pydantic import BaseModel

from scripts.verification.interaction_readback import (
    callback_prompt,
    question_response_for,
    read_snapshot,
    require_callback_capture,
)
from scripts.verification.owned_runtime import save_record
from scripts.verification.provider_session_control import (
    JourneyPlan,
    approver_for,
    call_once,
    caller_for,
    close_session,
    create_session,
    initialize_approver,
    inspect_created_session,
    observe_session,
    operation_id,
    qualify_before_input,
    qualify_retained,
    rpc_call,
)
from scripts.verification.typed_observations import (
    InteractionListing,
    InteractionSnapshot,
    JsonObject,
    PublicInteraction,
    observed_object,
    read_observed_json,
)


class JourneyArguments(BaseModel):
    action: str
    plan: Path


def scoped_records(
    plan: JourneyPlan, kind: str, target: JsonObject
) -> list[PublicInteraction]:
    name = "question_list" if kind == "question" else "approval_list"
    arguments: JsonObject = {"pending": True}
    if kind == "approval":
        arguments["includeOptions"] = True
    listed = InteractionListing.model_validate(rpc_call(plan, name, arguments))
    records = listed.questions if kind == "question" else listed.approvals
    return [
        item
        for item in records
        if item.requester.model_dump(mode="json", exclude_unset=True) == target
    ]


def drive_callbacks(plan: JourneyPlan) -> None:
    qualify_before_input(plan)
    target = read_observed_json((plan.evidence / "target-binding.json").read_text())
    caller = caller_for(plan)
    actor: JsonObject = {"kind": "session", "session": approver_for(plan)}
    marker = plan.evidence.parent / "workspace" / "automated-approval-marker"
    if marker.exists():
        raise RuntimeError("marker already exists")
    command = f"printf '%s\\n' 'VERIFICATION_MARKER' > {marker}"
    if plan.tools is None:
        raise RuntimeError("actual provider tools unqualified")
    prompt = callback_prompt(plan.tools.model_dump(mode="json"), command)
    operation = operation_id()
    save_record(
        plan.evidence / "prompt-intent.json",
        {
            "operationId": operation,
            "target": target,
            "actor": actor,
            "command": command,
            "prompt": prompt,
            "testAutomation": True,
        },
    )
    request_ids: list[str] = []
    with ThreadPoolExecutor(max_workers=2) as executor:
        observer = executor.submit(observe_session, plan, 120)
        prompted = executor.submit(
            rpc_call,
            plan,
            "conversation_prompt",
            {
                "operationId": operation,
                "target": target,
                "requestedBy": caller,
                "approver": approver_for(plan),
                "message": {"kind": "agent", "sender": caller, "text": prompt},
                "timeoutSeconds": 120,
            },
        )
        deadline = time.monotonic() + 120
        for kind in ["question", "approval"]:
            while time.monotonic() < deadline:
                records = scoped_records(plan, kind, target)
                if records:
                    break
                unexpected = scoped_records(
                    plan, "approval" if kind == "question" else "question", target
                )
                if unexpected:
                    save_record(
                        plan.evidence / "unexpected-pending.json",
                        [
                            item.model_dump(mode="json", exclude_unset=True)
                            for item in unexpected
                        ],
                    )
                    raise RuntimeError(
                        "unexpected interaction ordering/tool; no decision"
                    )
                if prompted.done():
                    save_record(
                        plan.evidence / "early-prompt-result.json", prompted.result()
                    )
                    raise RuntimeError(f"real provider emitted no {kind}; no retry")
                time.sleep(0.2)
            else:
                raise TimeoutError(f"no real {kind} within original turn bound")
            if len(records) != 1 or records[0].approver != actor:
                raise RuntimeError("ambiguous request or wrong actual Session Approver")
            record = records[0]
            request_id = record.requestId
            request_ids.append(request_id)
            save_record(
                plan.evidence / f"{kind}-pending-public.json",
                record.model_dump(mode="json", exclude_unset=True),
            )
            pending = read_snapshot(
                plan.service_directory / "interaction.sqlite", [request_id]
            )
            save_record(plan.evidence / f"{kind}-pending-sqlite.json", pending)
            if kind == "question":
                _ = call_once(
                    plan,
                    "question_answer",
                    {
                        "requestId": request_id,
                        "actor": actor,
                        "response": question_response_for(
                            record.model_dump(mode="json", exclude_unset=True)
                        ),
                    },
                )
            else:
                request = (
                    InteractionSnapshot.model_validate(pending)
                    .records[0]
                    .record.request
                )
                if request is None or request.subject is None:
                    raise RuntimeError(
                        "approval request/subject missing; no allow decision"
                    )
                description = request.description or ""
                if (
                    f"command: {command}" not in description.splitlines()
                    or request.subject.toolCall.kind != "Execute"
                ):
                    raise RuntimeError(
                        "unexpected approval tool/action; no allow decision"
                    )
                options = [
                    option
                    for option in record.options
                    if option.effect == "allow"
                    and option.scope == "once"
                    and not option.persistentTarget
                ]
                if len(options) != 1:
                    raise RuntimeError("actual allow-once option unavailable")
                _ = call_once(
                    plan,
                    "approval_decide",
                    {
                        "requestId": request_id,
                        "actor": actor,
                        "optionId": options[0].optionId,
                        "note": "Labelled test automation: exact private owned marker only",
                    },
                )
            print(
                json.dumps(
                    {
                        "automatedDecision": kind,
                        "requestId": request_id,
                        "actor": "actual Session",
                        "humanImpersonation": False,
                    }
                ),
                flush=True,
            )
        prompt_result = prompted.result(timeout=max(1, deadline - time.monotonic()))
        save_record(plan.evidence / "prompt-result.json", prompt_result)
        operation_result = rpc_call(
            plan, "conversation_operation_show", {"operationId": operation}
        )
        save_record(plan.evidence / "operation-terminal.json", operation_result)
        _ = observer.result()
    verify_retained(plan)


def verify_retained(plan: JourneyPlan) -> None:
    operation_result = read_observed_json(
        (plan.evidence / "operation-terminal.json").read_text()
    )
    capture = read_observed_json((plan.evidence / "callback-events.json").read_text())
    request_ids: list[str] = []
    for name in ["question_answer", "approval_decide"]:
        arguments = observed_object(
            read_observed_json((plan.evidence / f"{name}-intent.json").read_text())[
                "arguments"
            ]
        )
        request_id = arguments["requestId"]
        if not isinstance(request_id, str):
            raise TypeError("retained request id must be a string")
        request_ids.append(request_id)
    marker = plan.evidence.parent / "workspace" / "automated-approval-marker"
    if (
        operation_result["stage"] != "terminal"
        or operation_result["effect"] != "applied"
        or operation_result["reconciliation"] != "confirmed"
    ):
        raise RuntimeError("provider operation not settled; do not replay")
    require_callback_capture(capture, operation_result, request_ids)
    if not marker.is_file() or marker.read_bytes() != b"VERIFICATION_MARKER\n":
        raise RuntimeError("provider tool did not create exact marker")
    terminal = read_snapshot(plan.service_directory / "interaction.sqlite", request_ids)
    answer_arguments = observed_object(
        read_observed_json((plan.evidence / "question_answer-intent.json").read_text())[
            "arguments"
        ]
    )
    answer = observed_object(answer_arguments["response"])["content"]
    choice_arguments = observed_object(
        read_observed_json((plan.evidence / "approval_decide-intent.json").read_text())[
            "arguments"
        ]
    )
    choice = choice_arguments["optionId"]
    if not isinstance(choice, str):
        raise TypeError("retained option id must be a string")
    terminal_view = InteractionSnapshot.model_validate(terminal)
    if terminal_view.records[0].record.state != {
        "kind": "answered",
        "content": answer,
    } or terminal_view.records[1].record.state != {
        "kind": "decided",
        "option_id": choice,
    }:
        raise RuntimeError(
            "stored terminal answer/choice differs from actual decisions"
        )
    save_record(plan.evidence / "terminal-sqlite.json", terminal)
    save_record(plan.evidence / "request-ids.json", request_ids)
    print(
        json.dumps(
            {
                "callbackJourneyObserved": True,
                "requestIds": request_ids,
                "markerSHA256": hashlib.sha256(marker.read_bytes()).hexdigest(),
                "owningReopenStillRequired": True,
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument(
        "action",
        choices=[
            "create",
            "inspect",
            "observe",
            "initialize-approver",
            "close",
            "drive",
            "verify-retained",
            "qualify-retained",
        ],
    )
    _ = parser.add_argument("plan", type=Path)
    arguments = JourneyArguments.model_validate(vars(parser.parse_args()))
    plan = JourneyPlan.model_validate_json(arguments.plan.read_text())

    def qualify_retained_action(plan: JourneyPlan) -> None:
        print(json.dumps(qualify_retained(plan)))

    actions: dict[str, abc.Callable[[JourneyPlan], JsonObject | None]] = {
        "create": create_session,
        "inspect": inspect_created_session,
        "observe": observe_session,
        "initialize-approver": initialize_approver,
        "close": close_session,
        "drive": drive_callbacks,
        "verify-retained": verify_retained,
        "qualify-retained": qualify_retained_action,
    }
    _ = actions[arguments.action](plan)
