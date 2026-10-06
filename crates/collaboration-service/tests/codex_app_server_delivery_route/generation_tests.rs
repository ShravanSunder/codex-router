use super::*;

#[tokio::test]
async fn stale_strict_generation_stops_before_evidence_or_native_io()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let target: SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"thread-one"
    }))?;
    let current: CodexGeneration = serde_json::from_value(serde_json::json!({
        "serviceEpoch":service_id,"generation":1
    }))?;
    let stale: CodexGeneration = serde_json::from_value(serde_json::json!({
        "serviceEpoch":service_id,"generation":2
    }))?;
    let gate = NativeGenerationGate::default();
    gate.activate(
        current,
        std::path::PathBuf::from("/tmp/no-native-route.sock"),
        None,
    )?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        EndpointDirectory::new(service_id),
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: std::path::PathBuf::from("/tmp"),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let sink = Arc::new(CountingEvidenceSink(AtomicUsize::new(0)));
    let mut request = prepared_request(target, "hello", MessageDelivery::Auto)?;
    request.precondition = DeliveryPrecondition::EndpointGeneration { expected: stale };
    let receipt = route.deliver(request, sink.as_ref()).await?;
    if !matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: false,
            ..
        }
    ) || sink.0.load(Ordering::SeqCst) != 0
    {
        return Err("stale strict generation crossed the effect boundary".into());
    }
    Ok(())
}

