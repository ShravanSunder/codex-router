use super::*;

pub(super) async fn await_db_write_task_shutdown(task: Option<JoinHandle<()>>) {
    let Some(mut task) = task else {
        return;
    };
    let drain_grace = tokio::time::sleep(std::time::Duration::from_millis(
        DB_WRITE_SHUTDOWN_DRAIN_GRACE_MS,
    ));
    tokio::pin!(drain_grace);
    tokio::select! {
        _join_result = &mut task => {}
        () = &mut drain_grace => {
            task.abort();
            let _join_result = task.await;
        }
    }
}

pub(super) async fn run_session_affinity_scheduler(
    repository: Arc<dyn DbWriteRepository>,
    mut receiver: mpsc::Receiver<QueuedDbWriteCommand>,
    shutdown: CancellationToken,
    schedule_capacity: usize,
    last_queue_lag_event: Arc<Mutex<Option<QueueLagEvent>>>,
) {
    let mut schedules = HashMap::<SessionAffinityScheduleKey, ScheduledSessionAffinity>::new();
    loop {
        let next_deadline = schedules
            .values()
            .map(ScheduledSessionAffinity::next_deadline)
            .min()
            .unwrap_or_else(|| Instant::now() + std::time::Duration::from_secs(86_400));
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                receiver.close();
                while let Ok(command) = receiver.try_recv() {
                    process_session_affinity_trigger(
                        repository.as_ref(),
                        command,
                        &mut schedules,
                        schedule_capacity,
                        &last_queue_lag_event,
                    ).await;
                }
                flush_all_pending_session_affinities(
                    repository.as_ref(),
                    &mut schedules,
                    &last_queue_lag_event,
                ).await;
                return;
            }
            command = receiver.recv() => {
                let Some(command) = command else {
                    flush_all_pending_session_affinities(
                        repository.as_ref(),
                        &mut schedules,
                        &last_queue_lag_event,
                    ).await;
                    return;
                };
                process_session_affinity_trigger(
                    repository.as_ref(),
                    command,
                    &mut schedules,
                    schedule_capacity,
                    &last_queue_lag_event,
                ).await;
            }
            () = tokio::time::sleep_until(next_deadline) => {
                flush_due_session_affinities(
                    repository.as_ref(),
                    &mut schedules,
                    &last_queue_lag_event,
                ).await;
            }
        }
    }
}

pub(super) async fn process_session_affinity_trigger(
    repository: &dyn DbWriteRepository,
    command: QueuedDbWriteCommand,
    schedules: &mut HashMap<SessionAffinityScheduleKey, ScheduledSessionAffinity>,
    schedule_capacity: usize,
    last_queue_lag_event: &Arc<Mutex<Option<QueueLagEvent>>>,
) {
    let DbWriteCommand::SessionAccountAffinity { affinity, .. } = &command.command else {
        return;
    };
    let session_key = SessionAffinityScheduleKey {
        provider: affinity.provider(),
        session_id: affinity.session_id().to_owned(),
    };
    let account_id = affinity.account_id().cloned();
    let requested_at = command.enqueued_at;
    if let Some(schedule) = schedules.get_mut(&session_key) {
        schedule.latest_requested_at = requested_at;
        if schedule.account_id == account_id {
            schedule.pending_command = Some(command);
            schedule.first_pending_at.get_or_insert(requested_at);
            return;
        }
        schedule.account_id = account_id;
        schedule.pending_command = None;
        schedule.first_pending_at = None;
        persist_scheduled_session_affinity(repository, command, last_queue_lag_event).await;
        return;
    }

    if schedules.len() >= schedule_capacity
        && let Some(oldest_session_key) = schedules
            .iter()
            .min_by_key(|(_session_id, schedule)| schedule.latest_requested_at)
            .map(|(session_key, _schedule)| session_key.clone())
        && let Some(evicted_schedule) = schedules.remove(&oldest_session_key)
        && let Some(pending_command) = evicted_schedule.pending_command
    {
        persist_scheduled_session_affinity(repository, pending_command, last_queue_lag_event).await;
    }
    schedules.insert(
        session_key,
        ScheduledSessionAffinity {
            account_id,
            pending_command: None,
            first_pending_at: None,
            latest_requested_at: requested_at,
        },
    );
    persist_scheduled_session_affinity(repository, command, last_queue_lag_event).await;
}

pub(super) async fn flush_due_session_affinities(
    repository: &dyn DbWriteRepository,
    schedules: &mut HashMap<SessionAffinityScheduleKey, ScheduledSessionAffinity>,
    last_queue_lag_event: &Arc<Mutex<Option<QueueLagEvent>>>,
) {
    let now = Instant::now();
    let due_session_keys = schedules
        .iter()
        .filter(|(_session_id, schedule)| now >= schedule.next_deadline())
        .map(|(session_key, _schedule)| session_key.clone())
        .collect::<Vec<_>>();
    let mut pending_commands = Vec::new();
    for session_key in due_session_keys {
        let Some(schedule) = schedules.get_mut(&session_key) else {
            continue;
        };
        if let Some(command) = schedule.pending_command.take() {
            schedule.first_pending_at = None;
            pending_commands.push(command);
        } else {
            schedules.remove(&session_key);
        }
    }
    for command in pending_commands {
        persist_scheduled_session_affinity(repository, command, last_queue_lag_event).await;
    }
}

pub(super) async fn flush_all_pending_session_affinities(
    repository: &dyn DbWriteRepository,
    schedules: &mut HashMap<SessionAffinityScheduleKey, ScheduledSessionAffinity>,
    last_queue_lag_event: &Arc<Mutex<Option<QueueLagEvent>>>,
) {
    let pending_commands = schedules
        .drain()
        .filter_map(|(_session_key, schedule)| schedule.pending_command)
        .collect::<Vec<_>>();
    for command in pending_commands {
        persist_scheduled_session_affinity(repository, command, last_queue_lag_event).await;
    }
}

pub(super) async fn persist_scheduled_session_affinity(
    repository: &dyn DbWriteRepository,
    queued_command: QueuedDbWriteCommand,
    last_queue_lag_event: &Arc<Mutex<Option<QueueLagEvent>>>,
) {
    let queue_lag_event = emit_db_write_queue_lag_observed(
        queued_command.command.queue_name(),
        queued_command.command.route_band_label(),
        "processing",
        queue_lag_millis_since(queued_command.enqueued_at),
    );
    if let Ok(mut last_event) = last_queue_lag_event.lock() {
        *last_event = Some(queue_lag_event);
    }
    let command_result = handle_db_write_command(repository, queued_command.command).await;
    apply_db_write_command_result(command_result, &RouteBandQueueHealth::default(), 0, 0, 0);
}
