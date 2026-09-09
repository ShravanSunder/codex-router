//! Control connection admission and discovery bootstrap. Native dispatch is separate.
use crate::{EndpointDirectory, EndpointSubscription};
use communication_protocol::{
    AdmissionError, ControlAdmission, ControlFrameDecoder, EndpointDescription, UuidIdentity,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Clone)]
pub struct ServiceIdentity {
    service_id: UuidIdentity,
    service_epoch: UuidIdentity,
    schema_digest: communication_protocol::SchemaDigest,
    directory: EndpointDirectory,
    wake_wait_permits: std::sync::Arc<tokio::sync::Semaphore>,
    journal: Option<std::sync::Arc<lifecycle_observation::LifecycleStore>>,
    native_backend: Option<crate::NativeControlBackend>,
    automation: Option<std::sync::Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>>,
}
impl ServiceIdentity {
    pub fn wake_timing_worker(&self) -> Option<crate::WakeTimingWorker> {
        self.automation.as_ref().map(|store| {
            crate::WakeTimingWorker::new(
                std::sync::Arc::clone(store),
                crate::wakeup_native_sender::WakeNativeSender {
                    service_id: self.service_id.clone(),
                    endpoints: self.directory.clone(),
                    backend: self.native_backend.clone(),
                },
            )
        })
    }

    pub fn with_automation_store(
        mut self,
        store: std::sync::Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>,
    ) -> Self {
        self.automation = Some(store);
        self
    }

    pub fn with_native_backend(
        mut self,
        backend: crate::NativeControlBackend,
    ) -> Result<Self, String> {
        if backend.endpoint.service_id != self.service_id {
            return Err("native backend belongs to another service".into());
        }
        self.native_backend = Some(backend);
        Ok(self)
    }
    /// Registers a bounded inventory without inferring endpoint liveness.
    pub fn with_endpoints(self, endpoints: Vec<EndpointDescription>) -> Result<Self, String> {
        let mut identities = std::collections::HashSet::new();
        if endpoints.len() > 64 {
            return Err("too many endpoints".into());
        }
        for endpoint in &endpoints {
            if endpoint.endpoint.service_id != self.service_id {
                return Err("endpoint belongs to another service".into());
            }
            if !identities.insert(endpoint.endpoint.clone()) {
                return Err("duplicate endpoint identity".into());
            }
            if !(1..=2).contains(&endpoint.channels.len()) {
                return Err("invalid channel count".into());
            }
        }
        for endpoint in endpoints {
            self.directory
                .publish(endpoint)
                .map_err(|error| error.to_string())?;
        }
        Ok(self)
    }

    #[must_use]
    pub fn endpoint_directory(&self) -> EndpointDirectory {
        self.directory.clone()
    }

    pub fn with_journal(
        mut self,
        journal: std::sync::Arc<lifecycle_observation::LifecycleStore>,
    ) -> Self {
        self.journal = Some(journal);
        self
    }

