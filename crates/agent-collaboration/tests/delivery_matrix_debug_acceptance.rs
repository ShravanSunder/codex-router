#![allow(clippy::expect_used, clippy::indexing_slicing)]
//! Opt-in recipient-observed delivery proof on the owned automation debug Host.
#[path = "automation_live_support/proof_context.rs"]
#[allow(dead_code)]
mod proof_context;

use collaboration_client::board::{
    BoardCreateRequest, BoardId, Description, Identity, MessageId, MessagePostRequest,
    MessageReferences, MessageText as BoardMessageText, ParticipantRole, Placement,
    ProjectCreateRequest, ProjectId, ResourceName, ThreadCreateRequest, ThreadJoinRequest,
    ThreadListenDelivery, ThreadListenMode, ThreadListenRequest, ThreadListenSelection,
    TopicCreateRequest, TopicId,
};
use collaboration_client::protocol::{
    ConversationCreateOutcome, DestinationPreparation, ExecutionDestination,
    InstructionCreateParams, OperationId, RouterAccess, ScheduleCreateRequest, ScheduleDefinition,
    ScheduleEnableRequest, SchedulePrepareRequest, SessionRef, TimingRequest,
};
use collaboration_client::{ConversationClient, ConversationCreateActor, ConversationCreateInput};
#[path = "delivery_matrix/approval.rs"]
mod delivery_matrix_approval;
#[path = "delivery_matrix/support.rs"]
mod delivery_matrix_support;
use delivery_matrix_support::{
    ConfigHashGuard, PeerFixture, mcp_send, prepare_acp_target_fixture, prepare_provider_fixture,
};
#[path = "delivery_matrix/acp_target.rs"]
mod delivery_matrix_acp_target;
use proof_context::{ProofContext, ProofResult};
use serde_json::{Value, json};
use std::time::Duration;

const ACP_TARGET_EXPECTED_PROMPTS: usize = 9;

#[tokio::test]
#[ignore = "requires an owned isolated CLI Host with scripted provider fixture"]
async fn delivery_matrix_reaches_codex_and_fixture_claude_peer() -> ProofResult<()> {
    let config_guard = ConfigHashGuard::capture()?;
    let result = exercise_delivery_matrix(&config_guard).await;
    config_guard.verify()?;
    result
}

