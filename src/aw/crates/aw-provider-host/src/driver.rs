//! Bounded `exec-json/v1` process execution and declarative response mapping.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aw_contracts::common::Digest;
use aw_contracts::error::{ContractError, ErrorCategory};
use aw_contracts::provider::{
    CapabilityInvocation, ProviderDisposition, ProviderEvidenceRef, ProviderInvocationOutcome,
    ProviderInvocationResult, ProviderMeter, ProviderPayload, ProviderReceipt, ProviderScopeKind,
};
use sha2::{Digest as _, Sha256};
use wait_timeout::ChildExt;

use crate::process_group::{PlatformProcessGroup, ProcessGroupLifecycle};

use super::manifest::expand_environment_value;
use super::{
    canonical_json_v1_bytes, AdmittedCapability, AdmittedProvider, MissingValueAction,
    OutputValueSource, ProviderHostError, ProviderStderrDiagnostic, RequestValueSource, ScopeField,
};

const MAX_STDERR_CAPTURE_BYTES: usize = 16 * 1024;
const IO_COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(1);

pub(super) fn invoke(
    provider: &AdmittedProvider,
    capability: &AdmittedCapability,
    invocation: &CapabilityInvocation,
    state_root: Option<&Path>,
) -> Result<ProviderInvocationResult, ProviderHostError> {
    validate_invocation(provider, capability, invocation)?;
    validate_invocation_schema(capability, &invocation.input.body)?;
    let canonical_input = canonical_json_v1_bytes(&invocation.input.body).map_err(|error| {
        ProviderHostError::InvocationRejected(format!(
            "input body cannot be encoded as JSON: {error}"
        ))
    })?;
    let input_digest = sha256_digest(&canonical_input);
    if input_digest != invocation.input.digest {
        return Err(ProviderHostError::InvocationRejected(
            "input body digest does not match the admitted invocation".to_owned(),
        ));
    }
    let native_input = map_request(capability, invocation)?;
    validate_native_input_schema(capability, &native_input)?;
    let input = canonical_json_v1_bytes(&native_input).map_err(|error| {
        ProviderHostError::InvocationRejected(format!(
            "mapped Provider input cannot be encoded as JSON: {error}"
        ))
    })?;
    if input.len() > provider.limits.input_bytes {
        return Err(ProviderHostError::InvocationRejected(format!(
            "mapped input exceeds the {}-byte Provider limit",
            provider.limits.input_bytes
        )));
    }

    let now_ms = unix_time_ms()?;
    if invocation.deadline_at_ms <= now_ms {
        return Err(ProviderHostError::InvocationRejected(
            "invocation deadline has already expired".to_owned(),
        ));
    }
    if invocation.budget.wall_time_ms == 0 || invocation.budget.output_bytes == 0 {
        return Err(ProviderHostError::InvocationRejected(
            "invocation budget limits must be non-zero".to_owned(),
        ));
    }
    let remaining_ms = invocation.deadline_at_ms - now_ms;
    let timeout_ms = provider
        .limits
        .wall_time_ms
        .min(invocation.budget.wall_time_ms)
        .min(remaining_ms);
    let timeout = Duration::from_millis(timeout_ms);
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| ProviderHostError::InvocationRejected("deadline overflowed".to_owned()))?;
    let invocation_output_limit =
        usize::try_from(invocation.budget.output_bytes).map_err(|_| {
            ProviderHostError::InvocationRejected(
                "invocation output budget cannot be represented on this host".to_owned(),
            )
        })?;
    let output_limit = provider.limits.output_bytes.min(invocation_output_limit);
    let state_directory = if provider.requires_state_directory {
        let state_root = state_root.ok_or_else(|| {
            ProviderHostError::InvocationRejected(
                "Provider manifest requires an explicit state root".to_owned(),
            )
        })?;
        Some(validate_state_directory(
            state_root,
            provider.descriptor.provider_id.as_str(),
        )?)
    } else {
        None
    };

    let started_at_ms = unix_time_ms()?;
    let state_directory = match state_directory {
        Some(state_directory) => match materialize_state_directory(state_directory) {
            Ok(state_directory) => Some(state_directory),
            Err(error) => {
                return accepted_failure(provider, capability, invocation, started_at_ms, error);
            }
        },
        None => None,
    };
    if remaining_until(deadline).is_zero() {
        return accepted_failure(
            provider,
            capability,
            invocation,
            started_at_ms,
            deadline_error(provider, timeout),
        );
    }
    let process = match execute(
        provider,
        state_directory.as_deref(),
        input,
        deadline,
        timeout,
        output_limit,
    ) {
        Ok(process) => process,
        Err(error) => {
            return accepted_failure(provider, capability, invocation, started_at_ms, error);
        }
    };
    let response: serde_json::Value = match serde_json::from_slice(&process.stdout.bytes) {
        Ok(response) => response,
        Err(error) => {
            return accepted_failure(
                provider,
                capability,
                invocation,
                started_at_ms,
                ProviderHostError::InvalidResponse {
                    provider_id: provider.descriptor.provider_id.as_str().to_owned(),
                    reason: error.to_string(),
                    output_sha256: process.stdout.sha256,
                },
            );
        }
    };
    if let Err(reason) = schema_failure_reason(&capability.native_output_schema, &response) {
        return accepted_failure(
            provider,
            capability,
            invocation,
            started_at_ms,
            invalid_response(provider, &format!("native output {reason}"), &response),
        );
    }
    if let Err(reason) = validate_response_correlations(capability, &native_input, &response) {
        return accepted_failure(
            provider,
            capability,
            invocation,
            started_at_ms,
            invalid_response(provider, &reason, &response),
        );
    }
    if remaining_until(deadline).is_zero() {
        return accepted_failure(
            provider,
            capability,
            invocation,
            started_at_ms,
            deadline_error(provider, timeout),
        );
    }
    let mut result = match map_response(
        provider,
        capability,
        invocation,
        response,
        output_limit,
        started_at_ms,
        started_at_ms,
    ) {
        Ok(result) => result,
        Err(error) => {
            return accepted_failure(provider, capability, invocation, started_at_ms, error);
        }
    };
    if remaining_until(deadline).is_zero() {
        return accepted_failure(
            provider,
            capability,
            invocation,
            started_at_ms,
            deadline_error(provider, timeout),
        );
    }
    result.receipt.completed_at_ms = match unix_time_ms() {
        Ok(completed_at_ms) => completed_at_ms,
        Err(error) => {
            return accepted_failure(provider, capability, invocation, started_at_ms, error);
        }
    };
    Ok(result)
}

