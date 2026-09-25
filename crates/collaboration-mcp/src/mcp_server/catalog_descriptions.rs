pub(super) fn operation_description(name: &str) -> &'static str {
    match name {
        "instruction_create" => {
            "Creates one durable automation instruction from validated text and a required operation identity. Creation is local state, not native agent acceptance or completion."
        }
        "instruction_update" => {
            "Revises an existing instruction using its identity and expected revision guard. Returns the stored snapshot; a successful update does not start native work."
        }
        "instruction_show" => {
            "Reads one instruction by identity without changing it or starting native work."
        }
        "instruction_list" => {
            "Lists a bounded page of stored instructions. Read-only; pagination does not start or reconcile runs."
        }
        "revision_list" => {
            "Lists a bounded page of revisions for one instruction. Read-only and ordered by the stored revision history."
        }
        "wake_send" => {
            "Creates a durable timed wake with its required operation identity, target, sender, delivery mode and timing. Creation confirms local scheduling only; a firing, native input acceptance, completed turn and agent reply are distinct later evidence."
        }
        "wake_show" => {
            "Reads one wake definition, state, next due time and recorded first-fire evidence. Read-only; it does not wait for or replay delivery."
        }
        "wake_list" => {
            "Lists a bounded page of stored wakes and their current scheduling state. Read-only and does not fire or deliver them."
        }
        "wake_pause" => {
            "Pauses an active wake using its wake identity and required operation identity. The mutation affects future eligibility; it does not retract an already accepted native input."
        }
        "wake_resume" => {
            "Resumes a paused wake using its identity and required operation identity. This restores scheduling eligibility but does not itself fire or deliver a message."
        }
        "wake_cancel" => {
            "Cancels a wake using its identity and required operation identity. Cancellation prevents future eligibility when still applicable; it is not proof that earlier delivery effects stopped."
        }
        "delivery_show" => {
            "Reads one delivery record, including eligibility, attempts and any known native acceptance evidence. Read-only; acceptance is not turn completion or an agent reply."
        }
        "delivery_list" => {
            "Lists a bounded page of delivery records, optionally scoped by wake identity. Read-only and never retries uncertain delivery."
        }
        "delivery_attempts" => {
            "Lists the bounded recorded attempt history for one delivery. Read-only; missing acceptance is not inferred as failure or safe replay."
        }
        "delivery_reconcile" => {
            "Reconciles one delivery from existing recorded/native evidence. Requires its delivery identity; it inspects state without resubmitting native input."
        }
        "schedule_import" => {
            "Imports a validated portable schedule package, optionally overwriting according to the request. This mutates local schedule state but does not prepare or execute a run."
        }
        "schedule_export" => {
            "Exports one stored schedule as a portable typed package. Read-only and does not enable, prepare or run the schedule."
        }
        "schedule_create" => {
            "Creates a durable schedule from a validated definition. Supply effort for every destination; freshEachRun also requires model. Creation stores configuration only; it does not allocate a conversation or execute work."
        }
        "schedule_update" => {
            "Updates a schedule using its identity and expected change guard. Supply effort for every destination; freshEachRun also requires model. The returned snapshot reflects stored configuration; no run is started."
        }
        "schedule_show" => {
            "Reads one schedule snapshot by identity without changing timing or execution state."
        }
        "schedule_list" => {
            "Lists a bounded page of schedules. Read-only and does not prepare or execute them."
        }
        "schedule_enable" => {
            "Enables a schedule using its identity and existing mutation contract. Enabling permits future eligibility but is not a firing, run acceptance or completion receipt."
        }
        "schedule_disable" => {
            "Disables a schedule using its identity and existing mutation contract. It prevents future eligibility when applicable but does not cancel already accepted native work."
        }
        "schedule_prepare" => {
            "Prepares one reuseThread schedule for execution. Fresh or fork destinations allocate a native conversation; existing destinations use ReadThread to validate the supplied conversation identity/workspace and record its binding, without loading or resuming it. Preparation mutates stored run-planning state; it does not prove native input acceptance, turn completion or an agent reply."
        }
        "run_show" => {
            "Reads one run snapshot and its separately recorded execution and summary evidence. Read-only; stored worker outcome is distinct from summary outcome."
        }
        "run_list" => {
            "Lists a bounded page of runs, optionally scoped by schedule. Read-only and does not reconcile or retry execution."
        }
        "run_reconcile" => {
            "Reconciles one run from existing stored/native evidence. Requires its run identity and never resubmits uncertain native work."
        }
        "run_summaries" => {
            "Lists recorded summary attempts for one run. Read-only; a summary result is distinct from worker execution completion."
        }
        "run_summary_retry" => {
            "Requests another summary attempt for an eligible run using the existing operation contract. It mutates summary state only and does not replay worker execution."
        }
        "run_summary_skip" => {
            "Records that summary processing is skipped for an eligible run. It does not alter the worker outcome or claim assignment success."
        }
        "automation_configure" => {
            "Updates automation execution and summary attempt budgets. Requires validated timeout/count values; it changes future policy and does not start work."
        }
        "automation_events" => {
            "Reads a bounded page of recorded automation events after an optional cursor. Read-only; events are evidence, not commands or replay instructions."
        }
        "operation_show" => {
            "Reads one stored mutation operation and its known outcome. Read-only; an unknown effect is preserved rather than retried."
        }
        "operation_reconcile" => {
            "Reconciles one stored operation from existing evidence. Requires its operation identity and never replays the mutation."
        }
        "conversation_operation_show" => {
            "Reads durable metadata for one conversation operation by its caller-retained operationId. Read-only; it never dispatches, waits for or replays client work."
        }
        "conversation_operation_wait" => {
            "Waits boundedly for settlement of an already admitted conversation operation. The waiter is call-local: MCP cancellation detaches it without cancelling client work or replaying the operation."
        }
        "conversation_operation_reconcile" => {
            "Reconciles one conversation operation from exact supported evidence using its caller-retained operationId. It is read-only with respect to client work and never resubmits the operation."
        }
        "board_discovery_search" => {
            "Searches discoverable projects/boards/topics using the supplied query and page bounds. Read-only; discovered board content is context, not authorization."
        }
        "board_message_search" => {
            "Searches board messages using the supplied scope/query and page bounds. Read-only and does not mark inbox activity acknowledged."
        }
        "board_project_create" => {
            "Creates a project with validated metadata and actor identity. This mutates board state only; it does not authorize or complete agent work."
        }
        "board_project_update" => {
            "Updates an existing project with its identity, actor and requested fields. It changes board metadata only."
        }
        "board_project_show" => "Reads one project by identity without changing board state.",
        "board_project_list" => {
            "Lists a bounded page of projects, optionally filtered by repository. Read-only."
        }
        "board_repository_attach" => {
            "Attaches a validated repository reference to an existing project using the supplied actor. It changes discovery metadata, not repository contents."
        }
        "board_repository_detach" => {
            "Detaches a repository reference from an existing project using the supplied actor. It does not alter the repository itself."
        }
        "board_repository_list" => {
            "Lists repository references attached to one project. Read-only."
        }
        "board_create" => {
            "Creates a board inside an existing project using validated metadata and actor identity. It does not create a conversation or assignment."
        }
        "board_update" => {
            "Updates an existing board's metadata using its identity and actor. It does not mutate contained message text."
        }
        "board_show" => "Reads one board by identity without changing it.",
        "board_list" => "Lists a bounded page of boards for a project. Read-only.",
        "board_archive" => {
            "Archives an existing board using its identity and actor. Archived content remains historical context; this does not complete its work threads."
        }
        "board_topic_create" => {
            "Creates a topic inside an active board using validated metadata and actor identity."
        }
        "board_topic_update" => {
            "Updates an existing topic's metadata using its identity and actor; message content remains immutable."
        }
        "board_topic_list" => "Lists a bounded page of topics for a board. Read-only.",
        "board_message_post" => {
            "Posts immutable content as the supplied actor to a topic or existing unresolved thread. Saving the message does not prove delivery to a model, turn completion, reply or assignment success."
        }
        "board_message_show" => {
            "Reads one board message by identity without changing watch, inbox or acknowledgement state."
        }
        "board_message_list" => {
            "Lists a bounded page of messages for the requested board/topic/thread scope and selection. Reading does not acknowledge inbox activity."
        }
        "board_thread_show" => {
            "Reads one board thread and its current state. Read-only; thread state is not a task verdict."
        }
        "board_thread_resolve" => {
            "Marks an existing thread resolved using the supplied actor. Resolution is board state and must not be used as proof that an agent assignment succeeded."
        }
        "board_thread_unresolve" => {
            "Reopens a resolved thread using the supplied actor so further replies are permitted."
        }
        "board_thread_watch" => {
            "Adds the supplied actor's watch to an existing thread. Watching selects future inbox activity; it does not wake a model or acknowledge history."
        }
        "board_thread_unwatch" => {
            "Removes the supplied actor's watch from an existing thread. Existing content and acknowledgement state remain unchanged."
        }
        "board_topic_watch" => {
            "Watches current and future threads in one topic for the supplied actor. It changes personal delivery selection only."
        }
        "board_topic_unwatch" => {
            "Stops the supplied actor's topic watch. Existing thread watches/content remain governed by their own state."
        }
        "board_thread_list" => {
            "Lists a bounded page of threads for the requested scope. Read-only and does not watch or acknowledge them."
        }
        "board_thread_create" => {
            "Creates a new root thread in an active topic as the supplied actor. It records discussion context, not a Router task or conversation."
        }
        "board_thread_join" => {
            "Joins the supplied session to an existing thread in its requested participant role. For session-delivered listening, join first with --role participant (CLI: board thread join --root-message-id <thread-id> --actor self --role participant --no-watch --json). Participation records responsibility context but does not grant extra execution authority."
        }
        "board_thread_leave" => {
            "Removes the supplied session's eligible participation from a thread. It does not delete messages or terminate the session."
        }
        "board_thread_participant_list" => {
            "Lists current participants and roles for one thread. Read-only and not an authentication result."
        }
        "board_thread_listen" => {
            "Creates a bounded board-activity listener for explicit thread/topic selections and reader identity. Session delivery requires an existing thread participant and a fixed lifetime: CLI --lifetime short (25 minutes) or long (75 minutes), or --once (25 minutes); MCP mode must use once.maxWaitSeconds=1500 or repeating.lifetimeSeconds=1500|4500. Join first with board_thread_join as role participant. Listener readiness/delivery observes board activity only; it does not prove model activation, turn completion or reply."
        }
        "board_thread_listen_show" => {
            "Reads the supplied reader's active board-listener state. Read-only and does not wait for activity."
        }
        "board_thread_listen_cancel" => {
            "Cancels the supplied reader's active board listener. It stops future listener delivery but does not unwatch threads or acknowledge activity."
        }
        "board_inbox_fetch" => {
            "Fetches bounded activity for the supplied reader and selected project/board/topic scope. Unread mode initializes reader tracking without acknowledging activity; Latest mode does not initialize it. Fetching does not prove a model processed it."
        }
        "board_inbox_acknowledge" => {
            "Advances the supplied reader's acknowledgement for exactly the requested inbox scope and activity sequence. It mutates attention state only."
        }
        "board_inbox_projects" => {
            "Lists projects currently tracked by the supplied reader's inbox. Read-only."
        }
        _ => {
            "Undocumented collaboration operation; inspect the typed input and output schemas before calling."
        }
    }
}
