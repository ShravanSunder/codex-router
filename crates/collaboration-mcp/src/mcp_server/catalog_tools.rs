//! The catalog tools whose request and result types come straight from the domain crates:
//! automation (wakes, schedules, runs, instructions, configuration and inspection) and the
//! board. Each decodes its typed request, calls one application operation and presents the
//! typed result or rejection.
use super::*;
use collaboration_protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionView, ThreadSubscriptionsRequest,
    ThreadSubscriptionsResult, ThreadUnsubscribeRequest,
};

/// Registers one tool: `$name` takes `$request` and answers `$result`; `$call` runs with the
/// application bound to `$application` and the decoded request to `$input`.
macro_rules! application_tool {
    ($router:expr, $name:literal, $request:ty => $result:ty, |$application:ident, $input:ident| $call:expr) => {
        $router.add_route(ToolRoute::new_dyn(
            Tool::new(
                $name,
                operation_description($name),
                rmcp::handler::server::tool::schema_for_type::<$request>(),
            )
            .with_raw_output_schema(rmcp::handler::server::tool::schema_for_type::<
                McpToolOutput<$result>,
            >()),
            |context: ToolCallContext<'_, CollaborationMcpServer>| {
                Box::pin(async move {
                    let $input = match serde_json::from_value::<$request>(
                        serde_json::Value::Object(context.arguments.unwrap_or_default()),
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(CallToolResponse::Complete(validation_failure(
                                &error.to_string(),
                            )));
                        }
                    };
                    let $application = &context.service.application;
                    Ok(CallToolResponse::Complete($call))
                })
            },
        ));
    };
}

/// A domain operation on one application family: `mutation` decides the effect a rejection
/// the family does not type reports.
macro_rules! domain_tool {
    ($router:expr, $name:literal, $request:ty => $result:ty, $family:ident . $operation:ident, $mutation:literal) => {
        application_tool!($router, $name, $request => $result, |application, request| {
            domain_result(application.$family().$operation(request).await, $mutation)
        })
    };
    ($router:expr, $name:literal, $request:ty => $result:ty, $family:ident . $operation:ident(budget), $mutation:literal) => {
        application_tool!($router, $name, $request => $result, |application, request| {
            domain_result(
                application.$family().$operation(request, API_RESULT_BUDGET).await,
                $mutation,
            )
        })
    };
}

pub(super) fn register_automation_mutation_tools(router: &mut ToolRouter<CollaborationMcpServer>) {
    use collaboration_protocol::*;
    domain_tool!(router, "instruction_create", InstructionCreateParams => InstructionSnapshot, automation.instruction_create, true);
    domain_tool!(router, "instruction_update", InstructionUpdateParams => InstructionSnapshot, automation.instruction_update, true);
    domain_tool!(router, "instruction_show", InstructionShowParams => InstructionSnapshot, automation.instruction_show, false);
    domain_tool!(router, "wake_send", WakeSendRequest => WakeSnapshot, wakes.wake_send, true);
    domain_tool!(router, "wake_show", WakeShowRequest => WakeSnapshot, wakes.wake_show, false);
    domain_tool!(router, "wake_pause", WakeMutationRequest => WakeMutationResult, wakes.wake_pause, true);
    domain_tool!(router, "wake_resume", WakeMutationRequest => WakeMutationResult, wakes.wake_resume, true);
    domain_tool!(router, "wake_cancel", WakeMutationRequest => WakeMutationResult, wakes.wake_cancel, true);
    domain_tool!(router, "delivery_show", DeliveryShowRequest => DeliveryInspection, wakes.delivery_show, false);
    domain_tool!(router, "wake_list", AutomationPageRequest => AutomationPage<WakeSnapshot>, wakes.wake_list(budget), false);
    domain_tool!(router, "schedule_import", ScheduleImportRequest => ScheduleSnapshot, automation.schedule_import, true);
    domain_tool!(router, "schedule_export", ScheduleShowRequest => ScheduleExportResult, automation.schedule_export(budget), false);
    domain_tool!(router, "schedule_create", ScheduleCreateRequest => ScheduleSnapshot, automation.schedule_create, true);
    domain_tool!(router, "schedule_update", ScheduleUpdateRequest => ScheduleSnapshot, automation.schedule_update, true);
    domain_tool!(router, "schedule_show", ScheduleShowRequest => ScheduleSnapshot, automation.schedule_show, false);
    domain_tool!(router, "schedule_enable", ScheduleEnableRequest => ScheduleSnapshot, automation.schedule_enable, true);
    domain_tool!(router, "schedule_disable", ScheduleEnableRequest => ScheduleSnapshot, automation.schedule_disable, true);
    domain_tool!(router, "schedule_prepare", SchedulePrepareRequest => ScheduleSnapshot, automation.schedule_prepare, true);
    domain_tool!(router, "run_show", RunShowRequest => RunSnapshot, automation.run_show, false);
    domain_tool!(router, "run_summary_retry", RunRecoveryRequest => RunSnapshot, automation.run_summary_retry, true);
    domain_tool!(router, "run_summary_skip", RunRecoveryRequest => RunSnapshot, automation.run_summary_skip, true);
    domain_tool!(router, "automation_configure", AutomationConfigureRequest => AutomationConfiguration, automation.automation_configure, true);
}

