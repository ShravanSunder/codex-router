"""Typed views of verification evidence; no product wire or SQLite ownership."""

import typing as t

from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter

type JsonObject = dict[str, JsonValue]
type IdentityRecord = dict[str, int | str]

_object_adapter = TypeAdapter(JsonObject)


def observed_object(value: object) -> JsonObject:
    return _object_adapter.validate_python(value)


def read_observed_json(value: str | bytes) -> JsonObject:
    return _object_adapter.validate_json(value)


class EvidenceView(BaseModel):
    # Partial read views retain unknown metadata without inventing defaults on disk.
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="allow", strict=True)


class EndpointView(EvidenceView):
    serviceId: str
    endpointId: str


class SessionView(EvidenceView):
    endpoint: EndpointView
    sessionId: str


class GenerationView(EvidenceView):
    serviceEpoch: str
    generation: int


class BindingDetail(EvidenceView):
    generation: GenerationView


class BindingView(EvidenceView):
    binding: BindingDetail


class OperationView(EvidenceView):
    target: SessionView
    binding: BindingView
    inputId: str | None = None
    stage: str | None = None
    effect: str | None = None
    reconciliation: str | None = None


class McpAddress(EvidenceView):
    url: str


class HostManifest(EvidenceView):
    serviceId: str
    serviceEpoch: str
    mcp: McpAddress
    serviceVersion: str
    machineLabel: str
    routerProxyEndpoint: str


class RouterStarted(EvidenceView):
    binarySHA256: str
    head: str


class HarnessClaims(EvidenceView):
    rows: list[tuple[str, str, str]]
    details: dict[str, str]
    scope: str