#[tokio::test]
#[ignore = "requires a fresh owned isolated CLI Host with scripted provider fixture"]
async fn codex_acp_cli_create_then_prompt_load_route() -> ProofResult<()> {
    let config_guard = ConfigHashGuard::capture()?;
    let mut proof = ProofContext::connect().await?;
    let creator = proof.start_thread("Codex ACP load-route creator").await?;
    let target = cli_codex_create_then_prompt_load_route(&proof, &creator).await?;
    proof.record(
        "codexAcpLoadRoute",
        json!({"target":target,"status":"passedLoadRoute"}),
    )?;
    config_guard.verify()?;
    proof.client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh owned isolated CLI Host with scripted provider fixture"]
async fn approval_notice_reaches_codex_recipient() -> ProofResult<()> {
    let config_guard = ConfigHashGuard::capture()?;
    let result = async {
        let mut proof = ProofContext::connect().await?;
        let sender = proof.start_thread("Approval fixture sender").await?;
        let approver = create_empty_conversation(&proof, &sender).await?;
        let marker = matrix_marker("approvalNotice", "codexFocused");
        let request_id = delivery_matrix_approval::deliver_approval_notice(
            &mut proof, &sender, &approver, &marker, None,
        )
        .await?;
        proof.record("approvalNoticeFocused", json!({"requestId":request_id}))?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    config_guard.verify()?;
    result
}

#[tokio::test]
#[ignore = "requires a fresh owned isolated CLI Host with scripted provider fixture"]
async fn sequential_approval_notices_reach_codex_and_claude_recipients() -> ProofResult<()> {
    let config_guard = ConfigHashGuard::capture()?;
    let result = async {
        let mut proof = ProofContext::connect().await?;
        let mut peer = PeerFixture::start(&proof).await?;
        let sender = proof.start_thread("Sequential approval sender").await?;
        let codex = create_empty_conversation(&proof, &sender).await?;
        delivery_matrix_approval::deliver_approval_notice(
            &mut proof,
            &sender,
            &codex,
            &matrix_marker("approvalNotice", "codexFocused"),
            None,
        )
        .await?;
        let peer_target = peer.target.clone();
        delivery_matrix_approval::deliver_approval_notice(
            &mut proof,
            &sender,
            &peer_target,
            &matrix_marker("approvalNotice", "claudeFocused"),
            Some(&mut peer),
        )
        .await?;
        peer.shutdown().await?;
        proof.client.close().await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    config_guard.verify()?;
    result
}

#[test]
#[ignore = "creates a fresh private direct child of /tmp for the isolated CLI Host"]
fn prepare_delivery_matrix_provider_fixture() -> ProofResult<()> {
    prepare_provider_fixture()
}

#[test]
#[ignore = "creates a fresh isolated Host root with two scripted ACP provider runtimes"]
fn prepare_delivery_matrix_acp_target_fixture() -> ProofResult<()> {
    prepare_acp_target_fixture()
}

#[tokio::test]
#[ignore = "requires a fresh isolated CLI Host with scripted ACP target and requester"]
async fn delivery_matrix_reaches_scripted_acp_target() -> ProofResult<()> {
    let config_guard = ConfigHashGuard::capture()?;
    let result = delivery_matrix_acp_target::exercise_acp_target_matrix(&config_guard).await;
    config_guard.verify()?;
    result
}

async fn exercise_delivery_matrix(config_guard: &ConfigHashGuard) -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let mut peer = PeerFixture::start(&proof).await?;
    let sender = proof.start_thread("Delivery matrix sender").await?;
    let mut records = Vec::new();

    // CLI create and prompt open separate ACP connections. This catches a
    // multi-route load rejection that message_send to codex-local cannot see.
    let codex_load = cli_codex_create_then_prompt_load_route(&proof, &sender).await?;
    records.push(
        json!({"producer":"conversationPromptCli","target":"codexAcp",
        "status":"pass","evidence":"separate-connection session/load routed past ACP -32602",
        "sessionId":codex_load.session_id}),
    );
    config_guard.verify()?;

    for producer in [DirectProducer::Cli, DirectProducer::Mcp] {
        let codex = proof.start_thread(producer.codex_role()).await?;
        let marker = producer.marker("codex");
        let prompt = format!("{marker}. Reply exactly RECEIVED_{marker}. Do not call tools.");
        match producer {
            DirectProducer::Cli => cli_send(&proof, &sender, &codex, &prompt).await?,
            DirectProducer::Mcp => mcp_send(&proof, &sender, &codex, &prompt).await?,
        }
        wait_for_input_marker(&mut proof, &codex, &marker, Duration::from_secs(120)).await?;
        records.push(json!({"producer":producer.name(),"target":"codex","status":"pass","evidence":"thread/read user input"}));
        config_guard.verify()?;

        let marker = producer.marker("claude");
        match producer {
            DirectProducer::Cli => cli_send(&proof, &sender, &peer.target, &marker).await?,
            DirectProducer::Mcp => mcp_send(&proof, &sender, &peer.target, &marker).await?,
        }
        peer.expect_marker(&marker).await?;
        records.push(json!({"producer":producer.name(),"target":"claudeCodePeer","status":"pass","evidence":"fixture peer socket user frame"}));
        config_guard.verify()?;
    }

    let codex = create_empty_conversation(&proof, &sender).await?;
    let marker = matrix_marker("wakeSend", "codex");
    wake_send(
        &proof,
        &sender,
        &codex,
        &format!("{marker}. Reply exactly RECEIVED_{marker}. Do not call tools."),
    )
    .await?;
    wait_for_input_marker(&mut proof, &codex, &marker, Duration::from_secs(120)).await?;
    records.push(json!({"producer":"wakeSend","target":"codex","status":"pass","evidence":"first fire plus thread/read user input"}));
    config_guard.verify()?;

    let marker = matrix_marker("wakeSend", "claude");
    wake_send(&proof, &sender, &peer.target, &marker).await?;
    peer.expect_marker(&marker).await?;
    records.push(json!({"producer":"wakeSend","target":"claudeCodePeer","status":"pass","evidence":"first fire plus fixture peer socket user frame"}));
    config_guard.verify()?;

    let codex = create_empty_conversation(&proof, &sender).await?;
    let marker = matrix_marker("scheduledRun", "codex");
    schedule_send(
        &mut proof,
        &codex,
        &format!("{marker}. Reply exactly RECEIVED_{marker}. Do not call tools."),
    )
    .await?;
    wait_for_input_marker(&mut proof, &codex, &marker, Duration::from_secs(120)).await?;
    records.push(json!({"producer":"scheduledRun","target":"codex","status":"pass","evidence":"scheduled user input in thread/read"}));
    config_guard.verify()?;

    records.push(json!({"producer":"scheduledRun","target":"materializedCodex","status":"pending","evidence":"isolated Codex home has no model auth to finish a first turn; default-run fake native integration test covers the route; live recipient proof follows a production restart"}));

    let marker = matrix_marker("scheduledRun", "claude");
    schedule_send(&mut proof, &peer.target, &marker).await?;
    peer.expect_marker(&marker).await?;
    records.push(json!({"producer":"scheduledRun","target":"claudeCodePeer","status":"pass","evidence":"scheduled input in fixture peer socket frame"}));
    config_guard.verify()?;

    let codex = proof.start_thread("Board Listen recipient").await?;
    let codex_marker = matrix_marker("boardListen", "codex");
    let peer_marker = matrix_marker("boardListen", "claude");
    board_listen_push(
        &mut proof,
        &sender,
        &codex,
        &peer.target,
        &codex_marker,
        &peer_marker,
    )
    .await?;
    wait_for_input_marker(&mut proof, &codex, &codex_marker, Duration::from_secs(400)).await?;
    records.push(json!({"producer":"boardListen","target":"codex","status":"pass","evidence":"Thread Listen batch marker in thread/read"}));
    config_guard.verify()?;
    peer.expect_marker_with_timeout(&peer_marker, Duration::from_secs(400))
        .await?;
    records.push(json!({"producer":"boardListen","target":"claudeCodePeer","status":"pass","evidence":"Thread Listen batch marker in fixture peer socket frame"}));
    config_guard.verify()?;

    let codex = create_empty_conversation(&proof, &sender).await?;
    let marker = matrix_marker("approvalNotice", "codex");
    let request_id = delivery_matrix_approval::deliver_approval_notice(
        &mut proof, &sender, &codex, &marker, None,
    )
    .await?;
    records.push(json!({"producer":"approvalNotice","target":"codex","status":"pass","evidence":"native approval request ID in thread/read","requestId":request_id}));
    config_guard.verify()?;

    let marker = matrix_marker("approvalNotice", "claude");
    let peer_target = peer.target.clone();
    let request_id = delivery_matrix_approval::deliver_approval_notice(
        &mut proof,
        &sender,
        &peer_target,
        &marker,
        Some(&mut peer),
    )
    .await?;
    records.push(json!({"producer":"approvalNotice","target":"claudeCodePeer","status":"pass","evidence":"native approval request ID in fixture peer socket frame","requestId":request_id}));
    config_guard.verify()?;

    proof.record("deliveryMatrix", json!({"cells":records}))?;
    peer.shutdown().await?;
    proof.client.close().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum DirectProducer {
    Cli,
    Mcp,
}

impl DirectProducer {
    fn name(self) -> &'static str {
        match self {
            Self::Cli => "cliMessageSend",
            Self::Mcp => "mcpMessageSend",
        }
    }
    fn codex_role(self) -> &'static str {
        match self {
            Self::Cli => "CLI recipient",
            Self::Mcp => "MCP recipient",
        }
    }
    fn marker(self, target: &str) -> String {
        matrix_marker(self.name(), target)
    }
}

fn matrix_marker(producer: &str, target: &str) -> String {
    format!(
        "DELIVERY_MATRIX_{producer}_{target}_{}",
        uuid::Uuid::now_v7()
    )
}

fn turns_contain_input(turns: &[Value], marker: &str) -> bool {
    turns.iter().any(|turn| {
        turn.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("userMessage")
                        && item
                            .get("content")
                            .and_then(Value::as_array)
                            .is_some_and(|content| {
                                content.iter().any(|part| {
                                    part.get("type").and_then(Value::as_str) == Some("text")
                                        && part.get("text").and_then(Value::as_str).is_some_and(
                                            |text| user_text_contains_marker(text, marker),
                                        )
                                })
                            })
                })
            })
    })
}

