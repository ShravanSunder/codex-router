# Board source citation map — unchanged paths

Source baseline: `7a8cbd8943e6fb2dac23bcde7069203925003918`. This map covers eight production files cited by the board documentation track. Their paths and symbols remain unchanged by the current checkpoint; this is not a final refactor map.

All eight files are byte-identical to base, under 1000 physical lines, and outside the 28 oversized targets. Their old-to-new map is identity: same path, same symbol, same line. They are not proposed write surfaces. No board docs are edited. If a future admitted extraction affects a citation, publish a refreshed exact map before consumers re-anchor.

Ownership context: the coordinator reported that the board documentation track has no Rust path or symbol reservation. This is reported overlap disposition, not an implementation or proof grant. Detailed local coordination records remain outside the public checkpoint.

## Path map

| Base path | Current path | Physical lines | Result |
| --- | --- | ---: | --- |
| `crates/message-board-storage/src/message_write_operations.rs` | [same path](../../crates/message-board-storage/src/message_write_operations.rs#L1) | 595 | byte-identical; no move |
| `crates/message-board-storage/src/participant_records.rs` | [same path](../../crates/message-board-storage/src/participant_records.rs#L1) | 566 | byte-identical; no move |
| `crates/message-board-storage/src/storage_support.rs` | [same path](../../crates/message-board-storage/src/storage_support.rs#L1) | 560 | byte-identical; no move |
| `crates/collaboration-service/src/provider_session_event_hub.rs` | [same path](../../crates/collaboration-service/src/provider_session_event_hub.rs#L1) | 609 | byte-identical; no move |
| `crates/acp-client-runtime/src/provider_item_projection.rs` | [same path](../../crates/acp-client-runtime/src/provider_item_projection.rs#L1) | 331 | byte-identical; no move |
| `crates/collaboration-protocol/src/lifecycle_observation.rs` | [same path](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L1) | 156 | byte-identical; no move |
| `crates/message-board-storage/src/subscription_window_records.rs` | [same path](../../crates/message-board-storage/src/subscription_window_records.rs#L1) | 536 | byte-identical; no move |
| `crates/lifecycle-observation/src/native_lifecycle_mapping.rs` | [same path](../../crates/lifecycle-observation/src/native_lifecycle_mapping.rs#L1) | 122 | byte-identical; no move |

## Symbol anchors

| File | Base symbol and line | Current citation |
| --- | --- | --- |
| `message_write_operations.rs` | `BoardStore::post_message` at 26 | [BoardStore::post_message](../../crates/message-board-storage/src/message_write_operations.rs#L26) |
| `message_write_operations.rs` | `BoardStore::create_thread` at 201 | [BoardStore::create_thread](../../crates/message-board-storage/src/message_write_operations.rs#L201) |
| `message_write_operations.rs` | `enforce_and_record_cooldown` at 437 | [enforce_and_record_cooldown](../../crates/message-board-storage/src/message_write_operations.rs#L437) |
| `message_write_operations.rs` | `publish_message_unread` at 508 | [publish_message_unread](../../crates/message-board-storage/src/message_write_operations.rs#L508) |
| `participant_records.rs` | `apply_watch_choice` at 35 | [apply_watch_choice](../../crates/message-board-storage/src/participant_records.rs#L35) |
| `participant_records.rs` | `ParticipantChange` at 67 | [ParticipantChange](../../crates/message-board-storage/src/participant_records.rs#L67) |
| `participant_records.rs` | `BoardStore::leave_thread` at 161 | [BoardStore::leave_thread](../../crates/message-board-storage/src/participant_records.rs#L161) |
| `participant_records.rs` | `BoardStore::list_thread_participants` at 374 | [BoardStore::list_thread_participants](../../crates/message-board-storage/src/participant_records.rs#L374) |
| `participant_records.rs` | `resolve_in_transaction` at 448 | [resolve_in_transaction](../../crates/message-board-storage/src/participant_records.rs#L448) |
| `participant_records.rs` | `advance_participant_last_seen` at 491 | [advance_participant_last_seen](../../crates/message-board-storage/src/participant_records.rs#L491) |
| `storage_support.rs` | `BoardTransaction` at 11 | [BoardTransaction](../../crates/message-board-storage/src/storage_support.rs#L11) |
| `storage_support.rs` | `StoredIdentityRow` at 14 | [StoredIdentityRow](../../crates/message-board-storage/src/storage_support.rs#L14) |
| `storage_support.rs` | `encode_cursor` at 23 | [encode_cursor](../../crates/message-board-storage/src/storage_support.rs#L23) |
| `storage_support.rs` | `decode_cursor` at 38 | [decode_cursor](../../crates/message-board-storage/src/storage_support.rs#L38) |
| `storage_support.rs` | `identity_key` at 129 | [identity_key](../../crates/message-board-storage/src/storage_support.rs#L129) |
| `storage_support.rs` | `ensure_identity` at 141 | [ensure_identity](../../crates/message-board-storage/src/storage_support.rs#L141) |
| `storage_support.rs` | `ensure_acting_for_identity` at 185 | [ensure_acting_for_identity](../../crates/message-board-storage/src/storage_support.rs#L185) |
| `storage_support.rs` | `decode_identity` at 202 | [decode_identity](../../crates/message-board-storage/src/storage_support.rs#L202) |
| `storage_support.rs` | `allocate_activity_sequence` at 255 | [allocate_activity_sequence](../../crates/message-board-storage/src/storage_support.rs#L255) |
| `storage_support.rs` | `recompute_project_unread` at 284 | [recompute_project_unread](../../crates/message-board-storage/src/storage_support.rs#L284) |
| `storage_support.rs` | `validate_reader_activity_boundaries` at 326 | [validate_reader_activity_boundaries](../../crates/message-board-storage/src/storage_support.rs#L326) |
| `provider_session_event_hub.rs` | `ProviderSessionEventHub` at 17 | [ProviderSessionEventHub](../../crates/collaboration-service/src/provider_session_event_hub.rs#L17) |
| `provider_session_event_hub.rs` | `SessionHistory::project` at 79 | [SessionHistory::project](../../crates/collaboration-service/src/provider_session_event_hub.rs#L79) |
| `provider_session_event_hub.rs` | `ProviderSessionEventHub::publish` at 207 | [ProviderSessionEventHub::publish](../../crates/collaboration-service/src/provider_session_event_hub.rs#L207) |
| `provider_session_event_hub.rs` | `begin_history_replay` at 279 | [begin_history_replay](../../crates/collaboration-service/src/provider_session_event_hub.rs#L279) |
| `provider_session_event_hub.rs` | `begin_history_unavailable` at 317 | [begin_history_unavailable](../../crates/collaboration-service/src/provider_session_event_hub.rs#L317) |
| `provider_session_event_hub.rs` | `SessionEventHub implementation` at 371 | [SessionEventHub implementation](../../crates/collaboration-service/src/provider_session_event_hub.rs#L371) |
| `provider_session_event_hub.rs` | `HubReceiveError` at 525 | [HubReceiveError](../../crates/collaboration-service/src/provider_session_event_hub.rs#L525) |
| `provider_session_event_hub.rs` | `receive_hub_event` at 530 | [receive_hub_event](../../crates/collaboration-service/src/provider_session_event_hub.rs#L530) |
| `provider_item_projection.rs` | `ItemProjectionError` at 14 | [ItemProjectionError](../../crates/acp-client-runtime/src/provider_item_projection.rs#L14) |
| `provider_item_projection.rs` | `ProviderItemProjection` at 30 | [ProviderItemProjection](../../crates/acp-client-runtime/src/provider_item_projection.rs#L30) |
| `provider_item_projection.rs` | `ProviderItemProjection::observe` at 57 | [ProviderItemProjection::observe](../../crates/acp-client-runtime/src/provider_item_projection.rs#L57) |
| `provider_item_projection.rs` | `finish_text` at 151 | [finish_text](../../crates/acp-client-runtime/src/provider_item_projection.rs#L151) |
| `provider_item_projection.rs` | `finish` at 251 | [finish](../../crates/acp-client-runtime/src/provider_item_projection.rs#L251) |
| `provider_item_projection.rs` | `observe_unknown` at 264 | [observe_unknown](../../crates/acp-client-runtime/src/provider_item_projection.rs#L264) |
| `lifecycle_observation.rs` | `ThreadAddress` at 6 | [ThreadAddress](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L6) |
| `lifecycle_observation.rs` | `ObservationScope` at 12 | [ObservationScope](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L12) |
| `lifecycle_observation.rs` | `ObservationSource` at 19 | [ObservationSource](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L19) |
| `lifecycle_observation.rs` | `LifecycleSubject` at 27 | [LifecycleSubject](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L27) |
| `lifecycle_observation.rs` | `NativeThreadStatus` at 61 | [NativeThreadStatus](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L61) |
| `lifecycle_observation.rs` | `LifecycleChange` at 72 | [LifecycleChange](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L72) |
| `lifecycle_observation.rs` | `LifecycleObservation` at 95 | [LifecycleObservation](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L95) |
| `lifecycle_observation.rs` | `LifecycleObservation::validate` at 103 | [LifecycleObservation::validate](../../crates/collaboration-protocol/src/lifecycle_observation.rs#L103) |
| `subscription_window_records.rs` | `StoredSelectionWindow` at 23 | [StoredSelectionWindow](../../crates/message-board-storage/src/subscription_window_records.rs#L23) |
| `subscription_window_records.rs` | `load_window` at 54 | [load_window](../../crates/message-board-storage/src/subscription_window_records.rs#L54) |
| `subscription_window_records.rs` | `rescan_missing_windows_in_transaction` at 114 | [rescan_missing_windows_in_transaction](../../crates/message-board-storage/src/subscription_window_records.rs#L114) |
| `subscription_window_records.rs` | `BoardStore::due_subscription_roots` at 260 | [BoardStore::due_subscription_roots](../../crates/message-board-storage/src/subscription_window_records.rs#L260) |
| `subscription_window_records.rs` | `BoardStore::select_subscription_notice` at 311 | [BoardStore::select_subscription_notice](../../crates/message-board-storage/src/subscription_window_records.rs#L311) |
| `subscription_window_records.rs` | `record_subscription_post` at 447 | [record_subscription_post](../../crates/message-board-storage/src/subscription_window_records.rs#L447) |
| `native_lifecycle_mapping.rs` | `map_native_lifecycle` at 10 | [map_native_lifecycle](../../crates/lifecycle-observation/src/native_lifecycle_mapping.rs#L10) |
| `native_lifecycle_mapping.rs` | `parse_id` at 114 | [parse_id](../../crates/lifecycle-observation/src/native_lifecycle_mapping.rs#L114) |

## Established API/module homes

- Storage roots privately declare these modules; inherent methods remain on public `message_board_storage::BoardStore`. No storage/SQLx query, metadata or migration edits.
- Provider hub remains crate-root reexported at [collaboration-service lib.rs](../../crates/collaboration-service/src/lib.rs#L50).
- Provider item projection remains private, declared at [acp-client-runtime lib.rs](../../crates/acp-client-runtime/src/lib.rs#L42).
- Lifecycle wire types remain crate-root reexports at [collaboration-protocol lib.rs](../../crates/collaboration-protocol/src/lib.rs#L192). Their serde/schema attributes are unchanged.
- Native lifecycle mapper remains crate-root reexported at [lifecycle-observation lib.rs](../../crates/lifecycle-observation/src/lib.rs#L29).

Same-basename integration companions found during path lookup are `collaboration-service/tests/provider_session_event_hub.rs`, `collaboration-protocol/tests/lifecycle_observation.rs`, and `lifecycle-observation/tests/native_lifecycle_mapping.rs`. They are separate test targets, not substitutes for the production citation paths above. No changes proposed to them.

Preservation obligation: CLI/MCP JSON and serde contracts, SQLx text/metadata/migrations, seat/actor/SessionRef semantics remain unchanged. Identity map is source evidence only, not a runtime test or semantic regression pass.

Verification: direct definitions and relevant crate module/reexport declarations read; each current byte string compared with `git show 7a8cbd8943e6fb2dac23bcde7069203925003918:<path>`; all equal. Existing inventory membership checked: none of eight is an oversized target. Python/Git read-only check exit0. No Cargo/network/dependency/cache/firewall/toolchain command.
