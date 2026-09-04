"""Closed native wire protocol for the AW Provider entrypoint.

The native protocol is deliberately separate from the canonical AW Capability
Contracts. Each operation has its own request and completed-response model so
fields from one operation cannot be smuggled into another. Error and skipped
responses are also separate terminal shapes.

No response field carries matched content. Findings contain authored rule
identifiers and aggregate counts only; failure diagnostics are closed enums.
"""

from enum import StrEnum
from typing import Annotated, Literal, TypeAlias

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    StrictBool,
    StringConstraints,
    TypeAdapter,
    model_validator,
)

PROTOCOL_VERSION = 1
MAX_FINDINGS = 64
MAX_REASONS = 32
MAX_RULE_ID_BYTES = 64

RuleId = Annotated[
    str,
    StringConstraints(
        min_length=1,
        max_length=MAX_RULE_ID_BYTES,
        pattern=r"^[a-z0-9._-]+$",
    ),
]


class Operation(StrEnum):
    """Capability this invocation is expected to fulfil."""

    CONTENT_INSPECT = "content_inspect"
    CODE_INSPECT = "code_inspect"
    COMMAND_INSPECT = "command_inspect"


class Disposition(StrEnum):
    """Terminal outcome of one invocation."""

    COMPLETED = "completed"
    SKIPPED = "skipped"
    ERROR = "error"


class RequestLanguage(StrEnum):
    """Source language requested by the caller."""

    AUTO = "auto"
    BASH = "bash"
    PYTHON = "python"


class DetectedLanguage(StrEnum):
    """Rule set or rule sets that completed successfully."""

    BASH = "bash"
    PYTHON = "python"
    MIXED = "mixed"


class ContentSource(StrEnum):
    """Bounded source vocabulary understood by the PII scanner."""

    USER_INPUT = "user_input"
    TOOL_INPUT = "tool_input"
    TOOL_OUTPUT = "tool_output"
    MODEL_OUTPUT = "model_output"
    OBSERVABILITY = "observability"
    MANUAL = "manual"
    UNKNOWN = "unknown"


class InspectionVerdict(StrEnum):
    """Conclusion of a complete content or code inspection."""

    CLEAN = "clean"
    SUSPICIOUS = "suspicious"
    SENSITIVE = "sensitive"


class CommandVerdict(StrEnum):
    """Conclusion of a complete pending-command inspection."""

    ALLOW = "allow"
    WARN = "warn"
    DENY = "deny"


class FindingCategory(StrEnum):
    """Broad class of a finding."""

    SECRET = "secret"
    PERSONAL_DATA = "personal_data"
    CREDENTIAL = "credential"
    DANGEROUS_PATTERN = "dangerous_pattern"
    OBFUSCATION = "obfuscation"
    OTHER = "other"


class FindingSeverity(StrEnum):
    """Severity attached to a finding class."""

    INFO = "info"
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


class FindingConfidence(StrEnum):
    """Confidence attached to a finding class."""

    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"


class ProviderEngine(StrEnum):
    """Closed engine identity exposed across the native boundary."""

    PII_REGEX = "pii-regex"
    CODE_REGEX = "code-regex"


class ProviderErrorCode(StrEnum):
    """Content-free reason that an invocation could not produce a result."""

    SCANNER_FAILED = "scanner_failed"


class ProviderSkipReason(StrEnum):
    """Content-free reason that an admitted invocation was not applicable."""

    NOT_APPLICABLE = "not_applicable"


class NativeModel(BaseModel):
    """Closed base for every native protocol object."""

    model_config = ConfigDict(extra="forbid")


class ContentInspectRequest(NativeModel):
    """Native request for content inspection."""

    protocol_version: Literal[1]
    operation: Literal[Operation.CONTENT_INSPECT]
    content: str
    source: ContentSource
    include_low_confidence: StrictBool


class CodeInspectRequest(NativeModel):
    """Native request for code inspection."""

    protocol_version: Literal[1]
    operation: Literal[Operation.CODE_INSPECT]
    content: str
    language: RequestLanguage


class CommandInspectRequest(NativeModel):
    """Native request for pending-command inspection."""

    protocol_version: Literal[1]
    operation: Literal[Operation.COMMAND_INSPECT]
    content: str
    language: RequestLanguage


ProviderRequest: TypeAlias = Annotated[
    ContentInspectRequest | CodeInspectRequest | CommandInspectRequest,
    Field(discriminator="operation"),
]
PROVIDER_REQUEST_ADAPTER = TypeAdapter(ProviderRequest)


class ProviderFinding(NativeModel):
    """Content-free count of one finding class."""

    rule_id: RuleId
    category: FindingCategory
    severity: FindingSeverity
    confidence: FindingConfidence
    count: int = Field(ge=1)