fn user_text_contains_marker(text: &str, marker: &str) -> bool {
    text.contains(marker)
        || text.split("\n\n").any(|segment| {
            serde_json::from_str::<Value>(segment)
                .ok()
                .is_some_and(|notice| {
                    notice.get("requestId").and_then(Value::as_str) == Some(marker)
                })
        })
}

#[test]
fn recipient_observer_finds_composite_approval_request_id_in_user_text() {
    let request_id = "[\"matrix-provider-codex\",91]";
    let notice = format!(
        "Agent communication\nSelf-declared sender: fixture\n\n{}",
        json!({"kind":"externalProviderPermission","requestId":request_id})
    );
    let turns = vec![json!({"items":[{"type":"userMessage","content":[
        {"type":"text","text":notice}
    ]}]})];
    assert!(turns_contain_input(&turns, request_id));
    let peer_message = format!(
        "{}\n\nFor replies, use Router's message_send as this Claude session.",
        json!({"kind":"externalProviderPermission","requestId":request_id})
    );
    assert!(user_text_contains_marker(&peer_message, request_id));
}

async fn create_empty_conversation(
    proof: &ProofContext,
    creator: &SessionRef,
) -> ProofResult<SessionRef> {
    let conversation =
        ConversationClient::connect(&proof.service_directory, &proof.endpoint).await?;
    let created = conversation
        .create(
            ConversationCreateInput {
                operation_id: OperationId::generate(),
                endpoint: proof.endpoint.clone(),
                working_directory: proof.workspace.clone(),
                access: RouterAccess::WorkspaceWrite,
                created_by: ConversationCreateActor::Session(creator.clone()),
                approver: Some(ConversationCreateActor::Session(creator.clone())),
                generation: None,
                model: Some("gpt-5.6-luna".to_owned()),
                mode: None,
                effort: Some("low".to_owned()),
                fork: None,
                root_message_id: None,
            },
            Duration::from_secs(60),
        )
        .await?;
    match created {
        ConversationCreateOutcome::Created { target, .. }
        | ConversationCreateOutcome::CreatedWithoutSettings { target, .. } => Ok(target),
        ConversationCreateOutcome::Pending { .. } => {
            Err("Empty conversation creation stayed pending".into())
        }
    }
}

