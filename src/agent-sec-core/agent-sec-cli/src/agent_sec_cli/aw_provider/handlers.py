"""Side-effect-free adapters from local scanners to the AW native protocol.

These handlers bypass ``security_middleware.invoke`` because that path writes
security events and telemetry. The fixed PII detector list also avoids reading
custom rules from user configuration. Findings cross the Provider boundary
only as authored rule identifiers, classifications, and aggregate counts.
"""

from collections import Counter

from agent_sec_cli.aw_provider.protocol import (
    MAX_FINDINGS,
    MAX_REASONS,
    MAX_RULE_ID_BYTES,
    CodeCompletedResponse,
    CodeInspectRequest,
    CommandCompletedResponse,
    CommandInspectRequest,
    CommandVerdict,
    ContentCompletedResponse,
    ContentInspectRequest,
    DetectedLanguage,
    Disposition,
    FindingCategory,
    FindingConfidence,
    FindingSeverity,
    InspectionVerdict,
    Operation,
    ProviderErrorCode,
    ProviderErrorResponse,
    ProviderFinding,
    ProviderRequest,
    ProviderResponse,
    RequestLanguage,
)
from agent_sec_cli.code_scanner import scanner as code_scanner
from agent_sec_cli.code_scanner.models import Language as CodeLanguage
from agent_sec_cli.code_scanner.models import ScanResult as CodeScanResult
from agent_sec_cli.code_scanner.models import Verdict as CodeVerdict
from agent_sec_cli.pii_checker.detectors.regex import RegexPiiDetector
from agent_sec_cli.pii_checker.models import PiiScanResult
from agent_sec_cli.pii_checker.models import Verdict as PiiVerdict
from agent_sec_cli.pii_checker.scanner import PiiScanner

_RULE_ID_ALLOWED = set("abcdefghijklmnopqrstuvwxyz0123456789._-")

_PII_CATEGORIES = {
    "personal_data": FindingCategory.PERSONAL_DATA,
    "credential": FindingCategory.CREDENTIAL,
}

_SEVERITIES = {
    "warn": FindingSeverity.MEDIUM,
    "deny": FindingSeverity.HIGH,
}


def handle(request: ProviderRequest) -> ProviderResponse:
    """Dispatches one native request to its operation-specific scanner."""
    if isinstance(request, ContentInspectRequest):
        return _content_inspect(request)
    if isinstance(request, CodeInspectRequest):
        return _code_inspect(request)
    return _command_inspect(request)


def _content_inspect(request: ContentInspectRequest) -> ProviderResponse:
    """Reports secret and personal-data findings in model-visible content."""
    scanner = PiiScanner(detectors=[RegexPiiDetector()])
    try:
        result: PiiScanResult = scanner.scan(
            request.content,
            source=request.source.value,
            include_low_confidence=request.include_low_confidence,
            raw_evidence=False,
            redact_output=False,
        )
    except Exception:
        return _failed(request.operation, scanned_bytes=0)

    progress = _pii_progress(result)
    if progress is None:
        return _failed(request.operation, scanned_bytes=0)
    scanned_bytes, truncated = progress
    if not result.ok or result.verdict == PiiVerdict.ERROR.value:
        return _failed(request.operation, scanned_bytes=scanned_bytes)
    try:
        findings = _pii_findings(result)
        verdict = _inspection_verdict(result.verdict)
    except Exception:
        return _failed(request.operation, scanned_bytes=scanned_bytes)
    if truncated and verdict is InspectionVerdict.CLEAN:
        return _failed(request.operation, scanned_bytes=scanned_bytes)

    return ContentCompletedResponse(
        operation=request.operation,
        disposition=Disposition.COMPLETED,
        verdict=verdict,
        findings=findings,
        findings_total=sum(finding.count for finding in findings),
        scanned_bytes=scanned_bytes,
        truncated=truncated,
    )