fn validate_response_correlations(
    capability: &AdmittedCapability,
    request: &serde_json::Value,
    response: &serde_json::Value,
) -> Result<(), String> {
    for correlation in &capability.codec.response_correlations {
        let expected = request
            .pointer(&correlation.request_pointer)
            .ok_or_else(|| {
                "response correlation request field is absent after mapping".to_owned()
            })?;
        let actual = response
            .pointer(&correlation.response_pointer)
            .ok_or_else(|| {
                "response correlation field is absent from the native response".to_owned()
            })?;
        if actual != expected {
            return Err("native response does not correlate with its request".to_owned());
        }
    }
    Ok(())
}

fn validate_invocation_schema(
    capability: &AdmittedCapability,
    input: &serde_json::Value,
) -> Result<(), ProviderHostError> {
    schema_failure_reason(&capability.canonical_input_schema, input).map_err(|reason| {
        ProviderHostError::InvocationRejected(format!("canonical input {reason}"))
    })
}

fn validate_native_input_schema(
    capability: &AdmittedCapability,
    input: &serde_json::Value,
) -> Result<(), ProviderHostError> {
    schema_failure_reason(&capability.native_input_schema, input).map_err(|reason| {
        ProviderHostError::InvocationRejected(format!("mapped native input {reason}"))
    })
}

fn schema_failure_reason(
    validator: &jsonschema::Validator,
    instance: &serde_json::Value,
) -> Result<(), String> {
    validator.validate(instance).map_err(|error| {
        let path = error.instance_path().as_str();
        if path.is_empty() {
            "does not satisfy its admitted schema at the document root".to_owned()
        } else {
            format!("does not satisfy its admitted schema at `{path}`")
        }
    })
}

