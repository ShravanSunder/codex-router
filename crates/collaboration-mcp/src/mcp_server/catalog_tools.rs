//! The catalog tools whose request and result types come straight from the domain crates:
//! automation (wakes, schedules, runs, instructions, configuration and inspection) and the
//! board. Each decodes its typed request, calls one application operation and presents the
//! typed result or rejection.
use super::board_argument_classification::decode_board_arguments;
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

/// A board operation. Arguments that do not decode are refused with the board's typed
/// rejection naming the field, never with the decoder's message.
macro_rules! board_tool {
    ($router:expr, $name:literal, $request:ty => $result:ty, $operation:ident, $mutation:literal) => {
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
                    let request = match decode_board_arguments::<$request>(
                        $name,
                        context.arguments.unwrap_or_default(),
                    ) {
                        Ok(request) => request,
                        Err(rejection) => {
                            return Ok(CallToolResponse::Complete(domain_result::<(), _>(
                                Err(rejection),
                                $mutation,
                            )));
                        }
                    };
                    let board = context.service.application.board();
                    Ok(CallToolResponse::Complete(domain_result(
                        board.$operation(request).await,
                        $mutation,
                    )))
                })
            },
        ));
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
    board_tool!(router, "board_discovery_search", DiscoverySearchRequest => DiscoverySearchResult, discovery_search, false);
    board_tool!(router, "board_message_search", MessageSearchRequest => MessageSearchResult, message_search, false);
    board_tool!(router, "board_project_create", ProjectCreateRequest => ProjectCreateResult, project_create, true);
    board_tool!(router, "board_project_update", ProjectUpdateRequest => ProjectUpdateResult, project_update, true);
    board_tool!(router, "board_project_show", ProjectShowRequest => ProjectShowResult, project_show, false);
    board_tool!(router, "board_project_list", ProjectListRequest => ProjectListResult, project_list, false);
    board_tool!(router, "board_repository_attach", RepositoryAttachRequest => RepositoryAttachResult, repository_attach, true);
    board_tool!(router, "board_repository_detach", RepositoryDetachRequest => RepositoryDetachResult, repository_detach, true);
    board_tool!(router, "board_repository_list", RepositoryListRequest => RepositoryListResult, repository_list, false);
    board_tool!(router, "board_create", BoardCreateRequest => BoardCreateResult, board_create, true);
    board_tool!(router, "board_update", BoardUpdateRequest => BoardUpdateResult, board_update, true);
    board_tool!(router, "board_show", BoardShowRequest => BoardShowResult, board_show, false);
    board_tool!(router, "board_list", BoardListRequest => BoardListResult, board_list, false);
    board_tool!(router, "board_archive", BoardArchiveRequest => BoardArchiveResult, board_archive, true);
    board_tool!(router, "board_topic_create", TopicCreateRequest => TopicCreateResult, topic_create, true);
    board_tool!(router, "board_topic_update", TopicUpdateRequest => TopicUpdateResult, topic_update, true);
    board_tool!(router, "board_topic_list", TopicListRequest => TopicListResult, topic_list, false);
    board_tool!(router, "board_message_post", MessagePostRequest => MessagePostResult, message_post, true);
    board_tool!(router, "board_message_show", MessageShowRequest => MessageShowResult, message_show, false);
    board_tool!(router, "board_message_list", MessageListRequest => MessageListResult, message_list, false);
    board_tool!(router, "board_thread_show", ThreadShowRequest => ThreadShowResult, thread_show, false);
    board_tool!(router, "board_thread_resolve", ThreadResolveRequest => ThreadResolveResult, thread_resolve, true);
    board_tool!(router, "board_thread_unresolve", ThreadUnresolveRequest => ThreadUnresolveResult, thread_unresolve, true);
    board_tool!(router, "board_thread_watch", ThreadWatchRequest => ThreadWatchResult, thread_watch, true);
    board_tool!(router, "board_thread_unwatch", ThreadUnwatchRequest => ThreadUnwatchResult, thread_unwatch, true);
    board_tool!(router, "board_topic_watch", TopicWatchRequest => TopicWatchResult, topic_watch, true);
    board_tool!(router, "board_topic_unwatch", TopicWatchRequest => TopicWatchResult, topic_unwatch, true);
    board_tool!(router, "board_thread_list", ThreadListRequest => ThreadListResult, thread_list, false);
    board_tool!(router, "board_thread_create", ThreadCreateRequest => ThreadCreateResult, thread_create, true);
    board_tool!(router, "board_thread_join", ThreadJoinRequest => ThreadJoinResult, thread_join, true);
    board_tool!(router, "board_thread_leave", ThreadLeaveRequest => ThreadLeaveResult, thread_leave, true);
    board_tool!(router, "board_thread_participant_list", ThreadParticipantListRequest => ThreadParticipantListResult, thread_participant_list, false);
    board_tool!(router, "board_thread_subscribe", ThreadSubscribeRequest => ThreadSubscriptionView, thread_subscribe, true);
    board_tool!(router, "board_thread_unsubscribe", ThreadUnsubscribeRequest => ThreadSubscriptionView, thread_unsubscribe, true);
    board_tool!(router, "board_thread_subscriptions", ThreadSubscriptionsRequest => ThreadSubscriptionsResult, thread_subscriptions, false);
    board_tool!(router, "board_inbox_fetch", InboxFetchRequest => InboxFetchResult, inbox_fetch, true);
    board_tool!(router, "board_inbox_acknowledge", InboxAcknowledgeRequest => InboxAcknowledgeResult, inbox_acknowledge, true);
    board_tool!(router, "board_inbox_projects", InboxProjectsRequest => InboxProjectsResult, inbox_projects, false);
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyToolInput {}