#[tokio::test]
async fn prepared_push_reconciles_only_on_matching_codex_route_identity_and_line()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = tempfile::tempdir()?;
    let socket_path = root.path().join("native.sock");
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())?;
    let mismatched_push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458e".to_owned())?;
    let line: MessageText = format!(
        "✉️ sender · \"preview\" · router://{}/push/{}",
        String::from(service_id.clone()),
        push_id.as_str()
    )
    .try_into()?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"thread-one"
    }))?;

    // A prepared push must reject an ID mismatch before route effects or native I/O.
    let mismatched_request_route = CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        EndpointDirectory::new(service_id.clone()),
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate: NativeGenerationGate::default(),
            codex_home: root.path().to_path_buf(),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let mismatched_sink = RecordingEvidenceSink(Mutex::new(Vec::new()));
    let mismatched_request = PreparedDeliveryRequest {
        payload: PreparedPush {
            push_id: push_id.clone(),
            line: line.clone(),
            load_policy: LoadPolicy::LoadedOnly,
        },
        target: target.clone(),
        mode: MessageDelivery::Queue,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::try_from(mismatched_push_id.as_str().to_owned())?,
        attempt: agent_automation::AttemptId::generate(),
    };
    if !matches!(
        mismatched_request_route
            .deliver(mismatched_request, &mismatched_sink)
            .await,
        Err(DeliveryContractError::InvalidEvidence)
    ) || !mismatched_sink
        .0
        .lock()
        .map_err(|_| "evidence lock unavailable")?
        .is_empty()
    {
        return Err("prepared push id mismatch crossed the route boundary".into());
    }

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadTurnsList",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadQueueList",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?;
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(description.clone())?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, socket_path.clone(), Some(schemas))?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.path().to_path_buf(),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let sink = Arc::new(RecordingEvidenceSink(Mutex::new(Vec::new())));
    let observed = Arc::clone(&sink);
    let native_target = target.clone();
    let native_target_session_id = String::from(target.session_id.clone());
    let native_line = line.as_str().to_owned();
    let native_push_id = push_id.as_str().to_owned();
    let mismatched_native_push_id = mismatched_push_id.as_str().to_owned();
    let (finish_sender, finish_receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialize")??
                .to_text()?,
        )?;
        socket
            .send(Message::Text(
                json!({"id":initialize["id"],"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialized")??
                .to_text()?,
        )?;
        let read: Value =
            serde_json::from_str(socket.next().await.ok_or("missing read")??.to_text()?)?;
        let pre_effect_valid = {
            let pre_effect = observed.0.lock().map_err(|_| "evidence lock unavailable")?;
            pre_effect.len() == 1
                && matches!(&pre_effect[0], RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Dispatching && native.target.as_ref() == Some(&native_target) && native.client_user_message_id.as_deref() == Some(native_push_id.as_str()))
        };
        if !pre_effect_valid {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "dispatch evidence was not recorded before native I/O".into(),
            );
        }
        if read["method"] != "thread/read" {
            return Err("unexpected native inspection".into());
        }
        socket.send(Message::Text(json!({"id":read["id"],"result":{"thread":{"id":"thread-one","name":"🤖 Codex Main","status":{"type":"idle"}}}}).to_string().into())).await?;
        let queued: Value =
            serde_json::from_str(socket.next().await.ok_or("missing queue add")??.to_text()?)?;
        if queued["method"] != "thread/queue/add"
            || queued["params"]["threadId"] != native_target_session_id
            || queued["params"]["clientUserMessageId"] != native_push_id
            || queued["params"]["input"][0]["text"] != native_line
        {
            return Err("Codex route changed the prepared push id, target, or line".into());
        }
        socket
            .send(Message::Text(
                json!({"id":queued["id"],"result":{"queuedSubmission":{"id":"queued-one"}}})
                    .to_string()
                    .into(),
            ))
            .await?;

        drop(socket);
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing reconciliation initialize")??
                .to_text()?,
        )?;
        socket
            .send(Message::Text(
                json!({"id":initialize["id"],"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing reconciliation initialized")??
                .to_text()?,
        )?;
        let listing: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing queue list")??
                .to_text()?,
        )?;
        if listing["method"] != "thread/queue/list"
            || listing["params"]["threadId"] != native_target_session_id
        {
            return Err("Codex reconciliation used a different selected target".into());
        }
        socket
            .send(Message::Text(
                json!({
                    "id":listing["id"],
                    "result":{"data":[{
                        "id":"queued-one",
                        "clientUserMessageId":native_push_id,
                        "input":[{"type":"text","text":native_line}]
                    }],"nextCursor":null}
                })
                .to_string()
                .into(),
            ))
            .await?;
        drop(socket);

        // If a recorded ID disagrees with the push link, return a queue item that matches that
        // recorded ID and line. Correct reconciliation rejects it before native I/O.
        let connection = tokio::select! {
            accepted = listener.accept() => Some(accepted?),
            _ = finish_receiver => None,
        };
        if let Some((stream, _)) = connection {
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            let initialize: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing mismatched-id initialize")??
                    .to_text()?,
            )?;
            socket
                .send(Message::Text(
                    json!({"id":initialize["id"],"result":{}})
                        .to_string()
                        .into(),
                ))
                .await?;
            let _: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing mismatched-id initialized")??
                    .to_text()?,
            )?;
            let listing: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing mismatched-id queue list")??
                    .to_text()?,
            )?;
            if listing["method"] != "thread/queue/list"
                || listing["params"]["threadId"] != native_target_session_id
            {
                return Err("mismatched-ID reconciliation used an unexpected queue query".into());
            }
            socket
                .send(Message::Text(
                    json!({
                        "id":listing["id"],
                        "result":{"data":[{
                            "id":"queued-mismatched-id",
                            "clientUserMessageId":mismatched_native_push_id,
                            "input":[{"type":"text","text":native_line}]
                        }],"nextCursor":null}
                    })
                    .to_string()
                    .into(),
                ))
                .await?;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let delivery =
        SessionDeliveryRouter::new(vec![Arc::new(route) as Arc<dyn SessionDeliveryRoute>]);
    let receipt = tokio::time::timeout(
        Duration::from_secs(5),
        delivery.deliver(
            PreparedDeliveryRequest {
                payload: PreparedPush {
                    push_id: push_id.clone(),
                    line: line.clone(),
                    load_policy: LoadPolicy::LoadedOnly,
                },
                target: target.clone(),
                mode: MessageDelivery::Queue,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::try_from(push_id.as_str().to_owned())?,
                attempt: agent_automation::AttemptId::generate(),
            },
            sink.as_ref(),
        ),
    )
    .await??;
    if !matches!(receipt.outcome, DeliveryOutcome::Queued)
        || receipt.reachability != Some(collaboration_protocol::SessionReachability::CodexAppServer)
    {
        return Err("prepared push did not use the selected Codex app-server route".into());
    }
    if !matches!(receipt.client.as_ref(), Some(DeliveryClientReceipt::CodexAppServer(native)) if native.target == target && String::from(native.client_user_message_id.clone()) == push_id.as_str() && native.input_kind == MessageInputKind::Agent && native.representation == MessageRepresentation::DeclaredAgentText && matches!(&native.acceptance, collaboration_protocol::NativeSendAcceptance::QueueAccepted { submission_id } if String::from(submission_id.clone()) == "queued-one"))
    {
        return Err("prepared push did not retain its Codex route evidence and receipt".into());
    }
    let recorded = {
        let records = sink.0.lock().map_err(|_| "evidence lock unavailable")?;
        if records.len() != 2
            || !matches!(&records[1], RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Accepted && native.target.as_ref() == Some(&target) && native.client_user_message_id.as_deref() == Some(push_id.as_str()) && native.native_submission_id.as_deref() == Some("queued-one"))
        {
            return Err("prepared push did not retain its Codex route evidence and receipt".into());
        }
        records[1].clone()
    };

    let accepted = delivery
        .reconcile_attempt(AttemptReconciliationContext {
            target: target.clone(),
            prepared_push_id: push_id.clone(),
            mode: MessageDelivery::Queue,
            recorded: recorded.clone(),
        })
        .await?;
    let AttemptReconciliation::Accepted(reconciled) = accepted else {
        return Err("matching Codex push queue item was not reconciled".into());
    };
    if reconciled.outcome != DeliveryOutcome::Queued
        || !matches!(reconciled.client.as_ref(), Some(DeliveryClientReceipt::CodexAppServer(native)) if native.target == target && String::from(native.client_user_message_id.clone()) == push_id.as_str() && native.input_kind == MessageInputKind::Agent && native.representation == MessageRepresentation::DeclaredAgentText)
    {
        return Err("reconciliation changed the prepared push identity or target".into());
    }

    let mut mismatched_id_evidence = recorded.clone();
    mismatched_id_evidence
        .codex_app_server_mut()
        .ok_or("prepared push lost its Codex route evidence")?
        .client_user_message_id = Some(mismatched_push_id.as_str().to_owned());
    if !matches!(
        delivery
            .reconcile_attempt(AttemptReconciliationContext {
                target: target.clone(),
                prepared_push_id: push_id.clone(),
                mode: MessageDelivery::Queue,
                recorded: mismatched_id_evidence,
            })
            .await?,
        AttemptReconciliation::StillUnknown
    ) {
        return Err("push id mismatch reconciled as accepted".into());
    }

    let mismatched_target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"different-thread"
    }))?;
    if !matches!(
        delivery
            .reconcile_attempt(AttemptReconciliationContext {
                target: mismatched_target,
                prepared_push_id: push_id.clone(),
                mode: MessageDelivery::Queue,
                recorded: recorded.clone(),
            })
            .await?,
        AttemptReconciliation::StillUnknown
    ) {
        return Err("target mismatch reconciled as accepted".into());
    }

    let mismatched_route = RouteEffectEvidence::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
        session_id: PeerSessionReference::try_from("sidekick-session".to_owned())?,
        process_id: PeerProcessId::try_from(1_u32)?,
        write: PeerWriteEffect::Written,
    });
    if !matches!(
        delivery
            .reconcile_attempt(AttemptReconciliationContext {
                target: target.clone(),
                prepared_push_id: push_id.clone(),
                mode: MessageDelivery::Queue,
                recorded: mismatched_route,
            })
            .await?,
        AttemptReconciliation::StillUnknown
    ) {
        return Err("route mismatch reconciled as accepted".into());
    }

    let _ = finish_sender.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn retiring_during_thread_read_returns_retryable_unavailable()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = tempfile::tempdir()?;
    let socket_path = root.path().join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"thread-one"
    }))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadTurnsList",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?)?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, socket_path, Some(schemas))?;
    let route = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id,
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate: gate.clone(),
            codex_home: root.path().to_owned(),
        },
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    ));
    let (read_seen_tx, read_seen_rx) = tokio::sync::oneshot::channel();
    let (release_server_tx, release_server_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialize")??
                .to_text()?,
        )?;
        socket
            .send(Message::Text(
                json!({"id":initialize["id"],"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialized")??
                .to_text()?,
        )?;
        let read: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing thread/read")??
                .to_text()?,
        )?;
        if read.get("method").and_then(Value::as_str) != Some("thread/read") {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "expected thread/read before generation retirement".into(),
            );
        }
        read_seen_tx
            .send(())
            .map_err(|_| std::io::Error::other("delivery task left before thread/read"))?;
        release_server_rx
            .await
            .map_err(|_| std::io::Error::other("test did not release native fixture"))?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let mut request = prepared_request(target, "held batch", MessageDelivery::Auto)?;
    request.payload.load_policy = LoadPolicy::LoadedOnly;
    let sink = Arc::new(CountingEvidenceSink(AtomicUsize::new(0)));
    let delivery = tokio::spawn({
        let route = Arc::clone(&route);
        let sink = Arc::clone(&sink);
        async move { route.deliver(request, sink.as_ref()).await }
    });
    tokio::time::timeout(Duration::from_secs(5), read_seen_rx).await??;
    gate.retire()?;
    let receipt = tokio::time::timeout(Duration::from_secs(5), delivery).await???;
    release_server_tx
        .send(())
        .map_err(|_| std::io::Error::other("native fixture already exited"))?;
    server.await??;

    if !matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ref reason,
        } if reason == "unavailable"
    ) {
        return Err("retirement during thread/read was not retryable unavailable".into());
    }
    Ok(())
}