fn validate_invocation(
    provider: &AdmittedProvider,
    capability: &AdmittedCapability,
    invocation: &CapabilityInvocation,
) -> Result<(), ProviderHostError> {
    if invocation.capability != capability.descriptor.capability {
        return Err(ProviderHostError::InvocationRejected(
            "Capability identity does not match the selected descriptor".to_owned(),
        ));
    }
    if invocation.input.schema != capability.descriptor.input_contract.schema {
        return Err(ProviderHostError::InvocationRejected(format!(
            "input schema does not match Capability `{}/v{}`",
            invocation.capability.id.as_str(),
            invocation.capability.version
        )));
    }
    if invocation.binding_id.is_some() {
        return Err(ProviderHostError::InvocationRejected(format!(
            "one-shot Provider `{}` does not accept a stateful binding",
            provider.descriptor.provider_id.as_str()
        )));
    }
    if invocation.scope.attempt_id.is_some() && invocation.scope.work_id.is_none() {
        return Err(ProviderHostError::InvocationRejected(
            "Attempt scope requires a Work identity".to_owned(),
        ));
    }
    if invocation.scope.turn_id.is_some() && invocation.scope.agent_session_id.is_none() {
        return Err(ProviderHostError::InvocationRejected(
            "Turn scope requires an Agent session identity".to_owned(),
        ));
    }
    if invocation.scope.tool_use_id.is_some() && invocation.scope.turn_id.is_none() {
        return Err(ProviderHostError::InvocationRejected(
            "Tool-call scope requires a Turn identity".to_owned(),
        ));
    }
    let effective_scope = most_specific_scope(invocation);
    if !capability.descriptor.scopes.contains(&effective_scope) {
        return Err(ProviderHostError::InvocationRejected(format!(
            "Capability does not admit the effective {effective_scope:?} scope"
        )));
    }
    Ok(())
}

fn most_specific_scope(invocation: &CapabilityInvocation) -> ProviderScopeKind {
    if invocation.scope.tool_use_id.is_some() {
        ProviderScopeKind::ToolCall
    } else if invocation.scope.turn_id.is_some() {
        ProviderScopeKind::Turn
    } else if invocation.scope.attempt_id.is_some() {
        ProviderScopeKind::Attempt
    } else if invocation.scope.work_id.is_some() {
        ProviderScopeKind::Work
    } else if invocation.scope.agent_session_id.is_some() {
        ProviderScopeKind::AgentSession
    } else {
        ProviderScopeKind::ExecutionContext
    }
}

fn map_request(
    capability: &AdmittedCapability,
    invocation: &CapabilityInvocation,
) -> Result<serde_json::Value, ProviderHostError> {
    let scope = serde_json::to_value(&invocation.scope).map_err(|error| {
        ProviderHostError::InvocationRejected(format!(
            "execution scope cannot be mapped to Provider input: {error}"
        ))
    })?;
    let mut native = serde_json::json!({});
    for mapping in &capability.codec.request_fields {
        let value = match &mapping.source {
            RequestValueSource::Constant(value) => Some(value.clone()),
            RequestValueSource::Input(pointer) => invocation.input.body.pointer(pointer).cloned(),
            RequestValueSource::Scope(field) => scope
                .pointer(scope_field_pointer(*field))
                .filter(|value| !value.is_null())
                .cloned(),
        };
        match value {
            Some(value) => {
                set_object_pointer(&mut native, &mapping.target, value).map_err(|reason| {
                    ProviderHostError::InvocationRejected(format!(
                        "request mapping target `{}` is invalid: {reason}",
                        mapping.target
                    ))
                })?
            }
            None if matches!(mapping.on_missing, MissingValueAction::Omit) => {}
            None => {
                return Err(ProviderHostError::InvocationRejected(format!(
                    "request mapping source for `{}` is absent",
                    mapping.target
                )));
            }
        }
    }
    Ok(native)
}

fn scope_field_pointer(field: ScopeField) -> &'static str {
    match field {
        ScopeField::TargetKind => "/target/kind",
        ScopeField::TargetAuthority => "/target/authority",
        ScopeField::TargetIdentifier => "/target/identifier",
        ScopeField::EnvironmentId => "/environment_id",
        ScopeField::ExecutionContextId => "/execution_context_id",
        ScopeField::ActorId => "/actor_id",
        ScopeField::AgentSessionId => "/agent_session_id",
        ScopeField::WorkId => "/work_id",
        ScopeField::AttemptId => "/attempt_id",
        ScopeField::TurnId => "/turn_id",
        ScopeField::ToolUseId => "/tool_use_id",
    }
}