def _validate_finding_summary(
    *,
    verdict: InspectionVerdict,
    findings: list[ProviderFinding],
    findings_total: int,
    truncated: bool,
) -> None:
    """Rejects an observation that cannot support its claimed verdict."""
    if findings_total != sum(finding.count for finding in findings):
        raise ValueError("findings_total must equal the sum of finding counts")
    if verdict is InspectionVerdict.CLEAN:
        if findings:
            raise ValueError("a clean verdict cannot carry findings")
        if truncated:
            raise ValueError("a clean verdict requires a complete scan")
    elif not findings:
        raise ValueError("a finding verdict requires at least one finding")


class ContentCompletedResponse(NativeModel):
    """Completed native content-inspection response."""

    protocol_version: Literal[1] = PROTOCOL_VERSION
    operation: Literal[Operation.CONTENT_INSPECT]
    disposition: Literal[Disposition.COMPLETED] = Disposition.COMPLETED
    findings_total: int = Field(ge=0)
    scanned_bytes: int = Field(ge=0)
    truncated: bool
    verdict: InspectionVerdict
    findings: list[ProviderFinding] = Field(max_length=MAX_FINDINGS)
    engine: Literal[ProviderEngine.PII_REGEX] = ProviderEngine.PII_REGEX

    @model_validator(mode="after")
    def validate_verdict_evidence(self) -> "ContentCompletedResponse":
        """Ensures a completed content verdict is supported and complete."""
        _validate_finding_summary(
            verdict=self.verdict,
            findings=self.findings,
            findings_total=self.findings_total,
            truncated=self.truncated,
        )
        return self


class CodeCompletedResponse(NativeModel):
    """Completed native code-inspection response."""

    protocol_version: Literal[1] = PROTOCOL_VERSION
    operation: Literal[Operation.CODE_INSPECT]
    disposition: Literal[Disposition.COMPLETED] = Disposition.COMPLETED
    findings_total: int = Field(ge=0)
    scanned_bytes: int = Field(ge=0)
    truncated: bool
    verdict: InspectionVerdict
    findings: list[ProviderFinding] = Field(max_length=MAX_FINDINGS)
    language_detected: DetectedLanguage
    engine: Literal[ProviderEngine.CODE_REGEX] = ProviderEngine.CODE_REGEX

    @model_validator(mode="after")
    def validate_verdict_evidence(self) -> "CodeCompletedResponse":
        """Ensures a completed code verdict is supported and complete."""
        _validate_finding_summary(
            verdict=self.verdict,
            findings=self.findings,
            findings_total=self.findings_total,
            truncated=self.truncated,
        )
        return self


class CommandCompletedResponse(NativeModel):
    """Completed native pending-command response."""

    protocol_version: Literal[1] = PROTOCOL_VERSION
    operation: Literal[Operation.COMMAND_INSPECT]
    disposition: Literal[Disposition.COMPLETED] = Disposition.COMPLETED
    findings_total: int = Field(ge=0)
    scanned_bytes: int = Field(ge=0)
    verdict: CommandVerdict
    findings: list[ProviderFinding] = Field(max_length=MAX_FINDINGS)
    reasons: list[RuleId] = Field(max_length=MAX_REASONS)
    language_detected: DetectedLanguage
    engine: Literal[ProviderEngine.CODE_REGEX] = ProviderEngine.CODE_REGEX

    @model_validator(mode="after")
    def validate_verdict_evidence(self) -> "CommandCompletedResponse":
        """Ensures a completed gate verdict is supported by its rationale."""
        if self.findings_total != sum(finding.count for finding in self.findings):
            raise ValueError("findings_total must equal the sum of finding counts")
        if self.verdict is CommandVerdict.ALLOW:
            if self.findings or self.reasons:
                raise ValueError("an allow verdict cannot carry findings or reasons")
        elif not self.findings or not self.reasons:
            raise ValueError("warn and deny verdicts require findings and reasons")
        if len(self.reasons) != len(set(self.reasons)):
            raise ValueError("command reasons must be unique")
        finding_rule_ids = {finding.rule_id for finding in self.findings}
        if not set(self.reasons) <= finding_rule_ids:
            raise ValueError("every command reason must identify an emitted finding")
        return self


class ProviderSkippedResponse(NativeModel):
    """Settled invocation that did not apply to the supplied input."""

    protocol_version: Literal[1] = PROTOCOL_VERSION
    operation: Operation
    disposition: Literal[Disposition.SKIPPED] = Disposition.SKIPPED
    findings_total: Literal[0] = 0
    scanned_bytes: Literal[0] = 0
    skip_reason: ProviderSkipReason


class ProviderErrorResponse(NativeModel):
    """Settled scanner failure with no unsupported security conclusion."""

    protocol_version: Literal[1] = PROTOCOL_VERSION
    operation: Operation
    disposition: Literal[Disposition.ERROR] = Disposition.ERROR
    findings_total: Literal[0] = 0
    scanned_bytes: int = Field(ge=0)
    error_code: ProviderErrorCode


ProviderResponse: TypeAlias = (
    ContentCompletedResponse
    | CodeCompletedResponse
    | CommandCompletedResponse
    | ProviderSkippedResponse
    | ProviderErrorResponse
)
PROVIDER_RESPONSE_ADAPTER = TypeAdapter(ProviderResponse)