def _code_inspect(request: CodeInspectRequest) -> ProviderResponse:
    """Reports dangerous constructs in code-bearing content."""
    scanned_bytes = _input_bytes(request.content)
    try:
        results, language_detected = _scan_code(request.content, request.language)
        findings = _code_findings(results)
    except Exception:
        return _failed(request.operation, scanned_bytes=0)
    if not _scan_completed(results):
        return _failed(request.operation, scanned_bytes=0)

    return CodeCompletedResponse(
        operation=request.operation,
        disposition=Disposition.COMPLETED,
        verdict=_inspection_verdict(_aggregate_code_verdict(results).value),
        findings=findings,
        findings_total=sum(finding.count for finding in findings),
        scanned_bytes=scanned_bytes,
        truncated=False,
        language_detected=language_detected,
    )


def _command_inspect(request: CommandInspectRequest) -> ProviderResponse:
    """Returns a gate verdict for a command that has not run yet."""
    scanned_bytes = _input_bytes(request.content)
    try:
        results, language_detected = _scan_code(request.content, request.language)
        findings = _code_findings(results)
    except Exception:
        return _failed(request.operation, scanned_bytes=0)
    if not _scan_completed(results):
        return _failed(request.operation, scanned_bytes=0)

    reasons = list(dict.fromkeys(finding.rule_id for finding in findings))
    if len(reasons) > MAX_REASONS:
        return _failed(request.operation, scanned_bytes=scanned_bytes)
    return CommandCompletedResponse(
        operation=request.operation,
        disposition=Disposition.COMPLETED,
        verdict=_command_verdict(_aggregate_code_verdict(results)),
        findings=findings,
        reasons=reasons,
        findings_total=sum(finding.count for finding in findings),
        scanned_bytes=scanned_bytes,
        language_detected=language_detected,
    )


def _scan_code(
    content: str,
    language: RequestLanguage,
) -> tuple[list[CodeScanResult], DetectedLanguage]:
    """Runs every rule set implied by the requested language.

    Automatic mode invokes both local regex engines. A completed ``mixed``
    result therefore means both scans completed; callers reject the whole
    result if either engine reports an error.
    """
    if language is RequestLanguage.AUTO:
        return (
            [
                code_scanner.scan(content, CodeLanguage.BASH, mode="regex"),
                code_scanner.scan(content, CodeLanguage.PYTHON, mode="regex"),
            ],
            DetectedLanguage.MIXED,
        )

    selected = (
        CodeLanguage.PYTHON if language is RequestLanguage.PYTHON else CodeLanguage.BASH
    )
    result = code_scanner.scan(content, selected, mode="regex")
    return [result], _detected_language(result.language)


def _scan_completed(results: list[CodeScanResult]) -> bool:
    """Returns whether every required scan produced a usable conclusion."""
    return bool(results) and all(
        result.ok and result.verdict is not CodeVerdict.ERROR for result in results
    )


def _aggregate_code_verdict(results: list[CodeScanResult]) -> CodeVerdict:
    """Returns the highest settled verdict across completed rule sets."""
    if any(result.verdict is CodeVerdict.DENY for result in results):
        return CodeVerdict.DENY
    if any(result.verdict is CodeVerdict.WARN for result in results):
        return CodeVerdict.WARN
    return CodeVerdict.PASS


def _failed(operation: Operation, *, scanned_bytes: int) -> ProviderErrorResponse:
    """Builds a settled failure with only a content-free reason code."""
    return ProviderErrorResponse(
        operation=operation,
        disposition=Disposition.ERROR,
        findings_total=0,
        scanned_bytes=scanned_bytes,
        error_code=ProviderErrorCode.SCANNER_FAILED,
    )


def _input_bytes(content: str) -> int:
    """Returns the exact UTF-8 size of the submitted scanner input."""
    return len(content.encode("utf-8"))


def _pii_progress(result: PiiScanResult) -> tuple[int, bool] | None:
    """Returns scanner-reported byte coverage when its metadata is usable."""
    scanned_bytes = result.summary.get("bytes_scanned")
    truncated = result.summary.get("truncated")
    if (
        not isinstance(scanned_bytes, int)
        or isinstance(scanned_bytes, bool)
        or scanned_bytes < 0
        or not isinstance(truncated, bool)
    ):
        return None
    return scanned_bytes, truncated