fn set_object_pointer(
    root: &mut serde_json::Value,
    pointer: &str,
    value: serde_json::Value,
) -> Result<(), &'static str> {
    let mut segments = pointer.split('/').skip(1).peekable();
    let mut current = root;
    while let Some(encoded) = segments.next() {
        let key = decode_pointer_segment(encoded);
        let object = current
            .as_object_mut()
            .ok_or("target traverses a non-object value")?;
        if segments.peek().is_none() {
            object.insert(key, value);
            return Ok(());
        }
        current = object
            .entry(key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }
    Err("target must address a field below the root")
}

fn decode_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

struct ValidatedStateDirectory {
    root: PathBuf,
    candidate: PathBuf,
}

fn validate_state_directory(
    root: &Path,
    provider_id: &str,
) -> Result<ValidatedStateDirectory, ProviderHostError> {
    if !root.is_absolute() {
        return Err(ProviderHostError::PathNotAbsolute {
            label: "Provider state root",
            path: root.to_path_buf(),
        });
    }
    let metadata = fs::symlink_metadata(root).map_err(|source| ProviderHostError::Io {
        operation: "inspect Provider state root",
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ProviderHostError::SymlinkNotAllowed {
            label: "Provider state root",
            path: root.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(ProviderHostError::WrongFileType {
            label: "Provider state root",
            expected: "directory",
            path: root.to_path_buf(),
        });
    }
    let root = fs::canonicalize(root).map_err(|source| ProviderHostError::Io {
        operation: "canonicalize Provider state root",
        path: root.to_path_buf(),
        source,
    })?;
    let candidate = root.join(provider_id);
    match fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ProviderHostError::SymlinkNotAllowed {
                    label: "Provider state directory",
                    path: candidate,
                });
            }
            if !metadata.is_dir() {
                return Err(ProviderHostError::WrongFileType {
                    label: "Provider state directory",
                    expected: "directory",
                    path: candidate,
                });
            }
            let canonical =
                fs::canonicalize(&candidate).map_err(|source| ProviderHostError::Io {
                    operation: "canonicalize Provider state directory",
                    path: candidate.clone(),
                    source,
                })?;
            if canonical != candidate || !canonical.starts_with(&root) {
                return Err(ProviderHostError::InvocationRejected(
                    "Provider state directory escaped its authorized root".to_owned(),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ProviderHostError::Io {
                operation: "inspect Provider state directory",
                path: candidate,
                source,
            });
        }
    }
    Ok(ValidatedStateDirectory { root, candidate })
}

