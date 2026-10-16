"""Provider Session plans, qualified creation and bounded control for verification."""

import datetime as dt
import hashlib
import json
import secrets
import subprocess
import time
import typing as t
import urllib.request
import uuid
from http.client import HTTPResponse
from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field

from scripts.verification.interaction_readback import (
    decode_rpc_body,
    require_advertised_settings,
)
from scripts.verification.owned_runtime import save_record
from scripts.verification.typed_observations import (
    EndpointInventory,
    HostManifest,
    JsonObject,
    OperationView,
    ProviderRecord,
    PublicObservation,
    RpcEnvelope,
    SessionView,
    observed_object,
    read_observed_json,
)


def provider_record(value: JsonObject) -> ProviderRecord:
    return ProviderRecord.model_validate(value.get("record", value))


class EndpointReference(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="forbid")
    serviceId: str = Field(min_length=36, max_length=36)
    endpointId: str = Field(min_length=1)


class SessionReference(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="forbid")
    endpoint: EndpointReference
    sessionId: str = Field(min_length=1)


class ToolRoute(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="forbid")
    question_tool: str = Field(min_length=1)
    execution_tool: str = Field(min_length=1)
    question_method: t.Literal["elicitation/create", "cursor/ask_question"]
    evidence_path: Path
    evidence_sha256: str = Field(pattern="^[0-9a-f]{64}$")


class JourneyPlan(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="forbid")
    cli: Path
    evidence: Path
    service_directory: Path
    caller_session_id: str
    endpoint_id: t.Literal["claude-local", "cursor-local"]
    settings: dict[str, str]
    tools: ToolRoute | None = None
    approver: SessionReference | None = None


def operation_id() -> str:
    milliseconds = time.time_ns() // 1_000_000
    value = (
        (milliseconds << 80)
        | (7 << 76)
        | (secrets.randbits(12) << 64)
        | (2 << 62)
        | secrets.randbits(62)
    )
    return str(uuid.UUID(int=value))


def manifest_for(plan: JourneyPlan) -> HostManifest:
    return HostManifest.model_validate_json(
        (plan.service_directory / "service.json").read_text()
    )


def rpc_call(plan: JourneyPlan, name: str, arguments: JsonObject) -> JsonObject:
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }
    request = urllib.request.Request(
        manifest_for(plan).mcp.url,
        data=json.dumps(payload).encode(),
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "MCP-Protocol-Version": "2025-11-25",
        },
    )
    opened = t.cast(object, urllib.request.urlopen(request, timeout=140))
    if not isinstance(opened, HTTPResponse):
        raise TypeError("MCP response is not HTTP; no replay")
    with opened as response:
        result = RpcEnvelope.model_validate(decode_rpc_body(response.read().decode()))
    if result.error is not None:
        raise RuntimeError(f"RPC {name} error; no replay: {result.error.code}")
    value = result.result
    if value is None:
        raise RuntimeError(f"RPC {name} has no result; no replay")
    structured = value.structuredContent
    if structured is None:
        structured = next(
            observed_object(read_observed_json(item.text))
            for item in value.content
            if item.type == "text" and item.text is not None
        )
    if value.isError or structured.get("mcpResult") == "error":
        raise RuntimeError(f"Tool {name} rejected: {structured.get('kind')}; no replay")
    return structured


def call_once(plan: JourneyPlan, name: str, arguments: JsonObject) -> JsonObject:
    save_record(
        plan.evidence / f"{name}-intent.json",
        {
            "at": dt.datetime.now(dt.UTC).isoformat(),
            "arguments": arguments,
            "testAutomation": True,
        },
    )
    try:
        result = rpc_call(plan, name, arguments)
    except Exception as error:
        save_record(
            plan.evidence / f"{name}-unknown.json",
            {"error": type(error).__name__, "doNotReplay": True},
        )
        raise
    save_record(plan.evidence / f"{name}-result.json", result)
    return result


def caller_for(plan: JourneyPlan) -> JsonObject:
    return {
        "endpoint": {
            "serviceId": manifest_for(plan).serviceId,
            "endpointId": "codex-local",
        },
        "sessionId": plan.caller_session_id,
    }


def approver_for(plan: JourneyPlan) -> JsonObject:
    return (
        observed_object(plan.approver.model_dump())
        if plan.approver is not None
        else caller_for(plan)
    )