async fn cli_codex_create_then_prompt_load_route(
    proof: &ProofContext,
    creator: &SessionRef,
) -> ProofResult<SessionRef> {
    let creator_json = serde_json::to_string(creator)?;
    let create = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "create",
            "--endpoint",
            "codex-local",
            "--model",
            "gpt-5.6-luna",
            "--effort",
            "low",
            "--access",
            "workspace-write",
            "--from",
            &creator_json,
            "--cwd",
        ])
        .arg(&proof.workspace)
        .arg("--service-directory")
        .arg(&proof.service_directory)
        .arg("--json")
        .output()
        .await?;
    if !create.status.success() {
        return Err("CLI conversation create failed in Codex ACP matrix cell".into());
    }
    let created = String::from_utf8(create.stdout)?
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line["kind"] == "created")
        .ok_or("CLI conversation create did not return a target")?;
    let target: SessionRef = serde_json::from_value(created["target"].clone())?;
    let target_json = serde_json::to_string(&target)?;
    let prompt = tokio::time::timeout(
        Duration::from_secs(35),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "conversation",
                "prompt",
                "--to",
                &target_json,
                "--from",
                &creator_json,
                "--cwd",
            ])
            .arg(&proof.workspace)
            .args([
                "--text",
                "Reply briefly without tools.",
                "--timeout-seconds",
                "15",
                "--service-directory",
            ])
            .arg(&proof.service_directory)
            .arg("--json")
            .output(),
    )
    .await??;
    let records = String::from_utf8(prompt.stdout)?
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    if records.is_empty() {
        return Err("CLI conversation prompt returned no result".into());
    }
    if !prompt.status.success()
        && records
            .last()
            .and_then(|record| record.pointer("/error/stage"))
            .and_then(Value::as_str)
            != Some("prompt")
    {
        return Err("Codex ACP conversation failed before prompt dispatch".into());
    }
    // The isolated matrix home has no model authentication. Prompt settlement
    // is a separate live cell; here only the route's load admission is proved.
    Ok(target)
}

async fn wait_for_input_marker(
    proof: &mut ProofContext,
    target: &SessionRef,
    marker: &str,
    timeout: Duration,
) -> ProofResult<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if turns_contain_input(&proof.turns(target).await?, marker) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("recipient thread/read did not contain marker {marker}").into());
        }
    }
}

async fn cli_send(
    proof: &ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
    text: &str,
) -> ProofResult<()> {
    let _receipt = cli_send_receipt(proof, sender, target, text).await?;
    Ok(())
}