class ProcessIdentity(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(frozen=True)
    pid: int
    parent: int
    uid: int
    group: int
    birth: str
    executable: str

    def as_record(self) -> IdentityRecord:
        return {
            "pid": self.pid,
            "parent": self.parent,
            "uid": self.uid,
            "group": self.group,
            "birth": self.birth,
            "executable": self.executable,
        }


class QualifiedBinary(EvidenceView):
    target: str
    path: str
    sha256: str
    testGuardFeatures: list[str]


class BinaryQualification(EvidenceView):
    binaries: list[QualifiedBinary]


class AuthStatus(EvidenceView):
    kind: str


class ProviderCapabilities(EvidenceView):
    elicitation: bool
    authStatus: AuthStatus | None = None


class ProviderRecord(EvidenceView):
    target: SessionView
    state: str
    capabilities: ProviderCapabilities
    settingsCatalog: "SettingsCatalog"


class EndpointChannel(EvidenceView):
    kind: str
    bindingGeneration: int | None = None


class EndpointAvailability(EvidenceView):
    state: str


class ListedEndpoint(EvidenceView):
    endpoint: EndpointView
    availability: EndpointAvailability
    channels: list[EndpointChannel]


class EndpointInventory(EvidenceView):
    endpoints: list[ListedEndpoint]


class RpcError(EvidenceView):
    code: int


class ToolContent(EvidenceView):
    type: str
    text: str | None = None


class ToolResult(EvidenceView):
    isError: bool = False
    structuredContent: JsonObject | None = None
    content: list[ToolContent] = Field(default_factory=list)


class RpcEnvelope(EvidenceView):
    id: int
    result: ToolResult | None = None
    error: RpcError | None = None


class PublicOption(EvidenceView):
    effect: str
    scope: str
    optionId: str
    persistentTarget: JsonObject | None = None


class PublicInteraction(EvidenceView):
    requester: SessionView
    approver: JsonObject
    requestId: str
    options: list[PublicOption] = Field(default_factory=list)


class InteractionListing(EvidenceView):
    questions: list[PublicInteraction] = Field(default_factory=list)
    approvals: list[PublicInteraction] = Field(default_factory=list)


class RawSequencedEvent(EvidenceView):
    sequence: int
    event: JsonObject


class PublicObservation(EvidenceView):
    events: list[RawSequencedEvent]


class SettingsChoice(EvidenceView):
    value: str


class SettingsValue(EvidenceView):
    kind: str
    value: str | bool


class SettingsOption(EvidenceView):
    id: str
    category: str | None
    currentValue: SettingsValue
    choices: list[SettingsChoice]


class SettingsCatalog(EvidenceView):
    currentMode: str
    modes: list[SettingsChoice]
    configOptions: list[SettingsOption]


class CatalogRecord(EvidenceView):
    settingsCatalog: SettingsCatalog


class QuestionOption(EvidenceView):
    optionId: str
    label: str


class QuestionField(EvidenceView):
    kind: str
    fieldId: str
    required: bool
    options: list[QuestionOption] = Field(default_factory=list)


class QuestionView(EvidenceView):
    fields: list[QuestionField]


class ToolCallSubject(EvidenceView):
    toolCallId: str
    kind: str | None = None


class InteractionSubject(EvidenceView):
    toolCall: ToolCallSubject


class InteractionRequest(EvidenceView):
    requestId: str
    subject: InteractionSubject | None = None
    description: str | None = None


class RequestedInteraction(EvidenceView):
    kind: str
    request: InteractionRequest


class TurnOutcome(EvidenceView):
    kind: str
    stop_reason: str | None = None


class ToolStatus(EvidenceView):
    kind: t.Literal["toolCall"]
    status: str


class ToolItem(EvidenceView):
    itemId: str
    kind: ToolStatus


class TurnStartedEvent(EvidenceView):
    kind: t.Literal["turnStarted"]
    turnId: str
    inputId: str


class TurnEndedEvent(EvidenceView):
    kind: t.Literal["turnEnded"]
    turnId: str
    outcome: TurnOutcome


class InputAcceptedEvent(EvidenceView):
    kind: t.Literal["inputAccepted"]
    turnId: str
    inputId: str


class InteractionRequestedEvent(EvidenceView):
    kind: t.Literal["interactionRequested"]
    interaction: RequestedInteraction


class InteractionResolvedEvent(EvidenceView):
    kind: t.Literal["interactionResolved"]
    requestId: str


class ToolItemEvent(EvidenceView):
    kind: t.Literal["itemStarted", "itemUpdated"]
    item: ToolItem


type CallbackEvent = t.Annotated[
    TurnStartedEvent
    | TurnEndedEvent
    | InputAcceptedEvent
    | InteractionRequestedEvent
    | InteractionResolvedEvent
    | ToolItemEvent,
    Field(discriminator="kind"),
]


class SequencedEvent(EvidenceView):
    sequence: int
    event: CallbackEvent


class ObservationView(EvidenceView):
    target: SessionView
    generation: GenerationView
    attached: bool
    endReason: str


class CallbackCapture(EvidenceView):
    target: SessionView
    observation: ObservationView
    events: list[SequencedEvent]


class StoredInteraction(EvidenceView):
    kind: str
    state: JsonObject
    request: InteractionRequest | None = None


class StoredRow(EvidenceView):
    requestId: str
    createdAt: str
    record: StoredInteraction


class InteractionSnapshot(EvidenceView):
    database: str
    revision: int
    records: list[StoredRow]


class NativeContent(EvidenceView):
    type: str
    text: str | None = None


class NativeItem(EvidenceView):
    id: str
    type: str
    clientId: str | None = None
    content: list[NativeContent] = Field(default_factory=list)


class NativeTurn(EvidenceView):
    id: str
    items: list[NativeItem]


class NativeThread(EvidenceView):
    id: str
    turns: list[NativeTurn] = Field(default_factory=list)


class NativeExpectation(BaseModel):
    thread_id: str
    turn_id: str
    push_id: str


class NativeMatch(BaseModel):
    nativeId: str
    clientId: str
    turnId: str
    textSHA256: str


class NativeCorrelation(BaseModel):
    threadId: str
    matchingCount: int
    matches: list[NativeMatch]
    supported: bool


class SignalFailure(BaseModel):
    pid: int
    error: str


class SignalResult(BaseModel):
    sent: list[int] = Field(default_factory=list)
    errors: list[SignalFailure] = Field(default_factory=list)


class ProcessSignal(BaseModel):
    pid: int
    signal: int


class CleanupFailure(BaseModel):
    stage: str | None = None
    pid: int | None = None
    error: str


class CleanupResult(BaseModel):
    exit: int | None
    signals: list[ProcessSignal]
    errors: list[CleanupFailure]
    remaining: dict[int, IdentityRecord | str]
