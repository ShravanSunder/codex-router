//! Complete Control 1.0 wire schema and request/result/error pairings.
use crate::*;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

/// Binds native payloads to an exact advertised export. Without one, native-dependent
/// payload shapes are unavailable; metadata calls and error responses remain defined.
pub fn control_schema_document(
    native_digest: Option<&SchemaDigest>,
) -> Result<Value, serde_json::Error> {
    let native_uri = native_digest.map(|digest| {
        let digest = String::from(digest.clone());
        format!(
            "codex-schema://{}/codex_app_server_protocol.schemas.json",
            digest.trim_start_matches("sha256:")
        )
    });
    let mut assembly = SchemaAssembly {
        definitions: Map::new(),
        methods: Map::new(),
        frames: Vec::new(),
        native_uri,
    };
    assembly.add_type::<project_board::BoardError>("board-failure")?;
    assembly
        .add_method::<project_board::ProjectCreateRequest, project_board::ProjectCreateResult>(
            "board/projectCreate",
            &[],
        )?;
    assembly
        .add_method::<project_board::ProjectUpdateRequest, project_board::ProjectUpdateResult>(
            "board/projectUpdate",
            &[],
        )?;
    assembly.add_method::<project_board::ProjectShowRequest, project_board::ProjectShowResult>(
        "board/projectShow",
        &[],
    )?;
    assembly.add_method::<project_board::ProjectListRequest, project_board::ProjectListResult>(
        "board/projectList",
        &[],
    )?;
    assembly.add_method::<project_board::RepositoryAttachRequest, project_board::RepositoryAttachResult>("board/repositoryAttach", &[])?;
    assembly.add_method::<project_board::RepositoryDetachRequest, project_board::RepositoryDetachResult>("board/repositoryDetach", &[])?;
    assembly
        .add_method::<project_board::RepositoryListRequest, project_board::RepositoryListResult>(
            "board/repositoryList",
            &[],
        )?;
    assembly.add_method::<project_board::BoardCreateRequest, project_board::BoardCreateResult>(
        "board/create",
        &[],
    )?;
    assembly.add_method::<project_board::BoardUpdateRequest, project_board::BoardUpdateResult>(
        "board/update",
        &[],
    )?;
    assembly.add_method::<project_board::BoardShowRequest, project_board::BoardShowResult>(
        "board/show",
        &[],
    )?;
    assembly.add_method::<project_board::BoardListRequest, project_board::BoardListResult>(
        "board/list",
        &[],
    )?;
    assembly.add_method::<project_board::BoardArchiveRequest, project_board::BoardArchiveResult>(
        "board/archive",
        &[],
    )?;
    assembly.add_method::<project_board::TopicCreateRequest, project_board::TopicCreateResult>(
        "board/topicCreate",
        &[],
    )?;
    assembly.add_method::<project_board::TopicUpdateRequest, project_board::TopicUpdateResult>(
        "board/topicUpdate",
        &[],
    )?;
    assembly.add_method::<project_board::TopicListRequest, project_board::TopicListResult>(
        "board/topicList",
        &[],
    )?;
    assembly.add_method::<project_board::MessagePostRequest, project_board::MessagePostResult>(
        "board/messagePost",
        &[],
    )?;
    assembly.add_method::<project_board::MessageShowRequest, project_board::MessageShowResult>(
        "board/messageShow",
        &[],
    )?;
    assembly.add_method::<project_board::MessageListRequest, project_board::MessageListResult>(
        "board/messageList",
        &[],
    )?;
    assembly.add_method::<project_board::ThreadShowRequest, project_board::ThreadShowResult>(
        "board/threadShow",
        &[],
    )?;
    assembly
        .add_method::<project_board::ThreadResolveRequest, project_board::ThreadResolveResult>(
            "board/threadResolve",
            &[],
        )?;
    assembly
        .add_method::<project_board::ThreadUnresolveRequest, project_board::ThreadUnresolveResult>(
            "board/threadUnresolve",
            &[],
        )?;
    assembly.add_method::<project_board::ThreadWatchRequest, project_board::ThreadWatchResult>(
        "board/threadWatch",
        &[],
    )?;
    assembly
        .add_method::<project_board::ThreadUnwatchRequest, project_board::ThreadUnwatchResult>(
            "board/threadUnwatch",
            &[],
        )?;
    assembly.add_method::<project_board::ThreadListRequest, project_board::ThreadListResult>(
        "board/threadList",
        &[],
    )?;
    assembly.add_method::<project_board::InboxFetchRequest, project_board::InboxFetchResult>(
        "board/inboxFetch",
        &[],
    )?;
    assembly.add_method::<project_board::InboxAcknowledgeRequest, project_board::InboxAcknowledgeResult>("board/inboxAcknowledge", &[])?;
    assembly
        .add_method::<project_board::InboxProjectsRequest, project_board::InboxProjectsResult>(
            "board/inboxProjects",
            &[],
        )?;
    assembly.add_type::<ConfigurationFailure>("configuration-failure")?;
    assembly.add_type::<AutomationInspectionFailure>("automation-inspection-failure")?;
    assembly.add_method::<OperationShowRequest, OperationSnapshot>("operation/show", &[])?;
    assembly.add_method::<OperationShowRequest, OperationSnapshot>("operation/reconcile", &[])?;
    assembly.add_method::<DeliveryShowRequest, DeliveryInspection>("delivery/reconcile", &[])?;
    assembly.add_method::<RunShowRequest, RunSnapshot>("run/reconcile", &[])?;
    assembly
        .add_method::<AutomationEventsRequest, AutomationEventsPage>("automation/events", &[])?;
    assembly.add_method::<DeliveryAttemptsRequest, AttemptHistoryPage<AttemptInspection>>(
        "delivery/attempts",
        &[],
    )?;
    assembly.add_method::<RunSummariesRequest, AttemptHistoryPage<SummaryInspection>>(
        "run/summaries",
        &[],
    )?;
    assembly.add_method::<AutomationPageRequest, AutomationPage<InstructionSnapshot>>(
        "instruction/list",
        &[],
    )?;
    assembly.add_method::<AutomationPageRequest, AutomationPage<ScheduleSnapshot>>(
        "schedule/list",
        &[],
    )?;
    assembly.add_method::<RunListRequest, AutomationPage<RunSnapshot>>("run/list", &[])?;
    assembly
        .add_method::<RevisionListRequest, AutomationPage<RevisionRecord>>("revision/list", &[])?;
    assembly.add_method::<DeliveryListRequest, AutomationPage<DeliveryInspection>>(
        "delivery/list",
        &[],
    )?;
    assembly.add_method::<AutomationConfigureRequest, AutomationConfiguration>(
        "automation/configure",
        &[],
    )?;
    assembly.add_method::<EmptyParams, AutomationStatus>("automation/status", &[])?;
    assembly.add_type::<RunFailure>("run-failure")?;
    assembly.add_method::<RunShowRequest, RunSnapshot>("run/show", &[])?;
    for method in ["run/summaryRetry", "run/summarySkip"] {
        assembly.add_method::<RunRecoveryRequest, RunSnapshot>(method, &[])?;
    }
    assembly.add_type::<ScheduleFailure>("schedule-failure")?;
    assembly.add_method::<SchedulePrepareRequest, ScheduleSnapshot>("schedule/prepare", &[])?;
    assembly.add_method::<ScheduleCreateRequest, ScheduleSnapshot>("schedule/create", &[])?;
    assembly.add_method::<ScheduleShowRequest, ScheduleSnapshot>("schedule/show", &[])?;
    assembly.add_method::<ScheduleShowRequest, ScheduleExportResult>("schedule/export", &[])?;
    assembly.add_method::<ScheduleImportRequest, ScheduleSnapshot>("schedule/import", &[])?;
    assembly.add_method::<ScheduleUpdateRequest, ScheduleSnapshot>("schedule/update", &[])?;
    for method in ["schedule/enable", "schedule/disable"] {
        assembly.add_method::<ScheduleEnableRequest, ScheduleSnapshot>(method, &[])?;
    }
    assembly.add_type::<InstructionFailure>("instruction-failure")?;
    assembly.add_type::<WakeFailure>("wake-failure")?;
    assembly.add_method::<AutomationPageRequest, AutomationPage<WakeSnapshot>>("wake/list", &[])?;
    assembly.add_type::<WaitUnavailable>("wait-unavailable")?;
    assembly.add_type::<WakeNotFound>("wake-not-found")?;
    assembly.add_type::<WakeChanged>("wake-changed")?;
    assembly.add_method::<WakeShowRequest, WakeSubscription>("wake/subscribe", &[])?;
    for method in ["wake/pause", "wake/resume", "wake/cancel"] {
        assembly.add_method::<WakeMutationRequest, WakeMutationResult>(method, &[])?;
    }
    assembly.add_method::<DeliveryShowRequest, DeliveryInspection>("delivery/show", &[])?;
    assembly.add_method::<WakeSendRequest, WakeSnapshot>(
        "wake/send",
        &["invalidField", "automationUnavailable", "operationConflict"],
    )?;
    assembly.add_method::<WakeShowRequest, WakeSnapshot>(
        "wake/show",
        &["resourceNotFound", "automationUnavailable"],
    )?;
    assembly
        .add_method::<InstructionCreateParams, InstructionSnapshot>("instruction/create", &[])?;
    assembly
        .add_method::<InstructionUpdateParams, InstructionSnapshot>("instruction/update", &[])?;
    assembly.add_method::<InstructionShowParams, InstructionSnapshot>("instruction/show", &[])?;
    assembly.add_type::<JournalBounds>("journal-bounds")?;
    assembly.add_method::<ControlInitializationParams, ControlInitializationResult>(
        "control/initialize",
        &["unsupportedVersion", "unavailable"],
    )?;
    assembly.add_method::<EmptyParams, EndpointInventory>(
        "endpoint/list",
        &["unavailable", "overloaded"],
    )?;
    assembly.add_method::<NativeSessionListParams, NativeSessionListResult>(
        "codex/sessionList",
        &[
            "wrongService",
            "endpointNotFound",
            "unsupportedCapability",
            "unavailable",
            "overloaded",
        ],
    )?;
    assembly.add_method::<NativeInspectParams, NativeInspectResult>(
        "codex/sessionInspect",
        &[
            "wrongService",
            "endpointNotFound",
            "unsupportedCapability",
            "unavailable",
            "nativeRejected",
            "outcomeUnknown",
            "overloaded",
        ],
    )?;
    assembly.add_method::<NativeSendParams, NativeSendReceipt>(
        "codex/messageSend",
        &[
            "wrongService",
            "endpointNotFound",
            "unsupportedCapability",
            "staleGeneration",
            "unavailable",
            "nativeRejected",
            "outcomeUnknown",
            "overloaded",
            "threadNotLoaded",
            "noActiveTurn",
        ],
    )?;
    assembly.add_method::<NativeInterruptParams, NativeInterruptResult>(
        "codex/turnInterrupt",
        &[
            "wrongService",
            "endpointNotFound",
            "unsupportedCapability",
            "staleGeneration",
            "unavailable",
            "nativeRejected",
            "outcomeUnknown",
            "overloaded",
        ],
    )?;
    assembly.add_method::<AddressListParams, AddressPage>(
        "addressBook/list",
        &[
            "wrongService",
            "endpointNotFound",
            "unavailable",
            "overloaded",
        ],
    )?;
    assembly.add_method::<JournalReadParams, JournalPage>(
        "lifecycleJournal/read",
        &[
            "wrongService",
            "endpointNotFound",
            "unavailable",
            "overloaded",
        ],
    )?;
    assembly
        .add_method::<EmptyParams, JournalStatus>("lifecycleJournal/status", &["overloaded"])?;
    let notification_params = assembly.add_type::<EndpointChange>("endpoint-change-params")?;
    assembly.definitions.insert("endpoint-change-notification".into(), json!({
        "type":"object","required":["jsonrpc","method","params"],"additionalProperties":false,
        "properties":{"jsonrpc":{"const":"2.0"},"method":{"const":"endpoint/changed"},"params":notification_params}
    }));
    assembly
        .frames
        .push(reference("endpoint-change-notification"));
    assembly.definitions.insert("wake-change-notification".into(),json!({"type":"object","required":["jsonrpc","method","params"],"additionalProperties":false,"properties":{"jsonrpc":{"const":"2.0"},"method":{"const":"wake/changed"},"params":reference("wake-changed")}}));
    assembly.frames.push(reference("wake-change-notification"));
    Ok(json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"urn:agent-communication:control:1",
        "title":"ControlProtocol",
        "description":"Control 1.0 individual frames. Request IDs bind each response to its x-methods contract; frame shape alone cannot identify a response's method. Connection budgets, UTF-8 byte limits, negative-zero rejection and lifecycle ordering are codec/runtime obligations.",
        "anyOf":assembly.frames,
        "$defs":assembly.definitions,
        "x-protocolVersion":{"major":1,"minor":0},
        "x-nativeSchemaDigest":native_digest,
        "x-methods":assembly.methods,
        "x-notifications":{"endpoint/changed":reference("endpoint-change-notification"),"wake/changed":reference("wake-change-notification")},
        "x-maxFrameUtf8Bytes":1048576,
        "x-maxPendingRequests":64,
        "x-maxRequestsPerConnection":65536
    }))
}