def create_session(plan: JourneyPlan) -> None:
    plan.evidence.mkdir(mode=0o700, parents=True, exist_ok=True)
    if (plan.evidence / "create-intent.json").exists():
        raise RuntimeError(
            "create already attempted; inspect retained operation, never replay"
        )
    manifest = manifest_for(plan)
    caller = caller_for(plan)
    operation = operation_id()
    inventory = EndpointInventory.model_validate(rpc_call(plan, "endpoints_list", {}))
    endpoint = next(
        (
            item
            for item in inventory.endpoints
            if item.endpoint.model_dump()
            == {"serviceId": manifest.serviceId, "endpointId": plan.endpoint_id}
        ),
        None,
    )
    if endpoint is None or endpoint.availability.state != "available":
        raise RuntimeError("selected provider endpoint unavailable; no create")
    channel = next(
        (item for item in endpoint.channels if item.kind == "externalProvider"),
        None,
    )
    if channel is None or channel.bindingGeneration is None:
        raise RuntimeError("selected endpoint is not an admitted external provider")
    save_record(
        plan.evidence / "endpoint-preflight.json",
        endpoint.model_dump(mode="json", exclude_unset=True),
    )
    generation = {
        "serviceEpoch": manifest.serviceEpoch,
        "generation": channel.bindingGeneration,
    }
    argv = [
        str(plan.cli),
        "conversation",
        "create",
        "--endpoint",
        plan.endpoint_id,
        "--cwd",
        str(plan.evidence.parent / "workspace"),
        "--access",
        "write-restricted",
        "--from",
        json.dumps(caller),
        "--approver",
        json.dumps(approver_for(plan)),
        "--generation",
        json.dumps(generation),
        "--operation-id",
        operation,
        "--timeout-seconds",
        "60",
        "--service-directory",
        str(plan.service_directory),
        "--json",
    ]
    for setting_name, setting_value in plan.settings.items():
        if setting_name not in {"mode", "model", "effort"}:
            raise RuntimeError("setting has no supported CLI selector; no create")
        argv.extend([f"--{setting_name}", setting_value])
    save_record(
        plan.evidence / "create-intent.json",
        {
            "operationId": operation,
            "argv": argv,
            "creator": caller,
            "approver": approver_for(plan),
            "testAutomation": True,
        },
    )
    result = subprocess.run(
        argv, capture_output=True, text=True, timeout=75, check=False
    )
    if result.returncode != 0:
        save_record(
            plan.evidence / "create-failure.json",
            {
                "exit": result.returncode,
                "stdout": [
                    read_observed_json(line)
                    for line in result.stdout.splitlines()
                    if line.strip()
                ],
                "stderrSHA256": hashlib.sha256(result.stderr.encode()).hexdigest(),
                "doNotReplay": True,
            },
        )
        raise RuntimeError("create failed; reconcile operation, no second create")
    created = [
        read_observed_json(line) for line in result.stdout.splitlines() if line.strip()
    ][-1]
    save_record(plan.evidence / "create-result.json", created)
    inspect_created_session(plan)


def inspect_created_session(plan: JourneyPlan) -> None:
    intent = read_observed_json((plan.evidence / "create-intent.json").read_text())
    readback = plan.evidence / "create-operation-readback.json"
    if readback.exists():
        operation = read_observed_json(readback.read_text())
    else:
        operation = rpc_call(
            plan, "conversation_operation_show", {"operationId": intent["operationId"]}
        )
        save_record(readback, operation)
    snapshot = OperationView.model_validate(operation)
    if (
        snapshot.stage != "terminal"
        or snapshot.effect != "applied"
        or snapshot.reconciliation != "confirmed"
    ):
        raise RuntimeError("create operation not confirmed; no replay")
    target = observed_object(
        snapshot.target.model_dump(mode="json", exclude_unset=True)
    )
    save_record(plan.evidence / "target-binding.json", target)
    inspected = rpc_call(plan, "provider_session_inspect", {"target": target})
    save_record(plan.evidence / "session-inspect.json", inspected)
    record = provider_record(inspected)
    if not record.capabilities.elicitation:
        raise RuntimeError("unsupported real provider Question; no model prompt")
    if plan.settings:
        _ = require_advertised_settings(
            record.model_dump(mode="json", exclude_unset=True), plan.settings
        )
    print(
        json.dumps(
            {
                "created": target,
                "routerElicitationRoute": record.capabilities.elicitation,
                "selectedSettings": plan.settings,
                "catalogOnly": not bool(plan.settings),
                "authStatus": record.capabilities.authStatus.model_dump(
                    mode="json", exclude_unset=True
                )
                if record.capabilities.authStatus
                else None,
                "providerQuestionToolSupport": "requires qualified tool provenance and genuine emission",
            }
        ),
        flush=True,
    )


