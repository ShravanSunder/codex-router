"""Independent rejection cases for the verification apparatus, never product proof."""

import copy
import json
import sqlite3
import tempfile
import unittest
from contextlib import closing
from pathlib import Path

from scripts.verification.interaction_readback import (
    callback_prompt,
    compare_reopen,
    decode_rpc_body,
    question_response_for,
    read_snapshot,
    require_advertised_settings,
    require_callback_capture,
)
from scripts.verification.native_correlation import (
    correlate_message,
)
from scripts.verification.process_ownership import (
    owned_closure,
    remaining_owned,
    signal_each_owned,
)
from scripts.verification.typed_observations import (
    CallbackCapture,
    GenerationView,
    IdentityRecord,
    InteractionSnapshot,
    NativeCorrelation,
    NativeExpectation,
    NativeThread,
    OperationView,
    QuestionView,
    SettingsCatalog,
    observed_object,
)


class PortableProviderQualificationTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.catalog: SettingsCatalog = SettingsCatalog.model_validate(
            {
                "currentMode": "manual-mode",
                "modes": [{"value": "manual-mode", "label": "Manual"}],
                "configOptions": [
                    {
                        "id": "model",
                        "category": "model",
                        "currentValue": {"kind": "select", "value": "provider-model"},
                        "choices": [
                            {
                                "value": "provider-model",
                                "label": "Actual provider model",
                            }
                        ],
                    },
                    {
                        "id": "effort",
                        "category": "effort",
                        "currentValue": {"kind": "select", "value": "low"},
                        "choices": [{"value": "low", "label": "Low"}],
                    },
                    {
                        "id": "thinking",
                        "category": None,
                        "currentValue": {"kind": "boolean", "value": True},
                        "choices": [],
                    },
                ],
            }
        )

    def test_actual_model_effort_and_companion_are_preserved(self) -> None:
        result = require_advertised_settings(
            {
                "settingsCatalog": self.catalog.model_dump(
                    mode="json", exclude_unset=True
                )
            },
            {"mode": "manual-mode", "model": "provider-model", "effort": "low"},
        )
        self.assertEqual(
            result, self.catalog.model_dump(mode="json", exclude_unset=True)
        )
        self.assertTrue(
            SettingsCatalog.model_validate(result).configOptions[2].currentValue.value
        )

    def test_invented_model_sku_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "differs from selected advertised"):
            _ = require_advertised_settings(
                {
                    "settingsCatalog": self.catalog.model_dump(
                        mode="json", exclude_unset=True
                    )
                },
                {"model": "provider-model-low"},
            )

    def test_unqualified_catalog_only_plan_cannot_submit_model_input(self) -> None:
        with self.assertRaisesRegex(ValueError, "settings not qualified"):
            _ = require_advertised_settings(
                {
                    "settingsCatalog": self.catalog.model_dump(
                        mode="json", exclude_unset=True
                    )
                },
                {},
            )

    def test_prompt_uses_explicit_qualified_tools_without_claude_defaults(self) -> None:
        prompt = callback_prompt(
            {
                "question_tool": "qualified_question_tool",
                "execution_tool": "qualified_execute_tool",
                "question_method": "cursor/ask_question",
            },
            "exact-private-marker-command",
        )
        self.assertIn("qualified_question_tool", prompt)
        self.assertIn("qualified_execute_tool", prompt)
        self.assertIn("cursor/ask_question", prompt)
        self.assertIn("exact-private-marker-command", prompt)
        self.assertNotIn("AskUserQuestion", prompt)
        self.assertNotIn("Bash", prompt)


class ProcessOwnershipTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.root: IdentityRecord = {
            "pid": 10,
            "parent": 1,
            "uid": 501,
            "group": 10,
            "birth": "root-birth",
            "executable": "/owned/router",
        }
        self.child: IdentityRecord = {
            "pid": 11,
            "parent": 10,
            "uid": 501,
            "group": 10,
            "birth": "child-birth",
            "executable": "/owned/provider",
        }

    def test_reparented_child_remains_owned(self) -> None:
        observed = {11: {**self.child, "parent": 1}}
        self.assertEqual(
            set(remaining_owned({10: self.root, 11: self.child}, observed)), {11}
        )

    def test_reused_root_cannot_claim_new_descendants(self) -> None:
        observed = {
            10: {**self.root, "birth": "new-birth"},
            12: {**self.child, "pid": 12},
        }
        self.assertEqual(owned_closure(10, {10: self.root}, observed), set())

    def test_one_disappeared_child_does_not_skip_other_cleanup(self) -> None:
        known = {10: self.root, 11: self.child}
        signalled: list[int] = []

        def send(pid: int) -> None:
            if pid == 10:
                raise ProcessLookupError()
            signalled.append(pid)

        result = signal_each_owned(known, known.get, send)
        self.assertEqual(signalled, [11])
        self.assertEqual(result.model_dump(), {"sent": [11], "errors": []})


class NativeCorrelationTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.expected: NativeExpectation = NativeExpectation.model_validate(
            {
                "thread_id": "thread",
                "turn_id": "accepted-turn",
                "push_id": "push",
            }
        )
        self.thread: NativeThread = NativeThread.model_validate(
            {
                "id": "thread",
                "turns": [
                    {
                        "id": "accepted-turn",
                        "items": [
                            {
                                "id": "native-item",
                                "clientId": "push",
                                "type": "userMessage",
                                "content": [{"type": "text", "text": "synthetic"}],
                            }
                        ],
                    }
                ],
            }
        )

    def test_client_correlation_accepts_distinct_native_item_identity(self) -> None:
        result = correlate_message(self.thread, self.expected)
        self.assertTrue(result["supported"])
        self.assertEqual(
            NativeCorrelation.model_validate(result).matches[0].nativeId, "native-item"
        )

    def test_native_item_id_cannot_substitute_for_client_id(self) -> None:
        self.thread.turns[0].items[0].id = "push"
        self.thread.turns[0].items[0].clientId = "other"
        self.assertFalse(correlate_message(self.thread, self.expected)["supported"])

    def test_wrong_accepted_turn_is_rejected(self) -> None:
        self.thread.turns[0].id = "other-turn"
        self.assertFalse(correlate_message(self.thread, self.expected)["supported"])

    def test_duplicate_correlated_input_is_rejected(self) -> None:
        self.thread.turns[0].items *= 2
        self.assertFalse(correlate_message(self.thread, self.expected)["supported"])

    def test_native_identity_is_not_coerced_from_number(self) -> None:
        raw = self.thread.model_dump(mode="json", exclude_unset=True)
        raw["id"] = 123
        with self.assertRaises(ValueError):
            _ = correlate_message(raw, self.expected)


class RpcObservationCarrierTests(unittest.TestCase):
    def test_sse_notifications_are_not_the_final_tool_result(self) -> None:
        body = 'event: message\ndata: {"method":"notification","params":{}}\n\nevent: message\ndata: {"id":1,"result":{"events":[]}}\n\n'
        self.assertEqual(decode_rpc_body(body), {"id": 1, "result": {"events": []}})

    def test_wrong_response_cannot_be_used_as_observation(self) -> None:
        with self.assertRaises(ValueError):
            _ = decode_rpc_body('data: {"id":2,"result":{}}\n\n')


class CallbackCorrelationOracleTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        target = {
            "endpoint": {"serviceId": "service", "endpointId": "claude-local"},
            "sessionId": "requester",
        }
        generation = {"serviceEpoch": "epoch", "generation": 1}
        self.operation: OperationView = OperationView.model_validate(
            {
                "target": target,
                "binding": {"binding": {"generation": generation}},
                "inputId": "input",
            }
        )
        facts = [
            {"kind": "turnStarted", "turnId": "turn", "inputId": "input"},
            {
                "kind": "interactionRequested",
                "interaction": {
                    "kind": "question",
                    "request": {"requestId": "question"},
                },
            },
            {"kind": "interactionResolved", "requestId": "question"},
            {
                "kind": "itemUpdated",
                "item": {
                    "itemId": "question-tool",
                    "kind": {"kind": "toolCall", "status": "completed"},
                },
            },
            {
                "kind": "interactionRequested",
                "interaction": {
                    "kind": "approval",
                    "request": {
                        "requestId": "approval",
                        "subject": {"toolCall": {"toolCallId": "approval-tool"}},
                    },
                },
            },
            {"kind": "interactionResolved", "requestId": "approval"},
            {
                "kind": "itemUpdated",
                "item": {
                    "itemId": "approval-tool",
                    "kind": {"kind": "toolCall", "status": "completed"},
                },
            },
            {
                "kind": "turnEnded",
                "turnId": "turn",
                "outcome": {"kind": "ended", "stop_reason": "endTurn"},
            },
        ]
        self.capture: CallbackCapture = CallbackCapture.model_validate(
            {
                "target": target,
                "observation": {
                    "target": target,
                    "generation": generation,
                    "attached": True,
                    "endReason": "deadlineReached",
                    "continuationGap": True,
                },
                "events": [
                    {"sequence": index, "event": fact}
                    for index, fact in enumerate(facts, 1)
                ],
            }
        )

    def test_exact_callbacks_accept_protocols_unconditional_gap_flag(self) -> None:
        require_callback_capture(self.capture, self.operation, ["question", "approval"])

    def test_ordinary_prompt_omits_optional_public_operation_input_id(self) -> None:
        self.operation.inputId = None
        require_callback_capture(self.capture, self.operation, ["question", "approval"])

    def test_wrong_binding_generation_is_rejected(self) -> None:
        self.capture.observation.generation = GenerationView(
            serviceEpoch="wrong", generation=1
        )
        with self.assertRaisesRegex(ValueError, "binding generation"):
            require_callback_capture(
                self.capture, self.operation, ["question", "approval"]
            )

    def test_ack_without_actual_tool_completion_is_rejected(self) -> None:
        event = self.capture.events[6].event
        assert event.kind == "itemStarted" or event.kind == "itemUpdated"
        event.item.kind.status = "pending"
        with self.assertRaisesRegex(ValueError, "tool completion"):
            require_callback_capture(
                self.capture, self.operation, ["question", "approval"]
            )

    def test_callback_from_another_turn_is_rejected(self) -> None:
        event = self.capture.events[-1].event
        assert event.kind == "turnEnded"
        event.turnId = "other-turn"
        with self.assertRaisesRegex(ValueError, "turn terminal correlation"):
            require_callback_capture(
                self.capture, self.operation, ["question", "approval"]
            )

    def test_malformed_additional_callback_cannot_disappear_in_projection(self) -> None:
        raw = observed_object(self.capture.model_dump(mode="json", exclude_unset=True))
        raw_events = raw["events"]
        assert isinstance(raw_events, list)
        raw_events.append({"sequence": 9, "event": {"kind": "interactionResolved"}})
        with self.assertRaises(ValueError):
            require_callback_capture(raw, self.operation, ["question", "approval"])


class OwningReopenOracleTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.before: InteractionSnapshot = InteractionSnapshot.model_validate(
            {
                "database": "/owned/interaction.sqlite",
                "revision": 4,
                "records": [
                    {
                        "requestId": "question",
                        "createdAt": "2026-10-16T00:00:00.000000000Z",
                        "record": {
                            "kind": "question",
                            "state": {
                                "kind": "answered",
                                "content": {"choice": "first"},
                            },
                        },
                    }
                ],
            }
        )

    def test_exact_terminal_snapshot_survives(self) -> None:
        self.assertTrue(
            compare_reopen(
                self.before.model_dump(mode="json", exclude_unset=True),
                self.before.model_dump(mode="json", exclude_unset=True),
            )["supported"]
        )

    def test_ack_or_pending_cannot_prove_settlement(self) -> None:
        self.before.records[0].record.state = {"kind": "pending"}
        with self.assertRaisesRegex(ValueError, "pending or cancelled"):
            _ = compare_reopen(
                self.before.model_dump(mode="json", exclude_unset=True),
                self.before.model_dump(mode="json", exclude_unset=True),
            )

    def test_wrong_root_or_changed_creation_revision_content_fails(self) -> None:
        variations: list[tuple[str, str | int | list[str]]] = [
            ("database", "/wrong/interaction.sqlite"),
            ("revision", 5),
            ("records", []),
        ]
        for key, value in variations:
            with self.subTest(key=key):
                changed = self.before.model_dump(mode="json", exclude_unset=True)
                changed[key] = value
                with self.assertRaisesRegex(ValueError, "changed on owning reopen"):
                    _ = compare_reopen(
                        self.before.model_dump(mode="json", exclude_unset=True), changed
                    )

    def test_missing_database_is_not_created(self) -> None:
        with tempfile.TemporaryDirectory() as folder:
            database = Path(folder) / "missing.sqlite"
            with self.assertRaises(sqlite3.OperationalError):
                _ = read_snapshot(database, ["question"])
            self.assertFalse(database.exists())

    def test_scoped_sqlite_decoder_retains_raw_metadata_and_creation_precision(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as folder:
            database = Path(folder) / "apparatus.sqlite"
            record = self.before.records[0].record.model_dump(
                mode="json", exclude_unset=True
            )
            record["request"] = {"requestId": "question"}
            record["unknownMetadata"] = [True, None, "retained"]
            created = "2026-10-16T00:00:00.123456789Z"
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute(
                    "CREATE TABLE interaction_history_revision(metadata_id, revision)"
                )
                _ = connection.execute(
                    "INSERT INTO interaction_history_revision VALUES (1,4)"
                )
                _ = connection.execute(
                    "CREATE TABLE typed_interaction_history(request_id,record_json,created_at)"
                )
                _ = connection.execute(
                    "INSERT INTO typed_interaction_history VALUES (?,?,?)",
                    ("question", json.dumps(record), created),
                )
            snapshot = read_snapshot(database, ["question"])
            decoded = InteractionSnapshot.model_validate(snapshot)
            self.assertEqual(decoded.records[0].createdAt, created)
            self.assertEqual(
                decoded.records[0].record.model_dump(mode="json", exclude_unset=True),
                record,
            )
            self.assertEqual(decoded.revision, 4)
            with closing(sqlite3.connect(database)) as connection, connection:
                _ = connection.execute(
                    "UPDATE interaction_history_revision SET revision='not-an-integer'"
                )
            with self.assertRaises(ValueError):
                _ = read_snapshot(database, ["question"])

    def test_unknown_fields_and_nanosecond_timestamps_remain_exact(self) -> None:
        before = observed_object(
            self.before.model_dump(mode="json", exclude_unset=True)
        )
        before["opaqueMetadata"] = {"retained": [True, None, "unknown"]}
        after = copy.deepcopy(before)
        self.assertTrue(compare_reopen(before, after)["supported"])
        after["opaqueMetadata"] = {"retained": [True, None, "changed"]}
        with self.assertRaisesRegex(ValueError, "changed on owning reopen"):
            _ = compare_reopen(before, after)
        after = copy.deepcopy(before)
        after_view = InteractionSnapshot.model_validate(after)
        after_view.records[0].createdAt = "2026-10-16T00:00:00.000000001Z"
        after = observed_object(after_view.model_dump(mode="json", exclude_unset=True))
        with self.assertRaisesRegex(ValueError, "changed on owning reopen"):
            _ = compare_reopen(before, after)


class QuestionAutomationOracleTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.record: QuestionView = QuestionView.model_validate(
            {
                "fields": [
                    {
                        "kind": "singleChoice",
                        "fieldId": "question_0",
                        "required": False,
                        "options": [
                            {"optionId": "blue-id", "label": "Blue"},
                            {"optionId": "green-id", "label": "Green"},
                        ],
                    },
                    {"kind": "text", "fieldId": "question_0_custom", "required": False},
                ]
            }
        )

    def test_optional_adapter_other_field_does_not_block_offered_answer(self) -> None:
        self.assertEqual(
            question_response_for(self.record),
            {
                "action": "answered",
                "content": {"question_0": {"selectedOptionIds": ["blue-id"]}},
            },
        )

    def test_unexpected_required_field_gets_no_automatic_answer(self) -> None:
        self.record.fields[1].required = True
        with self.assertRaisesRegex(ValueError, "additional Question field"):
            _ = question_response_for(self.record)

    def test_changed_options_get_no_automatic_answer(self) -> None:
        self.record.fields[0].options[1].label = "Red"
        with self.assertRaisesRegex(ValueError, "unexpected Question options"):
            _ = question_response_for(self.record)


if __name__ == "__main__":
    _ = unittest.main()