struct SchemaAssembly {
    definitions: Map<String, Value>,
    methods: Map<String, Value>,
    frames: Vec<Value>,
    native_uri: Option<String>,
}
impl SchemaAssembly {
    fn add_type<TWire: JsonSchema>(&mut self, key: &str) -> Result<Value, serde_json::Error> {
        let schema = SchemaSettings::draft2020_12()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<TWire>();
        let mut schema = serde_json::to_value(schema)?;
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "$id".into(),
                json!(format!("urn:agent-communication:control:1:{key}")),
            );
        }
        bind_native_references(&mut schema, self.native_uri.as_deref());
        self.definitions.insert(key.to_owned(), schema);
        Ok(reference(key))
    }
    fn add_method<TParams: JsonSchema, TResult: JsonSchema>(
        &mut self,
        method: &str,
        failures: &[&str],
    ) -> Result<(), serde_json::Error> {
        let stem = method.replace('/', "-");
        let params = self.add_type::<TParams>(&format!("{stem}-params"))?;
        let result = self.add_type::<TResult>(&format!("{stem}-result"))?;
        let request_key = format!("{stem}-request");
        let response_key = format!("{stem}-response");
        let error_key = format!("{stem}-error");
        self.definitions.insert(request_key.clone(), json!({
            "type":"object","required":["jsonrpc","id","method","params"],"additionalProperties":false,
            "properties":{"jsonrpc":{"const":"2.0"},"id":request_id(false),"method":{"const":method},"params":params}
        }));
        self.definitions.insert(
            response_key.clone(),
            json!({
                "type":"object","required":["jsonrpc","id","result"],"additionalProperties":false,
                "properties":{"jsonrpc":{"const":"2.0"},"id":request_id(false),"result":result}
            }),
        );
        self.definitions
            .insert(error_key.clone(), method_error(method, failures));
        self.frames.extend([
            reference(&request_key),
            reference(&response_key),
            reference(&error_key),
        ]);
        self.methods.insert(method.to_owned(), json!({"params":params,"result":result,"request":reference(&request_key),"response":reference(&response_key),"error":reference(&error_key)}));
        Ok(())
    }
}
fn reference(key: &str) -> Value {
    json!({"$ref":format!("#/$defs/{key}")})
}
fn request_id(nullable: bool) -> Value {
    json!({"type":if nullable { json!(["string","null"]) } else { json!("string") },"minLength":1,"maxLength":128,"x-maxUtf8Bytes":128})
}
fn method_error(method: &str, failures: &[&str]) -> Value {
    let stages = [
        "initialize",
        "discovery",
        "inspect",
        "resume",
        "start",
        "queue",
        "steer",
        "interrupt",
    ];
    let text = json!({"type":"string","minLength":1,"maxLength":1024,"x-maxUtf8Bytes":1024});
    let mut properties = Map::from_iter([
        ("kind".to_owned(), json!({"enum":failures})),
        ("stage".to_owned(), json!({"enum":stages})),
        ("message".to_owned(), text.clone()),
    ]);
    let mut required = vec!["kind", "stage", "message"];
    if method == "codex/messageSend" {
        required.push("effects");
        properties.insert(
            "effects".to_owned(),
            json!({
                "type":"object", "required":["resume","submission"], "additionalProperties":false,
                "properties":{
                    "resume":{"enum":["notRequested","accepted","rejected","unknown"]},
                    "submission":{"enum":["notDispatched","rejected","unknown"]}
                }
            }),
        );
        properties.insert(
            "clientUserMessageId".to_owned(),
            json!({
                "type":"string", "minLength":1,"maxLength":4096,
                "pattern":"^[^\\u0000]+$", "x-maxUtf8Bytes":4096
            }),
        );
    }
    let mut data = vec![json!({"type":"object","required":required,
        "additionalProperties":false,"properties":properties})];
    if method.starts_with("wake/") || method.starts_with("delivery/") {
        data = vec![reference("wake-failure")];
    }
    if method == "wake/subscribe" {
        data = vec![reference("wait-unavailable"), reference("wake-not-found")];
    }
    if matches!(method, "automation/configure" | "automation/status") {
        data = vec![reference("configuration-failure")];
    }
    if method.starts_with("board/") {
        data = vec![reference("board-failure")];
    }
    if method.starts_with("run/") {
        data = vec![reference("run-failure")];
    }
    if method.starts_with("schedule/") {
        data = vec![reference("schedule-failure")];
    }
    if method.starts_with("instruction/") {
        data = vec![reference("instruction-failure")];
    }
    if matches!(
        method,
        "instruction/list"
            | "schedule/list"
            | "run/list"
            | "revision/list"
            | "delivery/list"
            | "automation/events"
            | "delivery/attempts"
            | "run/summaries"
            | "operation/show"
            | "operation/reconcile"
            | "delivery/reconcile"
            | "run/reconcile"
    ) {
        data = vec![reference("automation-inspection-failure")];
    }
    if method == "lifecycleJournal/read" {
        data.push(json!({"type":"object","required":["kind","current"],"additionalProperties":false,
            "properties":{"kind":{"enum":["journalChanged","historyExpired"]},"current":reference("journal-bounds")}}));
    }
    if method == "addressBook/list" {
        data.push(json!({"type":"object","required":["kind"],"additionalProperties":false,"properties":{"kind":{"const":"snapshotExpired"}}}));
    }
    json!({"type":"object","required":["jsonrpc","id","error"],"additionalProperties":false,
    "properties":{"jsonrpc":{"const":"2.0"},"id":request_id(true),"error":{"anyOf":[
        {"type":"object","required":["code","message"],"additionalProperties":false,
         "properties":{"code":{"enum":[-32700,-32600,-32601,-32602,-32603]},"message":text,"data":true}},
        {"type":"object","required":["code","message","data"],"additionalProperties":false,
         "properties":{"code":{"const":-32050},"message":text,"data":{"anyOf":data}}}
    ]}}})
}
fn bind_native_references(value: &mut Value, native_uri: Option<&str>) {
    if let Some(name) = value
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("urn:codex-native:"))
    {
        *value = match native_uri {
            Some(uri) => json!({"$ref":format!("{uri}#/definitions/v2/{name}")}),
            None => Value::Bool(false),
        };
        return;
    }
    match value {
        Value::Object(fields) => {
            for child in fields.values_mut() {
                bind_native_references(child, native_uri);
            }
        }
        Value::Array(items) => {
            for child in items {
                bind_native_references(child, native_uri);
            }
        }
        _ => {}
    }
}