pub(super) fn register_automation_inspection_tools(
    router: &mut ToolRouter<CollaborationMcpServer>,
) {
    use collaboration_protocol::*;
    domain_tool!(router, "run_reconcile", RunShowRequest => RunSnapshot, automation.run_reconcile(budget), false);
    domain_tool!(router, "delivery_reconcile", DeliveryShowRequest => DeliveryInspection, automation.delivery_reconcile(budget), false);
    domain_tool!(router, "operation_show", OperationShowRequest => OperationSnapshot, automation.operation_show(budget), false);
    domain_tool!(router, "operation_reconcile", OperationShowRequest => OperationSnapshot, automation.operation_reconcile(budget), false);
    domain_tool!(router, "automation_events", AutomationEventsRequest => AutomationEventsPage, automation.automation_events(budget), false);
    domain_tool!(router, "delivery_attempts", DeliveryAttemptsRequest => AttemptHistoryPage<AttemptInspection>, automation.delivery_attempts(budget), false);
    domain_tool!(router, "run_summaries", RunSummariesRequest => AttemptHistoryPage<SummaryInspection>, automation.run_summaries(budget), false);
    domain_tool!(router, "instruction_list", AutomationPageRequest => AutomationPage<InstructionSnapshot>, automation.instruction_list(budget), false);
    domain_tool!(router, "schedule_list", AutomationPageRequest => AutomationPage<ScheduleSnapshot>, automation.schedule_list(budget), false);
    domain_tool!(router, "run_list", RunListRequest => AutomationPage<RunSnapshot>, automation.run_list(budget), false);
    domain_tool!(router, "delivery_list", DeliveryListRequest => AutomationPage<DeliveryInspection>, automation.delivery_list(budget), false);
    domain_tool!(router, "revision_list", RevisionListRequest => AutomationPage<RevisionRecord>, automation.revision_list(budget), false);
}