fn materialize_state_directory(
    state: ValidatedStateDirectory,
) -> Result<PathBuf, ProviderHostError> {
    let root = state.root;
    let candidate = state.candidate;
    match fs::symlink_metadata(&candidate) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(ProviderHostError::WrongFileType {
                label: "Provider state directory",
                expected: "directory",
                path: candidate,
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_directory(&candidate)?;
        }
        Err(source) => {
            return Err(ProviderHostError::Io {
                operation: "inspect Provider state directory",
                path: candidate,
                source,
            });
        }
    }
    let state_directory = fs::canonicalize(&candidate).map_err(|source| ProviderHostError::Io {
        operation: "canonicalize Provider state directory",
        path: candidate.clone(),
        source,
    })?;
    if state_directory != candidate || !state_directory.starts_with(&root) {
        return Err(ProviderHostError::InvocationRejected(
            "Provider state directory escaped its authorized root".to_owned(),
        ));
    }
    Ok(state_directory)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), ProviderHostError> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|source| ProviderHostError::Io {
            operation: "create Provider state directory",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> Result<(), ProviderHostError> {
    fs::create_dir(path).map_err(|source| ProviderHostError::Io {
        operation: "create Provider state directory",
        path: path.to_path_buf(),
        source,
    })
}

fn execute(
    provider: &AdmittedProvider,
    state_directory: Option<&Path>,
    input: Vec<u8>,
    deadline: Instant,
    timeout: Duration,
    output_limit: usize,
) -> Result<ProcessOutput, ProviderHostError> {
    if remaining_until(deadline).is_zero() {
        return Err(deadline_error(provider, timeout));
    }
    let mut command = Command::new(&provider.executable);
    command
        .args(&provider.args)
        .current_dir(&provider.working_directory)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(state_directory) = state_directory {
        command.env("AW_PROVIDER_STATE_DIR", state_directory);
    }
    for (name, value) in &provider.environment {
        let value = match state_directory {
            Some(state_directory) => expand_environment_value(value, state_directory),
            None if value.contains("{provider_state_dir}") => {
                return Err(ProviderHostError::Process(
                    "Provider environment requires an unavailable state directory".to_owned(),
                ));
            }
            None => value.clone(),
        };
        command.env(name, value);
    }
    let process_groups = PlatformProcessGroup;
    process_groups.configure(&mut command);
    let mut child = command.spawn().map_err(|error| {
        ProviderHostError::Process(format!(
            "failed to spawn admitted executable {}: {error}",
            provider.executable.display()
        ))
    })?;
    let process_group = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderHostError::Process("Provider stdin was not piped".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderHostError::Process("Provider stdout was not piped".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProviderHostError::Process("Provider stderr was not piped".to_owned()))?;
    let writer = spawn_writer(stdin, input).map_err(|error| {
        let _ = child.kill();
        let _ = child.wait();
        ProviderHostError::Process(format!("failed to start Provider stdin writer: {error}"))
    })?;
    let stdout_reader =
        spawn_capture("aw-provider-stdout", stdout, output_limit).map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            ProviderHostError::Process(format!("failed to start Provider stdout reader: {error}"))
        })?;
    let stderr_reader = spawn_capture("aw-provider-stderr", stderr, MAX_STDERR_CAPTURE_BYTES)
        .map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            ProviderHostError::Process(format!("failed to start Provider stderr reader: {error}"))
        })?;

    let status = child
        .wait_timeout(remaining_until(deadline))
        .map_err(|error| {
            ProviderHostError::Process(format!("failed to wait for Provider: {error}"))
        })?;
    let status = match status {
        Some(status) => status,
        None => {
            kill_and_reap(&mut child, process_group, &process_groups)?;
            drop(writer);
            drop(stdout_reader);
            return Err(timeout_error(provider, timeout, stderr_reader));
        }
    };
    if !wait_for_io_until(deadline, &writer, &stdout_reader, &stderr_reader) {
        kill_process_group(process_group, &process_groups)?;
        drop(writer);
        drop(stdout_reader);
        return Err(timeout_error(provider, timeout, stderr_reader));
    }
    let write_result = join_writer(writer)?;
    let stdout = join_capture(stdout_reader, "stdout")?;
    let stderr = join_capture(stderr_reader, "stderr")?;
    let stderr_diagnostic = stderr.diagnostic();
    if !status.success() {
        return Err(ProviderHostError::NonZeroExit {
            provider_id: provider.descriptor.provider_id.as_str().to_owned(),
            status: exit_status(status),
            stderr: stderr_diagnostic,
        });
    }
    write_result.map_err(|error| {
        ProviderHostError::Process(format!("failed to send Provider input: {error}"))
    })?;
    if stdout.total_bytes > output_limit as u64 {
        return Err(ProviderHostError::OutputTooLarge {
            provider_id: provider.descriptor.provider_id.as_str().to_owned(),
            limit: output_limit,
            output_sha256: stdout.sha256,
        });
    }
    Ok(ProcessOutput { stdout })
}

fn kill_and_reap(
    child: &mut Child,
    process_group: u32,
    lifecycle: &PlatformProcessGroup,
) -> Result<ExitStatus, ProviderHostError> {
    if lifecycle.kill(process_group).is_err() {
        let _ = child.kill();
    }
    child.wait().map_err(|error| {
        ProviderHostError::Process(format!("failed to reap timed-out Provider: {error}"))
    })
}