def _pii_findings(result: PiiScanResult) -> list[ProviderFinding]:
    """Aggregates PII findings into content-free per-class counts."""
    counter: Counter[
        tuple[str, FindingCategory, FindingSeverity, FindingConfidence]
    ] = Counter()
    for finding in result.findings:
        counter[
            (
                _rule_id(finding.type),
                _pii_category(finding.category),
                _severity(finding.severity),
                _confidence(finding.confidence),
            )
        ] += 1
    return _to_findings(counter)


def _code_findings(results: list[CodeScanResult]) -> list[ProviderFinding]:
    """Merges code findings without carrying matched source across the boundary."""
    matches: dict[
        tuple[str, FindingCategory, FindingSeverity, FindingConfidence],
        set[str],
    ] = {}
    for result in results:
        for finding in result.findings:
            key = (
                _rule_id(finding.rule_id),
                FindingCategory.DANGEROUS_PATTERN,
                _severity(finding.severity.value),
                FindingConfidence.HIGH,
            )
            evidence = set(finding.evidence) or {""}
            matches.setdefault(key, set()).update(evidence)

    counter = Counter({key: len(evidence) for key, evidence in matches.items()})
    return _to_findings(counter)


def _to_findings(
    counter: Counter[tuple[str, FindingCategory, FindingSeverity, FindingConfidence]],
) -> list[ProviderFinding]:
    """Returns deterministically ordered findings within the declared bound."""
    ordered = sorted(counter.items(), key=lambda item: item[0][0])
    if len(ordered) > MAX_FINDINGS:
        raise ValueError("finding classes exceed the native protocol bound")
    return [
        ProviderFinding(
            rule_id=rule_id,
            category=category,
            severity=severity,
            confidence=confidence,
            count=count,
        )
        for (rule_id, category, severity, confidence), count in ordered
    ]


def _rule_id(raw: str) -> str:
    """Normalizes a scanner rule name to the native rule-id alphabet."""
    normalized = "".join(
        character if character in _RULE_ID_ALLOWED else "." for character in raw.lower()
    )
    trimmed = normalized.strip(".")[:MAX_RULE_ID_BYTES]
    return trimmed or "unnamed"


def _pii_category(raw: str) -> FindingCategory:
    """Maps a PII category to its native class."""
    return _PII_CATEGORIES.get(raw, FindingCategory.OTHER)


def _severity(raw: str) -> FindingSeverity:
    """Maps a scanner severity to its native class."""
    return _SEVERITIES.get(raw, FindingSeverity.MEDIUM)


def _confidence(score: float) -> FindingConfidence:
    """Buckets a numeric confidence into the native confidence class."""
    if score < 0.5:
        return FindingConfidence.LOW
    if score < 0.8:
        return FindingConfidence.MEDIUM
    return FindingConfidence.HIGH


def _inspection_verdict(raw: str) -> InspectionVerdict:
    """Maps a completed scanner verdict to an inspection conclusion."""
    if raw == "deny":
        return InspectionVerdict.SENSITIVE
    if raw == "warn":
        return InspectionVerdict.SUSPICIOUS
    return InspectionVerdict.CLEAN


def _command_verdict(verdict: CodeVerdict) -> CommandVerdict:
    """Maps a completed scanner verdict to a Tool Call gate verdict."""
    if verdict is CodeVerdict.DENY:
        return CommandVerdict.DENY
    if verdict is CodeVerdict.WARN:
        return CommandVerdict.WARN
    return CommandVerdict.ALLOW


def _detected_language(language: CodeLanguage) -> DetectedLanguage:
    """Maps the scanner's resolved language to the native enum."""
    if language is CodeLanguage.PYTHON:
        return DetectedLanguage.PYTHON
    return DetectedLanguage.BASH