pub(super) fn register_board_tools(router: &mut ToolRouter<CollaborationMcpServer>) {
    use message_board::*;
    domain_tool!(router, "board_discovery_search", DiscoverySearchRequest => DiscoverySearchResult, board.discovery_search, false);
    domain_tool!(router, "board_message_search", MessageSearchRequest => MessageSearchResult, board.message_search, false);
    domain_tool!(router, "board_project_create", ProjectCreateRequest => ProjectCreateResult, board.project_create, true);
    domain_tool!(router, "board_project_update", ProjectUpdateRequest => ProjectUpdateResult, board.project_update, true);
    domain_tool!(router, "board_project_show", ProjectShowRequest => ProjectShowResult, board.project_show, false);
    domain_tool!(router, "board_project_list", ProjectListRequest => ProjectListResult, board.project_list, false);
    domain_tool!(router, "board_repository_attach", RepositoryAttachRequest => RepositoryAttachResult, board.repository_attach, true);
    domain_tool!(router, "board_repository_detach", RepositoryDetachRequest => RepositoryDetachResult, board.repository_detach, true);
    domain_tool!(router, "board_repository_list", RepositoryListRequest => RepositoryListResult, board.repository_list, false);
    domain_tool!(router, "board_create", BoardCreateRequest => BoardCreateResult, board.board_create, true);
    domain_tool!(router, "board_update", BoardUpdateRequest => BoardUpdateResult, board.board_update, true);
    domain_tool!(router, "board_show", BoardShowRequest => BoardShowResult, board.board_show, false);
    domain_tool!(router, "board_list", BoardListRequest => BoardListResult, board.board_list, false);
    domain_tool!(router, "board_archive", BoardArchiveRequest => BoardArchiveResult, board.board_archive, true);
    domain_tool!(router, "board_topic_create", TopicCreateRequest => TopicCreateResult, board.topic_create, true);
    domain_tool!(router, "board_topic_update", TopicUpdateRequest => TopicUpdateResult, board.topic_update, true);
    domain_tool!(router, "board_topic_list", TopicListRequest => TopicListResult, board.topic_list, false);
    domain_tool!(router, "board_message_post", MessagePostRequest => MessagePostResult, board.message_post, true);
    domain_tool!(router, "board_message_show", MessageShowRequest => MessageShowResult, board.message_show, false);
    domain_tool!(router, "board_message_list", MessageListRequest => MessageListResult, board.message_list, false);
    domain_tool!(router, "board_thread_show", ThreadShowRequest => ThreadShowResult, board.thread_show, false);
    domain_tool!(router, "board_thread_resolve", ThreadResolveRequest => ThreadResolveResult, board.thread_resolve, true);
    domain_tool!(router, "board_thread_unresolve", ThreadUnresolveRequest => ThreadUnresolveResult, board.thread_unresolve, true);
    domain_tool!(router, "board_thread_watch", ThreadWatchRequest => ThreadWatchResult, board.thread_watch, true);
    domain_tool!(router, "board_thread_unwatch", ThreadUnwatchRequest => ThreadUnwatchResult, board.thread_unwatch, true);
    domain_tool!(router, "board_topic_watch", TopicWatchRequest => TopicWatchResult, board.topic_watch, true);
    domain_tool!(router, "board_topic_unwatch", TopicWatchRequest => TopicWatchResult, board.topic_unwatch, true);
    domain_tool!(router, "board_thread_list", ThreadListRequest => ThreadListResult, board.thread_list, false);
    domain_tool!(router, "board_thread_create", ThreadCreateRequest => ThreadCreateResult, board.thread_create, true);
    domain_tool!(router, "board_thread_join", ThreadJoinRequest => ThreadJoinResult, board.thread_join, true);
    domain_tool!(router, "board_thread_leave", ThreadLeaveRequest => ThreadLeaveResult, board.thread_leave, true);
    domain_tool!(router, "board_thread_participant_list", ThreadParticipantListRequest => ThreadParticipantListResult, board.thread_participant_list, false);
    domain_tool!(router, "board_thread_subscribe", ThreadSubscribeRequest => ThreadSubscriptionView, board.thread_subscribe, true);
    domain_tool!(router, "board_thread_unsubscribe", ThreadUnsubscribeRequest => ThreadSubscriptionView, board.thread_unsubscribe, true);
    domain_tool!(router, "board_thread_subscriptions", ThreadSubscriptionsRequest => ThreadSubscriptionsResult, board.thread_subscriptions, false);
    domain_tool!(router, "board_inbox_fetch", InboxFetchRequest => InboxFetchResult, board.inbox_fetch, true);
    domain_tool!(router, "board_inbox_acknowledge", InboxAcknowledgeRequest => InboxAcknowledgeResult, board.inbox_acknowledge, true);
    domain_tool!(router, "board_inbox_projects", InboxProjectsRequest => InboxProjectsResult, board.inbox_projects, false);
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyToolInput {}