    pub fn new(service_id: &str, service_epoch: &str, schema_digest: &str) -> Result<Self, String> {
        let digest = communication_protocol::SchemaDigest::try_from(schema_digest.to_owned())
            .map_err(str::to_owned)?;
        Ok(Self {
            service_id: UuidIdentity::try_from(service_id.to_owned())
                .map_err(|error| error.to_string())?,
            service_epoch: UuidIdentity::try_from(service_epoch.to_owned())
                .map_err(|error| error.to_string())?,
            schema_digest: digest,
            journal: None,
            native_backend: None,
            wake_wait_permits: std::sync::Arc::new(tokio::sync::Semaphore::new(16)),
            automation: None,
            directory: EndpointDirectory::new(
                UuidIdentity::try_from(service_id.to_owned()).map_err(|error| error.to_string())?,
            ),
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    jsonrpc: String,
    id: String,
    method: String,
    params: Value,
}
/// Serves admitted Control calls; native effects use the separately bound generation gate.
pub async fn serve_control_connection(
    mut stream: UnixStream,
    identity: ServiceIdentity,
) -> io::Result<()> {
    let mut subscription = identity.directory.subscribe()?;
    let mut wake_subscription: Option<crate::wakeup_subscription::WakeSubscriptionState> = None;
    let mut wake_poll = tokio::time::interval(std::time::Duration::from_millis(50));
    wake_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut decoder = ControlFrameDecoder::default();
    let mut admission = ControlAdmission::default();
    let mut buffer = [0_u8; 8192];
    let mut pending = tokio::task::JoinSet::<(String, Value)>::new();
    loop {
        let count = tokio::select! {
            result = stream.read(&mut buffer) => result?,
            _=wake_poll.tick(), if wake_subscription.is_some()=>{
                if let Some(wake)=&mut wake_subscription {
                    for change in wake.next_changes().await? {
                        let mut output=serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"wake/changed","params":change})).map_err(io::Error::other)?;
                        output.push(b'\n');stream.write_all(&output).await?;
                    }
                }
                continue;
            },
            completion = pending.join_next(), if !pending.is_empty() => {
                let (id,response)=completion.ok_or_else(||io::Error::other("pending task missing"))?.map_err(|_|io::Error::other("Control task failed"))?;
                admission.complete(&id);
                let mut output=serde_json::to_vec(&response).map_err(io::Error::other)?;output.push(b'\n');stream.write_all(&output).await?;continue;
            },
            update = subscription.next(), if admission.is_initialized() => {
                let update = update?;
                let mut output = serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"endpoint/changed","params":{"serviceEpoch":identity.service_epoch,"sequence":update.sequence,"endpoint":update.endpoint}})).map_err(io::Error::other)?;
                output.push(b'\n');
                stream.write_all(&output).await?;
                continue;
            }
        };
        if count == 0 {
            return decoder.finish().map_err(io::Error::other);
        }
        let input = buffer
            .get(..count)
            .ok_or_else(|| io::Error::other("read boundary"))?;
        let frames = decoder.push(input).map_err(io::Error::other)?;
        for frame in frames {
            // Notifications do not dispatch request-only methods and receive no response.
            if frame.get("id").is_none() {
                continue;
            }
            let response = match admit_request(frame, &mut admission) {
                Err(response) => response,
                Ok(request) if request.method == "wake/subscribe" => {
                    admission.complete(&request.id);
                    match serde_json::from_value::<communication_protocol::WakeShowRequest>(
                        request.params,
                    ) {
                        Err(_) => error(
                            json!(request.id),
                            -32602,
                            "Provide exact wakeupId for first-fire subscription",
                        ),
                        Ok(params) => {
                            let wakeup_id = params.wakeup_id.clone();
                            let permit = std::sync::Arc::clone(&identity.wake_wait_permits)
                                .try_acquire_owned();
                            if wake_subscription.is_some() {
                                crate::wakeup_subscription::unavailable(
                                    json!(request.id),
                                    wakeup_id,
                                )
                            } else if let (Some(store), Ok(permit)) =
                                (identity.automation.as_ref(), permit)
                            {
                                match crate::wakeup_subscription::start(
                                    std::sync::Arc::clone(store),
                                    identity.service_id.clone(),
                                    params,
                                    permit,
                                )
                                .await
                                {
                                    Ok((state, result)) => {
                                        wake_subscription = Some(state);
                                        json!({"jsonrpc":"2.0","id":request.id,"result":result})
                                    }
                                    Err(automation_storage::StorageError::WakeNotFound) => {
                                        crate::wakeup_subscription::not_found(
                                            json!(request.id),
                                            wakeup_id,
                                        )
                                    }
                                    Err(_) => crate::wakeup_subscription::unavailable(
                                        json!(request.id),
                                        wakeup_id,
                                    ),
                                }
                            } else {
                                crate::wakeup_subscription::unavailable(
                                    json!(request.id),
                                    wakeup_id,
                                )
                            }
                        }
                    }
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "wake/send"
                            | "wake/list"
                            | "wake/show"
                            | "wake/pause"
                            | "wake/resume"
                            | "wake/cancel"
                            | "delivery/show"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response =
                            crate::wakeup_dispatch::dispatch(crate::wakeup_dispatch::WakeRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            })
                            .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "schedule/prepare" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::schedule_preparation_dispatch::dispatch(
                            crate::schedule_preparation_dispatch::PreparationRequest {
                                id: json!(id),
                                params: request.params,
                                service_id: &identity.service_id,
                                backend: identity.native_backend.as_ref(),
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "schedule/create"
                            | "schedule/show"
                            | "schedule/update"
                            | "schedule/enable"
                            | "schedule/disable"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::schedule_dispatch::dispatch(
                            crate::schedule_dispatch::ScheduleRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "instruction/create" | "instruction/update" | "instruction/show"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::instruction_dispatch::dispatch(
                            crate::instruction_dispatch::InstructionRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "codex/sessionInspect"
                            | "codex/turnInterrupt"
                            | "codex/messageSend"
                            | "codex/sessionList"
                    ) =>
                {
                    let identity = identity.clone();
                    let endpoints = subscription.snapshot()?.endpoints;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::native_control_dispatch::dispatch_native(
                            crate::native_control_dispatch::NativeControlRequest {
                                method: &request.method,
                                params: request.params,
                                id: json!(id),
                                service_id: &identity.service_id,
                                backend: identity.native_backend.as_ref(),
                                endpoints: &endpoints,
                                stored_observation: identity.journal.as_deref().map(|store| {
                                    crate::stored_inventory_observation::StoredInventoryObservation {
                                        store,
                                        observer_id: &identity.service_epoch,
                                    }
                                }),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "lifecycleJournal/read" | "lifecycleJournal/status" | "addressBook/list"
                    ) =>
                {
                    let identity = identity.clone();
                    let endpoints = subscription.snapshot()?.endpoints;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::journal_dispatch::dispatch_journal(
                            &request.method,
                            request.params,
                            json!(id),
                            identity.service_id,
                            identity.journal,
                            endpoints,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) => dispatch(request, &identity, &subscription, &mut admission),
            };
            let mut output = serde_json::to_vec(&response).map_err(io::Error::other)?;
            output.push(b'\n');
            stream.write_all(&output).await?;
        }
    }
}
fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn admit_request(frame: Value, admission: &mut ControlAdmission) -> Result<Request, Value> {
    let id = frame
        .get("id")
        .filter(|value| value.is_string())
        .cloned()
        .unwrap_or(Value::Null);
    let Ok(request) = serde_json::from_value::<Request>(frame) else {
        return Err(error(id, -32600, "Invalid request"));
    };
    if request.jsonrpc != "2.0" {
        return Err(error(id, -32600, "Invalid JSON-RPC version"));
    }
    if let Err(failure) = admission.admit(&request.id, &request.method) {
        if failure == AdmissionError::Overloaded {
            if request.method.starts_with("wake/") || request.method.starts_with("delivery/") {
                return Err(crate::wakeup_dispatch::overloaded(id));
            }
            if request.method.starts_with("instruction/") {
                return Err(crate::instruction_dispatch::overloaded(id));
            }
            if request.method == "control/initialize" {
                return Err(error(id, -32603, "Request capacity exceeded"));
            }
            let data = if request.method == "codex/messageSend" {
                json!({"kind":"overloaded","stage":"discovery","message":"Request capacity exceeded","effects":{"resume":"notRequested","submission":"notDispatched"}})
            } else {
                json!({"kind":"overloaded","stage":"discovery","message":"Request capacity exceeded"})
            };
            return Err(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Request capacity exceeded","data":data}}),
            );
        }
        return Err(error(id, -32600, &failure.to_string()));
    }
    Ok(request)
}
fn dispatch(
    request: Request,
    identity: &ServiceIdentity,
    subscription: &EndpointSubscription,
    admission: &mut ControlAdmission,
) -> Value {
    let id = json!(request.id);
    let response = match request.method.as_str() {
        "control/initialize" => initialize(request.params, id, identity, admission),
        "endpoint/list" if request.params == json!({}) => match subscription.snapshot() {
            Ok(snapshot) => {
                json!({"jsonrpc":"2.0","id":id,"result":communication_protocol::EndpointInventory {
                    service_epoch: identity.service_epoch.clone(), sequence: snapshot.sequence, endpoints: snapshot.endpoints,
                }})
            }
            Err(_) => error(id, -32603, "Endpoint directory unavailable"),
        },
        "endpoint/list" => error(id, -32602, "Invalid parameters"),
        _ => error(id, -32601, "Method not found"),
    };
    admission.complete(&request.id);
    response
}
fn initialize(
    params: Value,
    id: Value,
    identity: &ServiceIdentity,
    admission: &mut ControlAdmission,
) -> Value {
    let Ok(params) =
        serde_json::from_value::<communication_protocol::ControlInitializationParams>(params)
    else {
        return error(id, -32602, "Invalid initialization parameters");
    };
    if params.version.major > 9_007_199_254_740_991 || params.version.minor > 9_007_199_254_740_991
    {
        return error(id, -32602, "Invalid initialization parameters");
    }
    if params.version.major != 1 || params.version.minor != 0 {
        return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Unsupported version","data":{"kind":"unsupportedVersion","stage":"initialize","message":"Control 1.0 required"}}});
    }
    admission.initialized();
    json!({"jsonrpc":"2.0","id":id,"result":communication_protocol::ControlInitializationResult {
        version: communication_protocol::ProtocolVersion { major: 1, minor: 0 },
        service_id: identity.service_id.clone(), service_epoch: identity.service_epoch.clone(),
        control_schema_digest: identity.schema_digest.clone(),
    }})
}

#[cfg(test)]
mod admission_error_tests {
    use super::*;

    #[test]
    fn saturated_message_admission_reports_that_no_native_effect_was_dispatched() {
        // Arrange: existing admitted work occupies every pending slot.
        let mut admission = ControlAdmission::default();
        admission.admit("init", "control/initialize").unwrap();
        admission.initialized();
        admission.complete("init");
        for number in 0..64 {
            admission
                .admit(&format!("pending-{number}"), "codex/sessionInspect")
                .unwrap();
        }
        // Act: this request is rejected by the real admission path before native dispatch.
        let result = admit_request(
            json!({"jsonrpc":"2.0","id":"rejected-message","method":"codex/messageSend","params":{}}),
            &mut admission,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("saturated admission accepted work"),
        };
        // Assert: clients can distinguish overload from unknown or partial native submission.
        assert_eq!(error["error"]["data"]["kind"], "overloaded");
        assert_eq!(
            error["error"]["data"]["effects"],
            json!({"resume":"notRequested","submission":"notDispatched"})
        );
        assert!(error["error"]["data"].get("clientUserMessageId").is_none());
        assert!(communication_protocol::control_error_is_valid(
            "codex/messageSend",
            &error
        ));
        let initialization = admit_request(
            json!({"jsonrpc":"2.0","id":"rejected-init","method":"control/initialize","params":{}}),
            &mut admission,
        );
        let initialization = match initialization {
            Err(error) => error,
            Ok(_) => panic!("saturated initialization accepted"),
        };
        assert!(communication_protocol::control_error_is_valid(
            "control/initialize",
            &initialization
        ));
    }
}
