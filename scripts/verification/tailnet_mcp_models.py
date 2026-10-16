"Typed local verification plans and response views; no runtime side effects."

import datetime as dt
import enum
import ipaddress
import typing as t
import urllib.parse
from pathlib import Path

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    JsonValue,
    field_validator,
    model_validator,
)

from scripts.verification.typed_observations import (
    JsonObject,
    ProcessIdentity,
)


class DriverModel(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="forbid", frozen=True)


class NativeReadiness(enum.StrEnum):
    STARTING = "starting"
    READY = "ready"


class StartingProviderIdentity(DriverModel):
    serviceId: str = Field(min_length=1)
    endpointId: t.Literal["claude-local", "cursor-local"]


class DisabledProviderState(DriverModel):
    state: t.Literal["unavailable"]
    observedAt: str
    reason: t.Literal["disabled in providers.json"]
    fix: str = Field(min_length=1)

    @field_validator("observedAt")
    @classmethod
    def valid_utc_observation(cls, value: str) -> str:
        if not value.endswith("Z"):
            raise ValueError("startup observation must use UTC Z")
        _ = dt.datetime.fromisoformat(value)
        return value


class StartingProviderEndpoint(DriverModel):
    endpoint: StartingProviderIdentity
    label: str = Field(min_length=1)
    availability: DisabledProviderState
    channels: list[JsonValue] = Field(max_length=0)


class NativeStartingInventory(DriverModel):
    serviceEpoch: str = Field(min_length=1)
    sequence: int = Field(strict=True, ge=0)
    endpoints: list[StartingProviderEndpoint] = Field(min_length=2, max_length=2)

    @model_validator(mode="after")
    def exact_disabled_providers(self) -> t.Self:
        if {entry.endpoint.endpointId for entry in self.endpoints} != {
            "claude-local",
            "cursor-local",
        }:
            raise ValueError(
                "starting inventory must name both disabled providers exactly once"
            )
        return self


type BoardUuid = t.Annotated[
    str,
    Field(
        pattern=r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    ),
]


class BoardMarker(DriverModel):
    topic_id: BoardUuid
    message_id: BoardUuid
    root_message_id: BoardUuid
    text: str = Field(min_length=1)


class HttpsEndpoint(DriverModel):
    url: str

    @model_validator(mode="after")
    def private_url(self) -> t.Self:
        parsed = urllib.parse.urlsplit(self.url)
        if (
            parsed.scheme != "https"
            or not (parsed.hostname or "").endswith(".ts.net")
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
            or parsed.path != "/mcp"
        ):
            raise ValueError("expected private ts.net HTTPS /mcp URL")
        if parsed.port == 0:
            raise ValueError("HTTPS port must be nonzero")
        return self

    @property
    def authority(self) -> str:
        parsed = urllib.parse.urlsplit(self.url)
        return f"{parsed.hostname}:{parsed.port or 443}"

    @property
    def origin(self) -> str:
        return f"https://{self.authority}"


class LoopbackEndpoint(DriverModel):
    url: str

    @model_validator(mode="after")
    def loopback_url(self) -> t.Self:
        parsed = urllib.parse.urlsplit(self.url)
        if (
            parsed.scheme != "http"
            or not ipaddress.ip_address(parsed.hostname or "").is_loopback
            or not parsed.port
            or parsed.path != "/mcp"
            or parsed.query
            or parsed.fragment
            or parsed.username
            or parsed.password
        ):
            raise ValueError("expected explicit loopback HTTP MCP endpoint")
        return self

    @property
    def origin(self) -> str:
        parsed = urllib.parse.urlsplit(self.url)
        return f"http://{parsed.netloc}"


class ProbeHeaders(DriverModel):
    origin: str | None = None
    host_authority: str | None = None


class ManifestMcpSelector(LoopbackEndpoint):
    transport: t.Literal["streamableHttp"]


class HostPlan(DriverModel):
    repository: Path
    source_head: str = Field(pattern=r"^[0-9a-f]{40}$")
    qualification: Path
    qualification_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    expected_version: str
    router_root: Path
    port: int
    https_port: int = 8443
    https: HttpsEndpoint
    profile_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    tailscale: Path
    deadline: dt.datetime
    protected: list[ProcessIdentity] = Field(min_length=1)

    @model_validator(mode="after")
    def valid_plan(self) -> t.Self:
        validate_ports(self.port, self.https_port)
        require_deadline(self.deadline)
        if (urllib.parse.urlsplit(self.https.url).port or 443) != self.https_port:
            raise ValueError("HTTPS URL and selected port differ")
        for path in [
            self.repository,
            self.qualification,
            self.router_root,
            self.tailscale,
        ]:
            if not path.is_absolute() or ".." in path.parts:
                raise ValueError("plan paths must be absolute without parent traversal")
        return self

    @property
    def evidence(self) -> Path:
        return self.router_root / "evidence"

    @property
    def service(self) -> Path:
        return self.router_root / "agent-communication"


class BoardPlan(DriverModel):
    destination_root: Path
    evidence: Path
    https: HttpsEndpoint
    destination_service_id: str = Field(min_length=1)
    destination_service_epoch: str = Field(min_length=1)
    actor: JsonObject
    marker: BoardMarker
    deadline: dt.datetime

    @model_validator(mode="after")
    def valid_plan(self) -> t.Self:
        require_deadline(self.deadline)
        if self.actor.get("kind") != "session":
            raise ValueError(
                "an actual Session actor is required, never a fabricated Human"
            )
        for path in [self.destination_root, self.evidence]:
            if not path.is_absolute() or ".." in path.parts:
                raise ValueError("absolute board paths are required")
        return self

    @property
    def service(self) -> Path:
        return self.destination_root / "agent-communication"


class QualifiedArtifact(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="ignore")
    name: str
    path: Path
    sha256: str
    features: list[str]
    version: str


class ArtifactQualification(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(extra="ignore")
    sourceHead: str
    artifacts: list[QualifiedArtifact]


class HttpReply(DriverModel):
    status: int
    body: bytes


def require_deadline(deadline: dt.datetime) -> None:
    if deadline.tzinfo is None or deadline <= dt.datetime.now(dt.UTC):
        raise ValueError("allocation deadline is absent, naive or expired")


def validate_ports(port: int, https_port: int) -> None:
    if (
        not 0 < port < 65536
        or not 0 < https_port < 65536
        or port in {19741, 18787, 43127}
        or port == https_port
    ):
        raise ValueError("unsafe or conflicting selected ports")


class DriverArguments(DriverModel):
    action: t.Literal["host", "peer", "readback"]
    plan: Path


class CleanupInventoryObserved(DriverModel):
    kind: t.Literal["observed"] = "observed"
    recorded: list[ProcessIdentity]
    matching: list[ProcessIdentity]


class CleanupInventoryUnavailable(DriverModel):
    kind: t.Literal["unavailable"] = "unavailable"
    recorded: list[ProcessIdentity]
    error: str


type CleanupInventory = CleanupInventoryObserved | CleanupInventoryUnavailable
