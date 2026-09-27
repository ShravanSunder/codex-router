//! ACP form elicitation is translated through the typed Question port.

#![allow(clippy::expect_used)]

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{
    ApprovalRequest, QuestionAnswerValue, QuestionField, QuestionRequest, QuestionResponse,
    SessionEvent,
};
use tokio_util::sync::CancellationToken;

const ELICITATION_FIXTURE: &str = r#"
import json,sys
mode,receipt=sys.argv[1:]
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
assert request['params']['clientCapabilities']['elicitation']=={'form':{}},request
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'elicitation-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
if mode=='unsupported':
    params={'mode':'url','sessionId':'fixture-session','message':'Open a link','url':'https://example.invalid'}
else:
    params={'mode':'form','sessionId':'fixture-session','message':'Choose settings',
        'requestedSchema':{'type':'object','properties':{
            'count':{'type':'number','title':'Count'},
            'active':{'type':'boolean','title':'Active'},
            'color':{'type':'string','title':'Color','oneOf':[
                {'const':'blue','title':'Blue'},{'const':'red','title':'Red'}]},
            'tags':{'type':'array','title':'Tags','items':{'type':'string','anyOf':[
                {'const':'a','title':'Alpha'},{'const':'b','title':'Beta'}]}}},
            'required':['count','color']}}
send({'jsonrpc':'2.0','id':'elicit-one','method':'elicitation/create','params':params})
response=read()
assert response['id']=='elicit-one',response
with open(receipt,'w') as output: json.dump(response,output)
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

struct QuestionPort {
    response: QuestionResponse,
    captured: Mutex<Option<QuestionRequest>>,
}
impl InteractionPort for QuestionPort {
    type Context = ();
    type OperationId = u64;
    fn operation_id(_context: &Self::Context) -> Self::OperationId {
        1
    }
    fn binding_retirement(_context: &Self::Context) -> CancellationToken {
        CancellationToken::new()
    }
    fn request_approval(
        &self,
        _context: Self::Context,
        _request: ApprovalRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async { ApprovalPortOutcome::Cancelled })
    }
    fn request_question(
        &self,
        _context: Self::Context,
        request: QuestionRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, QuestionResponse> {
        *self.captured.lock().expect("capture") = Some(request);
        let response = self.response.clone();
        Box::pin(async move { response })
    }
    fn record_refusal(
        &self,
        _context: Self::Context,
        _refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_all(
        &self,
        _context: Self::Context,
        _reason: &'static str,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct NoopEventSink;
impl SessionEventSink for NoopEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, _event: SessionEvent) -> Result<(), EventSinkClosed> {
        Ok(())
    }
}

async fn run_elicitation(
    mode: &str,
    response: QuestionResponse,
) -> (serde_json::Value, Option<QuestionRequest>) {
    let root = tempfile::tempdir().expect("fixture root");
    let receipt = root.path().join("elicitation-response.json");
    let port = Arc::new(QuestionPort {
        response,
        captured: Mutex::new(None),
    });
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                ELICITATION_FIXTURE.to_owned(),
                mode.to_owned(),
                receipt.to_string_lossy().into_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        port.clone(),
        Arc::new(NoopEventSink),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt_with_approval_context(session_id, "Elicit".to_owned(), ()),
    )
    .await
    .expect("elicitation exchange completes")
    .expect("prompt ends");
    client.shutdown().await;
    let receipt =
        serde_json::from_slice(&std::fs::read(receipt).expect("receipt")).expect("response JSON");
    let question = port.captured.lock().expect("capture").clone();
    (receipt, question)
}

/// Oracle: R18 keeps number, boolean, and titled option IDs intact; ACP gets
/// its exact form answer types after the Approver answers.
#[tokio::test]
async fn form_answer_round_trips_typed_fields_and_titled_choices() {
    let mut content = BTreeMap::new();
    content.insert("count".to_owned(), QuestionAnswerValue::Number(3.into()));
    content.insert("active".to_owned(), QuestionAnswerValue::Boolean(true));
    content.insert(
        "color".to_owned(),
        QuestionAnswerValue::SelectedOptions {
            selected_option_ids: vec!["blue".to_owned()],
        },
    );
    content.insert(
        "tags".to_owned(),
        QuestionAnswerValue::SelectedOptions {
            selected_option_ids: vec!["a".to_owned(), "b".to_owned()],
        },
    );
    let (wire, question) = run_elicitation("form", QuestionResponse::Answered { content }).await;
    assert_eq!(
        wire["result"],
        serde_json::json!({"action":"accept","content":{
        "count":3,"active":true,"color":"blue","tags":["a","b"]}})
    );
    let question = question.expect("question routed");
    let fields = question.fields.iter().collect::<Vec<_>>();
    assert!(fields.iter().any(|field| matches!(field, QuestionField::Number { field_id, required: true, .. } if field_id == "count")));
    assert!(fields.iter().any(
        |field| matches!(field, QuestionField::Boolean { field_id, .. } if field_id == "active")
    ));
    assert!(fields.iter().any(
        |field| matches!(field, QuestionField::SingleChoice { field_id, options, .. }
        if field_id == "color" && options[0].option_id == "blue" && options[0].label == "Blue")
    ));
    assert!(fields.iter().any(
        |field| matches!(field, QuestionField::MultiChoice { field_id, options, .. }
        if field_id == "tags" && options.len() == 2 && options[1].option_id == "b")
    ));
}

#[tokio::test]
async fn form_decline_cancel_and_unsupported_mode_stay_distinct() {
    let (declined, _) = run_elicitation("form", QuestionResponse::Declined).await;
    assert_eq!(declined["result"], serde_json::json!({"action":"decline"}));
    let (cancelled, _) = run_elicitation("form", QuestionResponse::Cancelled).await;
    assert_eq!(cancelled["result"], serde_json::json!({"action":"cancel"}));
    let (unsupported, question) = run_elicitation("unsupported", QuestionResponse::Cancelled).await;
    assert_eq!(unsupported["error"]["code"], -32602);
    assert!(question.is_none());
}