fn kill_process_group(
    process_group: u32,
    lifecycle: &PlatformProcessGroup,
) -> Result<(), ProviderHostError> {
    lifecycle.kill(process_group).map_err(|error| {
        ProviderHostError::Process(format!(
            "failed to kill timed-out Provider process group {process_group}: {error}"
        ))
    })
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn wait_for_io_until(
    deadline: Instant,
    writer: &JoinHandle<io::Result<()>>,
    stdout: &JoinHandle<io::Result<CapturedBytes>>,
    stderr: &JoinHandle<io::Result<CapturedBytes>>,
) -> bool {
    loop {
        if writer.is_finished() && stdout.is_finished() && stderr.is_finished() {
            return true;
        }
        let remaining = remaining_until(deadline);
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(remaining.min(IO_COMPLETION_POLL_INTERVAL));
    }
}

fn timeout_error(
    provider: &AdmittedProvider,
    timeout: Duration,
    stderr: JoinHandle<io::Result<CapturedBytes>>,
) -> ProviderHostError {
    let stderr = if stderr.is_finished() {
        join_capture(stderr, "stderr")
            .map(|captured| captured.diagnostic())
            .unwrap_or_else(|_| empty_stderr_diagnostic())
    } else {
        empty_stderr_diagnostic()
    };
    ProviderHostError::Timeout {
        provider_id: provider.descriptor.provider_id.as_str().to_owned(),
        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        stderr,
    }
}

fn empty_stderr_diagnostic() -> ProviderStderrDiagnostic {
    ProviderStderrDiagnostic {
        sha256: sha256_hex(&[]),
        bytes: 0,
        truncated: false,
    }
}

fn deadline_error(provider: &AdmittedProvider, timeout: Duration) -> ProviderHostError {
    ProviderHostError::Timeout {
        provider_id: provider.descriptor.provider_id.as_str().to_owned(),
        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        stderr: empty_stderr_diagnostic(),
    }
}

fn spawn_writer(mut stdin: ChildStdin, input: Vec<u8>) -> io::Result<JoinHandle<io::Result<()>>> {
    thread::Builder::new()
        .name("aw-provider-stdin".to_owned())
        .spawn(move || {
            stdin.write_all(&input)?;
            stdin.flush()
        })
}

fn spawn_capture<R>(
    name: &str,
    mut reader: R,
    retain_limit: usize,
) -> io::Result<JoinHandle<io::Result<CapturedBytes>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new().name(name.to_owned()).spawn(move || {
        let mut retained = Vec::with_capacity(retain_limit.min(8 * 1024));
        let mut total_bytes = 0_u64;
        let mut digest = Sha256::new();
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(read as u64);
            digest.update(&chunk[..read]);
            let remaining = retain_limit.saturating_sub(retained.len());
            retained.extend_from_slice(&chunk[..read.min(remaining)]);
        }
        Ok(CapturedBytes {
            bytes: retained,
            total_bytes,
            sha256: format!("{:x}", digest.finalize()),
        })
    })
}

fn join_writer(handle: JoinHandle<io::Result<()>>) -> Result<io::Result<()>, ProviderHostError> {
    handle
        .join()
        .map_err(|_| ProviderHostError::Process("Provider stdin writer panicked".to_owned()))
}

fn join_capture(
    handle: JoinHandle<io::Result<CapturedBytes>>,
    stream: &'static str,
) -> Result<CapturedBytes, ProviderHostError> {
    handle
        .join()
        .map_err(|_| ProviderHostError::Process(format!("Provider {stream} reader panicked")))?
        .map_err(|error| {
            ProviderHostError::Process(format!("failed to read Provider {stream}: {error}"))
        })
}