async fn cli_send_receipt(
    proof: &ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
    text: &str,
) -> ProofResult<Value> {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "message",
            "send",
            "--from",
            &serde_json::to_string(sender)?,
            "--to",
            &serde_json::to_string(target)?,
            "--text",
            text,
            "--service-directory",
        ])
        .arg(&proof.service_directory)
        .arg("--json")
        .output()
        .await?;
    let receipt: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() || receipt["kind"] != "result" {
        return Err(format!(
            "CLI message send failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(receipt["result"]["record"].clone())
}

async fn wake_send(
    proof: &ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
    text: &str,
) -> ProofResult<()> {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "wake",
            "send",
            "--from",
            &serde_json::to_string(sender)?,
            "--to",
            &serde_json::to_string(target)?,
            "--text",
            text,
            "--after",
            "1s",
            "--wait-until-first-fire",
            "--operation-id",
            &uuid::Uuid::now_v7().to_string(),
            "--service-directory",
        ])
        .arg(&proof.service_directory)
        .arg("--json")
        .output()
        .await?;
    let result: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() || result["kind"] != "result" {
        return Err(format!(
            "wake send failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    if result.pointer("/result/record/firstFire/kind") != Some(&json!("wakeFired")) {
        return Err("wake send did not report a first firing".into());
    }
    Ok(())
}

async fn schedule_send(
    proof: &mut ProofContext,
    target: &SessionRef,
    text: &str,
) -> ProofResult<()> {
    let instruction = proof
        .client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: text.to_owned().try_into()?,
        })
        .await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRequest::After {
                    seconds: 5.try_into()?,
                },
                enabled: false,
                destination: ExecutionDestination::Unprepared,
                execution_timeout_seconds: Some(120.try_into()?),
                model: Some("gpt-5.6-luna".to_owned()),
                effort: Some("low".to_owned()),
            },
        })
        .await?;
    proof
        .client
        .prepare_schedule(SchedulePrepareRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
            destination: DestinationPreparation::Existing {
                target: target.clone(),
                cwd: proof.workspace.to_string_lossy().into_owned(),
            },
        })
        .await?;
    let enabled = proof
        .client
        .enable_schedule(ScheduleEnableRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id,
        })
        .await?;
    if enabled.next_due_at.is_none() {
        return Err("Scheduled delivery has no next due time".into());
    }
    Ok(())
}

async fn board_listen_push(
    proof: &mut ProofContext,
    sender: &SessionRef,
    codex: &SessionRef,
    peer: &SessionRef,
    codex_marker: &str,
    peer_marker: &str,
) -> ProofResult<()> {
    board_listen_push_targets(proof, sender, &[(codex, codex_marker), (peer, peer_marker)]).await
}

async fn board_listen_push_targets(
    proof: &mut ProofContext,
    sender: &SessionRef,
    targets: &[(&SessionRef, &str)],
) -> ProofResult<()> {
    let actor: Identity = serde_json::from_value(json!({"kind":"session","session":sender}))?;
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    proof
        .client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from(format!("Delivery matrix {}", project_id.as_str()))?,
            description: Description::try_from("Disposable recipient delivery proof".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    proof
        .client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from(format!("Delivery board {}", board_id.as_str()))?,
            description: Description::try_from("Thread Listen recipient proof".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    proof
        .client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Delivery".to_owned())?,
            description: Description::try_from("One marker burst".to_owned())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let root_id = MessageId::generate();
    proof
        .client
        .board_thread_create(ThreadCreateRequest {
            message_id: root_id.clone(),
            topic_id,
            actor: actor.clone(),
            acting_for: None,
            text: BoardMessageText::try_from("Delivery matrix root".to_owned())?,
            references: MessageReferences::try_from(Vec::new())?,
            role: Some(ParticipantRole::Orchestrator),
            watch: true,
        })
        .await?;
    for (target, _) in targets {
        let reader: Identity = serde_json::from_value(json!({"kind":"session","session":target}))?;
        proof
            .client
            .board_thread_join(ThreadJoinRequest {
                root_message_id: root_id.clone(),
                actor: reader.clone(),
                role: ParticipantRole::Participant,
                watch: true,
                replace: None,
                note: None,
            })
            .await?;
        proof
            .client
            .board_thread_listen(ThreadListenRequest {
                reader,
                selection: ThreadListenSelection::Roots {
                    root_message_ids: vec![root_id.clone()],
                },
                mode: ThreadListenMode::Once {
                    max_wait_seconds: 1500,
                },
                from_activity_sequence: None,
                acknowledge: false,
                delivery: ThreadListenDelivery::Session,
            })
            .await?;
    }
    for (_, marker) in targets {
        proof
            .client
            .board_message_post(MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root_id.clone(),
                },
                actor: actor.clone(),
                acting_for: None,
                text: BoardMessageText::try_from((*marker).to_owned())?,
                references: MessageReferences::try_from(Vec::new())?,
            })
            .await?;
    }
    Ok(())
}