def close_session(plan: JourneyPlan) -> None:
    target = read_observed_json((plan.evidence / "target-binding.json").read_text())
    caller = caller_for(plan)
    operation = operation_id()
    _ = call_once(
        plan,
        "conversation_close",
        {
            "operationId": operation,
            "target": target,
            "requestedBy": caller,
            "approver": approver_for(plan),
        },
    )
    settled = rpc_call(
        plan,
        "conversation_operation_wait",
        {"operationId": operation, "timeoutSeconds": 30},
    )
    save_record(plan.evidence / "close-settlement.json", settled)
    print(json.dumps(settled), flush=True)


def initialize_approver(plan: JourneyPlan) -> None:
    qualify_before_input(plan, require_tools=False)
    target = read_observed_json((plan.evidence / "target-binding.json").read_text())
    caller = caller_for(plan)
    operation = operation_id()
    text = "You are a dedicated test-automation notification sink on this debug Host. For every future Router Question or Approval notice, call no tools, make no decision, change nothing, and reply exactly TEST_NOTICE_ACK. An external test driver will answer the actual requests explicitly under your real Session identity. For this initialization only, call no tools and reply exactly TEST_APPROVER_READY, then end the turn."
    result = call_once(
        plan,
        "conversation_prompt",
        {
            "operationId": operation,
            "target": target,
            "requestedBy": caller,
            "approver": approver_for(plan),
            "message": {"kind": "agent", "sender": caller, "text": text},
            "timeoutSeconds": 60,
        },
    )
    terminal = rpc_call(plan, "conversation_operation_show", {"operationId": operation})
    save_record(plan.evidence / "initialization-operation.json", terminal)
    terminal_view = OperationView.model_validate(terminal)
    output_text = observed_object(
        observed_object(observed_object(result["settlement"])["detail"])["output"]
    )["text"]
    if (
        terminal_view.stage != "terminal"
        or terminal_view.effect != "applied"
        or terminal_view.reconciliation != "confirmed"
        or not isinstance(output_text, str)
        or output_text.strip() != "TEST_APPROVER_READY"
    ):
        raise RuntimeError("approver initialization not confirmed; no requester prompt")
    capture = observe_session(plan)
    if any(
        wrapped.event["kind"] in {"itemStarted", "itemUpdated", "interactionRequested"}
        for wrapped in PublicObservation.model_validate(capture).events
    ):
        raise RuntimeError(
            "approver initialization used a tool or requested an interaction"
        )
    inspected = rpc_call(plan, "provider_session_inspect", {"target": target})
    save_record(plan.evidence / "approver-ready-inspect.json", inspected)
    if provider_record(inspected).state != "idle":
        raise RuntimeError("approver not idle after initialization")
    print(
        json.dumps({"actualApprover": target, "noToolsInitializationConfirmed": True}),
        flush=True,
    )


def observe_session(plan: JourneyPlan, timeout_seconds: int = 1) -> JsonObject:
    target = read_observed_json((plan.evidence / "target-binding.json").read_text())
    observation = rpc_call(
        plan,
        "events_observe",
        {
            "target": target,
            "timeoutSeconds": timeout_seconds,
            "maxEvents": 1024,
            "maxBytes": 1048576,
        },
    )
    events: list[JsonObject] = []
    for wrapped_view in PublicObservation.model_validate(observation).events:
        wrapped = observed_object(
            wrapped_view.model_dump(mode="json", exclude_unset=True)
        )
        event = wrapped_view.event
        if event["kind"] in {
            "resyncRequired",
            "eventTooLarge",
        }:
            raise RuntimeError("observer resync or oversized event gap")
        if event["kind"] in {
            "interactionRequested",
            "interactionResolved",
            "turnStarted",
            "turnEnded",
            "inputAccepted",
        }:
            events.append(wrapped)
        elif (
            event["kind"] in {"itemStarted", "itemUpdated"}
            and observed_object(observed_object(event["item"])["kind"])["kind"]
            == "toolCall"
        ):
            filtered = read_observed_json(json.dumps(wrapped))
            filtered_event = observed_object(filtered["event"])
            filtered_item = observed_object(filtered_event["item"])
            _ = filtered_item.pop("text", None)
            filtered_event["item"] = filtered_item
            filtered["event"] = filtered_event
            events.append(filtered)
    retained = observed_object(
        {
            "target": target,
            "events": events,
            "observation": {
                key: value for key, value in observation.items() if key != "events"
            },
        }
    )
    save_record(plan.evidence / "callback-events.json", retained)
    return retained