fn map_response(
    provider: &AdmittedProvider,
    capability: &AdmittedCapability,
    invocation: &CapabilityInvocation,
    response: serde_json::Value,
    output_limit: usize,
    started_at_ms: u64,
    completed_at_ms: u64,
) -> Result<ProviderInvocationResult, ProviderHostError> {
    let native_disposition = response
        .pointer(&capability.codec.disposition.source)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            invalid_response(
                provider,
                "disposition pointer is absent or not text",
                &response,
            )
        })?;
    let disposition = capability
        .codec
        .disposition
        .values
        .get(native_disposition)
        .copied()
        .ok_or_else(|| invalid_response(provider, "native disposition is not mapped", &response))?;
    let (output, output_bytes) = if disposition == ProviderDisposition::Produced {
        let mut output_body = serde_json::json!({});
        for mapping in &capability.codec.output_fields {
            if !mapping.when_disposition.contains(&disposition) {
                continue;
            }
            let value = match &mapping.source {
                OutputValueSource::Constant(value) => Some(value.clone()),
                OutputValueSource::Input(pointer) => {
                    invocation.input.body.pointer(pointer).cloned()
                }
                OutputValueSource::Response(pointer) => response.pointer(pointer).cloned(),
            }
            .ok_or_else(|| {
                invalid_response(provider, "mapped output source is absent", &response)
            })?;
            set_object_pointer(&mut output_body, &mapping.target, value).map_err(|reason| {
                invalid_response(
                    provider,
                    &format!("mapped output target is invalid: {reason}"),
                    &response,
                )
            })?;
        }
        if let Err(reason) =
            schema_failure_reason(&capability.canonical_output_schema, &output_body)
        {
            return Err(invalid_response(
                provider,
                &format!("canonical output {reason}"),
                &response,
            ));
        }
        let encoded = canonical_json_v1_bytes(&output_body).map_err(|error| {
            invalid_response(
                provider,
                &format!("mapped output cannot be encoded: {error}"),
                &response,
            )
        })?;
        if encoded.len() > output_limit {
            return Err(ProviderHostError::OutputTooLarge {
                provider_id: provider.descriptor.provider_id.as_str().to_owned(),
                limit: output_limit,
                output_sha256: sha256_digest(&encoded).as_str().to_owned(),
            });
        }
        let bytes = encoded.len() as u64;
        (
            Some(ProviderPayload {
                schema: capability.descriptor.output_contract.schema.clone(),
                digest: sha256_digest(&encoded),
                body: output_body,
            }),
            Some(bytes),
        )
    } else {
        (None, None)
    };
    let meters = capability
        .codec
        .meters
        .iter()
        .map(|mapping| {
            let value = response
                .pointer(&mapping.value_pointer)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    invalid_response(
                        provider,
                        "meter pointer is absent or not an unsigned integer",
                        &response,
                    )
                })?;
            Ok(ProviderMeter {
                meter_id: mapping.meter_id.clone(),
                unit: mapping.unit.clone(),
                measurement_kind: mapping.measurement_kind,
                method: mapping.method.clone(),
                value,
            })
        })
        .collect::<Result<Vec<_>, ProviderHostError>>()?;
    let error = disposition_error(disposition)?;
    let output_schema = output.as_ref().map(|value| value.schema.clone());
    let output_digest = output.as_ref().map(|value| value.digest.clone());
    let receipt = ProviderReceipt {
        invocation_id: invocation.invocation_id.clone(),
        provider_id: provider.descriptor.provider_id.clone(),
        provider_version: provider.descriptor.provider_version.clone(),
        manifest_digest: provider.descriptor.manifest_digest.clone(),
        binding_id: invocation.binding_id.clone(),
        provider_generation: None,
        capability: invocation.capability.clone(),
        input_schema: invocation.input.schema.clone(),
        input_digest: invocation.input.digest.clone(),
        scope: invocation.scope.clone(),
        disposition,
        output_schema,
        output_digest,
        output_bytes,
        error,
        meters,
        evidence: Vec::<ProviderEvidenceRef>::new(),
        started_at_ms,
        completed_at_ms,
    };
    Ok(ProviderInvocationResult {
        outcome: ProviderInvocationOutcome { output },
        receipt,
    })
}

fn accepted_failure(
    provider: &AdmittedProvider,
    capability: &AdmittedCapability,
    invocation: &CapabilityInvocation,
    started_at_ms: u64,
    failure: ProviderHostError,
) -> Result<ProviderInvocationResult, ProviderHostError> {
    let disposition =
        if capability.descriptor.authority == aw_contracts::provider::ProviderAuthority::Enforce {
            ProviderDisposition::Uncertain
        } else {
            ProviderDisposition::Failed
        };
    let (code, safe_message, retryable) = match &failure {
        ProviderHostError::Timeout { .. } => (
            "provider_timeout",
            "Provider did not settle before the invocation deadline",
            true,
        ),
        ProviderHostError::NonZeroExit { .. } => (
            "provider_nonzero_exit",
            "Provider exited without a successful native response",
            false,
        ),
        ProviderHostError::OutputTooLarge { .. } => (
            "provider_output_too_large",
            "Provider output exceeded its admitted byte limit",
            false,
        ),
        ProviderHostError::InvalidResponse { .. } => (
            "provider_invalid_response",
            "Provider output did not satisfy the declared response mapping",
            false,
        ),
        _ => (
            "provider_process_failed",
            "Provider execution failed after invocation acceptance",
            true,
        ),
    };
    let error = ContractError::new(
        code,
        ErrorCategory::RuntimeUnavailable,
        retryable,
        safe_message,
    )
    .map_err(|build_error| {
        ProviderHostError::Process(format!(
            "failed to construct static Provider failure receipt: {build_error}"
        ))
    })?;
    Ok(ProviderInvocationResult {
        outcome: ProviderInvocationOutcome { output: None },
        receipt: ProviderReceipt {
            invocation_id: invocation.invocation_id.clone(),
            provider_id: provider.descriptor.provider_id.clone(),
            provider_version: provider.descriptor.provider_version.clone(),
            manifest_digest: provider.descriptor.manifest_digest.clone(),
            binding_id: invocation.binding_id.clone(),
            provider_generation: None,
            capability: invocation.capability.clone(),
            input_schema: invocation.input.schema.clone(),
            input_digest: invocation.input.digest.clone(),
            scope: invocation.scope.clone(),
            disposition,
            output_schema: None,
            output_digest: None,
            output_bytes: None,
            error: Some(error),
            meters: Vec::new(),
            evidence: Vec::new(),
            started_at_ms,
            completed_at_ms: match unix_time_ms() {
                Ok(completed_at_ms) => completed_at_ms,
                Err(_) => started_at_ms,
            },
        },
    })
}

fn disposition_error(
    disposition: ProviderDisposition,
) -> Result<Option<ContractError>, ProviderHostError> {
    let specification = match disposition {
        ProviderDisposition::Denied => Some((
            "provider_denied",
            ErrorCategory::PolicyDenied,
            false,
            "Provider denied the capability invocation",
        )),
        ProviderDisposition::Failed => Some((
            "provider_failed",
            ErrorCategory::Internal,
            false,
            "Provider reported a terminal capability failure",
        )),
        ProviderDisposition::Uncertain => Some((
            "provider_uncertain",
            ErrorCategory::Internal,
            true,
            "Provider could not prove whether the capability settled",
        )),
        ProviderDisposition::Produced
        | ProviderDisposition::EffectApplied
        | ProviderDisposition::Bypassed => None,
    };
    specification
        .map(|(code, category, retryable, message)| {
            ContractError::new(code, category, retryable, message).map_err(|error| {
                ProviderHostError::Process(format!(
                    "failed to construct static Provider error: {error}"
                ))
            })
        })
        .transpose()
}

fn invalid_response(
    provider: &AdmittedProvider,
    reason: &str,
    response: &serde_json::Value,
) -> ProviderHostError {
    // `Value` serialization is infallible today; retain a deterministic marker
    // if that upstream invariant ever changes rather than exposing content.
    let encoded = match canonical_json_v1_bytes(response) {
        Ok(encoded) => encoded,
        Err(_) => b"unserializable-provider-json".to_vec(),
    };
    ProviderHostError::InvalidResponse {
        provider_id: provider.descriptor.provider_id.as_str().to_owned(),
        reason: reason.to_owned(),
        output_sha256: sha256_hex(&encoded),
    }
}

fn unix_time_ms() -> Result<u64, ProviderHostError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ProviderHostError::Process(format!("system clock is invalid: {error}")))?;
    u64::try_from(duration.as_millis()).map_err(|_| {
        ProviderHostError::Process("system clock does not fit the Provider contract".to_owned())
    })
}

fn sha256_digest(bytes: &[u8]) -> Digest {
    Digest::parse(sha256_hex(bytes))
        .unwrap_or_else(|_| unreachable!("SHA-256 formatting always produces a canonical digest"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn exit_status(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| code.to_string())
}

struct ProcessOutput {
    stdout: CapturedBytes,
}

struct CapturedBytes {
    bytes: Vec<u8>,
    total_bytes: u64,
    sha256: String,
}

impl CapturedBytes {
    fn diagnostic(&self) -> ProviderStderrDiagnostic {
        ProviderStderrDiagnostic {
            sha256: self.sha256.clone(),
            bytes: self.total_bytes,
            truncated: self.total_bytes > self.bytes.len() as u64,
        }
    }
}