def qualify_retained(plan: JourneyPlan) -> JsonObject:
    inspected = read_observed_json((plan.evidence / "session-inspect.json").read_text())
    record = provider_record(inspected)
    if record.target.endpoint.endpointId != plan.endpoint_id:
        raise RuntimeError("retained inspection belongs to another provider endpoint")
    catalog = require_advertised_settings(
        record.model_dump(mode="json", exclude_unset=True), plan.settings
    )
    if plan.tools is None:
        raise RuntimeError(
            "actual provider tool names/route are unqualified; no callback input"
        )
    expected_method = (
        "elicitation/create"
        if plan.endpoint_id == "claude-local"
        else "cursor/ask_question"
    )
    if plan.tools.question_method != expected_method:
        raise RuntimeError("provider Question route does not match selected endpoint")
    if (
        hashlib.sha256(plan.tools.evidence_path.read_bytes()).hexdigest()
        != plan.tools.evidence_sha256
    ):
        raise RuntimeError("tool provenance artifact hash mismatch")
    return {
        "settingsCatalog": catalog,
        "tools": plan.tools.model_dump(mode="json"),
        "toolEvidenceIsProvenanceNotRuntimeProof": True,
    }


def qualify_before_input(plan: JourneyPlan, require_tools: bool = True) -> None:
    target = read_observed_json((plan.evidence / "target-binding.json").read_text())
    target_view = SessionView.model_validate(target)
    if target_view.endpoint.endpointId != plan.endpoint_id:
        raise RuntimeError("actual target does not match selected provider endpoint")
    inspected = rpc_call(plan, "provider_session_inspect", {"target": target})
    record = provider_record(inspected)
    _ = require_advertised_settings(
        record.model_dump(mode="json", exclude_unset=True), plan.settings
    )
    if record.state != "idle" or not record.capabilities.elicitation:
        raise RuntimeError("provider is not idle or Router Question route unavailable")
    if (
        plan.endpoint_id == "cursor-local"
        and record.capabilities.authStatus is not None
        and record.capabilities.authStatus.kind == "loggedOut"
    ):
        raise RuntimeError("actual Cursor provider reports loggedOut; no model input")
    if require_tools:
        if plan.tools is None:
            raise RuntimeError(
                "actual provider tools are not qualified; no callback input"
            )
        expected_method = (
            "elicitation/create"
            if plan.endpoint_id == "claude-local"
            else "cursor/ask_question"
        )
        if (
            plan.tools.question_method != expected_method
            or hashlib.sha256(plan.tools.evidence_path.read_bytes()).hexdigest()
            != plan.tools.evidence_sha256
        ):
            raise RuntimeError("tool route or provenance mismatch; no callback input")
        if (
            plan.approver is None
            or plan.approver.model_dump() == target
            or plan.approver.endpoint.serviceId != target_view.endpoint.serviceId
            or plan.approver.endpoint.endpointId != plan.endpoint_id
        ):
            raise RuntimeError("distinct actual same-Host provider Approver required")
        approver = rpc_call(
            plan, "provider_session_inspect", {"target": plan.approver.model_dump()}
        )
        if provider_record(approver).state != "idle":
            raise RuntimeError("actual Approver is not idle")
    save_record(
        plan.evidence / "pre-input-qualification.json",
        {
            "target": target,
            "actualInspection": inspected,
            "selectedSettings": plan.settings,
            "tools": plan.tools.model_dump(mode="json") if plan.tools else None,
            "kind": "callback" if require_tools else "noToolsApproverInitialization",
        },
    )
