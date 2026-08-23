//! Local operator CLI for the durable Agent Memory backend.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_memory::knowledge::KnowledgeProvider;
use agent_memory::knowledge::mant::{MantCliConfig, MantCliProvider};
use agent_memory::protocol::{
    BackendRequestContext, ContextBudget, FeedbackOutcome, IdentityContext, LocalManagementContext,
    LocalMemoryBackend, LocalMemoryStats, MemoryAuthority, MemoryBackend, MemoryEvent,
    MemoryEventKind, MemoryEventOutcome, MemoryObjectKind, ProtocolError, ProtocolErrorCode,
    RecallBinding, RecallPurpose, RecallTrace, RuntimeContext, SessionOutcome,
    default_local_memory_path, local_workspace_id,
};
use clap::{Parser, Subcommand, ValueEnum};
use nix::unistd::Uid;
use serde::Serialize;
use ulid::Ulid;

const BACKEND_ID: &str = "local-sqlite-v1";
const LOCAL_STORE_ACTION: &str =
    "Check that the local state directory is writable and owner-only (mode 0700), then retry.";

#[derive(Debug, Parser)]
#[command(name = "agent-memory-ctl", version)]
#[command(about = "Inspect and verify the local Agent Memory backend")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show bounded storage and object counters.
    Status {
        /// Emit one machine-readable JSON object.
        #[arg(long)]
        json: bool,
    },
    /// Check the local store and optional Cosh and ManT integrations.
    Doctor {
        /// Emit one machine-readable JSON object.
        #[arg(long)]
        json: bool,
    },
    /// Capture synthetic evidence and recall it from a new local session.
    Demo {
        /// Emit one machine-readable JSON object.
        #[arg(long)]
        json: bool,
    },
    /// Explain why items were selected for one ContextView.
    Why {
        /// ContextView identifier printed by Cosh or the demo command.
        context_view_id: String,
        /// Emit one machine-readable JSON object.
        #[arg(long)]
        json: bool,
    },
    /// Delete one object in the current trusted workspace scope.
    Forget {
        /// Object category to delete.
        #[arg(value_enum)]
        kind: ObjectKind,
        /// Stable object identifier.
        memory_id: String,
        /// Confirm the destructive operation.
        #[arg(long)]
        yes: bool,
        /// Emit one machine-readable JSON object.
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    fn json(&self) -> bool {
        match self {
            Self::Status { json }
            | Self::Doctor { json }
            | Self::Demo { json }
            | Self::Why { json, .. }
            | Self::Forget { json, .. } => *json,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ObjectKind {
    Task,
    Event,
    ContextView,
}

impl ObjectKind {
    fn protocol_kind(self) -> MemoryObjectKind {
        match self {
            Self::Task => MemoryObjectKind::Task,
            Self::Event => MemoryObjectKind::Event,
            Self::ContextView => MemoryObjectKind::ContextView,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Event => "event",
            Self::ContextView => "context_view",
        }
    }
}

#[derive(Debug)]
struct CliFailure {
    code: &'static str,
    message: String,
    action: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorOutput<'a> {
    status: &'static str,
    code: &'a str,
    message: &'a str,
    action: &'a str,
}

#[derive(Debug, Serialize)]
struct StatusOutput {
    status: &'static str,
    backend: &'static str,
    durability: &'static str,
    logical_bytes: u64,
    physical_bytes: u64,
    sessions: u64,
    events: u64,
    tasks: u64,
    context_views: u64,
    session_capacity: u64,
    event_capacity: u64,
    task_capacity: u64,
    context_view_capacity: u64,
    context_view_retention_days: u16,
    closed_session_retention_days: u16,
    recall_sample_size: u64,
    recall_with_items: u64,
    reported_outcomes: u64,
    useful_outcomes: u64,
    diagnostic_recall_samples: u64,
}

impl From<LocalMemoryStats> for StatusOutput {
    fn from(stats: LocalMemoryStats) -> Self {
        Self {
            status: "ready",
            backend: BACKEND_ID,
            durability: "durable",
            logical_bytes: stats.logical_bytes,
            physical_bytes: stats.physical_bytes,
            sessions: stats.session_count,
            events: stats.event_count,
            tasks: stats.task_count,
            context_views: stats.view_count,
            session_capacity: stats.session_capacity,
            event_capacity: stats.event_capacity,
            task_capacity: stats.task_capacity,
            context_view_capacity: stats.view_capacity,
            context_view_retention_days: stats.view_retention_days,
            closed_session_retention_days: stats.closed_session_retention_days,
            recall_sample_size: stats.recall_sample_count,
            recall_with_items: stats.recall_with_items_count,
            reported_outcomes: stats.reported_outcome_count,
            useful_outcomes: stats.useful_outcome_count,
            diagnostic_recall_samples: stats.diagnostic_recall_sample_count,
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorOutput {
    status: &'static str,
    checks: Vec<DoctorCheck>,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    required: bool,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct DemoOutput {
    status: &'static str,
    captured_events: u64,
    recalled_items: u64,
    recalled_candidate_evidence: u64,
    context_view_id: String,
    cold_reopen_ms: u64,
    outcome: &'static str,
    message: &'static str,
}

#[derive(Debug, Serialize)]
struct WhyOutput {
    status: &'static str,
    context_view_id: String,
    backend: String,
    degraded: bool,
    degradation_reason: Option<String>,
    candidates: u64,
    admitted: u64,
    decisions: Vec<WhyDecision>,
    outcome: &'static str,
}

#[derive(Debug, Serialize)]
struct WhyDecision {
    item_id: String,
    admitted: bool,
    rank: u32,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ForgetOutput<'a> {
    status: &'static str,
    kind: &'a str,
    memory_id: &'a str,
    deleted: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.command.json();
    match run(cli.command) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            render_failure(&error, json);
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<bool, CliFailure> {
    match command {
        Command::Status { json } => status(json),
        Command::Doctor { json } => doctor(json),
        Command::Demo { json } => demo(json),
        Command::Why {
            context_view_id,
            json,
        } => why(&context_view_id, json),
        Command::Forget {
            kind,
            memory_id,
            yes,
            json,
        } => forget(kind, &memory_id, yes, json),
    }
}

fn status(json: bool) -> Result<bool, CliFailure> {
    let backend = open_backend()?;
    let report = StatusOutput::from(backend.stats().map_err(protocol_failure)?);
    if json {
        write_json(&report)?;
    } else {
        println!("Agent Memory is ready.");
        println!("Backend: {} ({})", report.backend, report.durability);
        println!(
            "Stored: {} sessions, {} events, {} tasks, {} context views",
            report.sessions, report.events, report.tasks, report.context_views
        );
        println!(
            "Space: {} logical bytes, {} physical bytes",
            report.logical_bytes, report.physical_bytes
        );
        println!(
            "Capacity: {}/{} sessions, {}/{} events, {}/{} tasks, {}/{} context views",
            report.sessions,
            report.session_capacity,
            report.events,
            report.event_capacity,
            report.tasks,
            report.task_capacity,
            report.context_views,
            report.context_view_capacity
        );
        println!(
            "Lifecycle: context views {} days; closed sessions and raw events {} days",
            report.context_view_retention_days, report.closed_session_retention_days
        );
        println!(
            "Recent recall: {}/{} views returned items; {}/{} reported outcomes were useful",
            report.recall_with_items,
            report.recall_sample_size,
            report.useful_outcomes,
            report.reported_outcomes
        );
        println!(
            "Diagnostics: {} synthetic views excluded from recent recall",
            report.diagnostic_recall_samples
        );
    }
    Ok(true)
}

fn doctor(json: bool) -> Result<bool, CliFailure> {
    let store = match open_backend()
        .and_then(|backend| backend.stats().map(|_| ()).map_err(protocol_failure))
    {
        Ok(()) => DoctorCheck {
            name: "local_store",
            status: "ok",
            required: true,
            detail: "private durable store is ready".to_string(),
            action: None,
        },
        Err(error) => DoctorCheck {
            name: "local_store",
            status: "error",
            required: true,
            detail: error.message,
            action: Some(error.action),
        },
    };
    let hook_available = command_path("agent-memory-cosh-hook").is_some();
    let hook = DoctorCheck {
        name: "cosh_hook",
        status: if hook_available { "ok" } else { "error" },
        required: true,
        detail: if hook_available {
            "Cosh lifecycle hook is available on PATH".to_string()
        } else {
            "Cosh lifecycle hook is not available on PATH".to_string()
        },
        action: (!hook_available).then_some(
            "Reinstall agent-memory and ensure its bin directory is present on the Cosh PATH.",
        ),
    };
    let cosh_available = ["cosh", "co", "copilot"]
        .into_iter()
        .any(|name| command_path(name).is_some());
    let cosh = DoctorCheck {
        name: "cosh_runtime",
        status: if cosh_available {
            "available"
        } else {
            "not_found"
        },
        required: false,
        detail: if cosh_available {
            "Cosh runtime is available on PATH".to_string()
        } else {
            "Cosh runtime was not found; local CLI checks still work".to_string()
        },
        action: (!cosh_available)
            .then_some("Install cosh-ng before expecting automatic lifecycle capture and recall."),
    };
    let mant = mant_doctor_check();
    let healthy = store.status == "ok" && hook_available;
    let report = DoctorOutput {
        status: if healthy { "healthy" } else { "unhealthy" },
        checks: vec![store, hook, cosh, mant],
    };
    if json {
        write_json(&report)?;
    } else {
        println!("Agent Memory doctor: {}", report.status);
        for check in &report.checks {
            println!("[{}] {}: {}", check.status, check.name, check.detail);
            if let Some(action) = check.action {
                println!("  Action: {action}");
            }
        }
    }
    Ok(healthy)
}

fn demo(json: bool) -> Result<bool, CliFailure> {
    let backend = open_backend()?;
    let identity = local_identity()?;
    let run_id = Ulid::new().to_string();
    let capture_context = request_context(
        &identity,
        format!("memory-ctl-capture-{run_id}"),
        format!("memory-ctl-trace-capture-{run_id}"),
        &run_id,
    );
    backend
        .open_session(&capture_context, &runtime_context())
        .map_err(protocol_failure)?;
    let event_id = format!("memory-ctl-demo-{run_id}");
    let demo_summary = format!("demo verification {run_id} completed successfully");
    let replayed = backend
        .append_event(
            &capture_context,
            &event_id,
            &MemoryEvent {
                event_id: event_id.clone(),
                kind: MemoryEventKind::ToolCompleted,
                source: "agent-memory-ctl".to_string(),
                outcome: MemoryEventOutcome::Succeeded,
                observed_at_ms: now_ms(),
                summary: demo_summary,
                evidence_ref: Some(format!("agent-memory://demo/{run_id}")),
            },
        )
        .map_err(protocol_failure)?;
    if replayed {
        return Err(CliFailure {
            code: "demo_identity_conflict",
            message: "the synthetic demo event unexpectedly replayed".to_string(),
            action: "Run agent-memory-ctl demo again. If this repeats, run agent-memory-ctl doctor.",
        });
    }
    backend
        .close_session(
            &capture_context,
            &format!("memory-ctl-close-capture-{run_id}"),
            SessionOutcome::Completed,
        )
        .map_err(protocol_failure)?;

    drop(backend);
    let cold_reopen_started = Instant::now();
    let backend = open_backend()?;
    let cold_reopen_ms =
        u64::try_from(cold_reopen_started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let recall_context = request_context(
        &identity,
        format!("memory-ctl-recall-{run_id}"),
        format!("memory-ctl-trace-recall-{run_id}"),
        &run_id,
    );
    backend
        .open_session(&recall_context, &runtime_context())
        .map_err(protocol_failure)?;
    let recall_query = format!("demo verification {run_id}");
    let view = backend
        .materialize_context(
            &recall_context,
            RecallPurpose::Turn,
            &RecallBinding {
                task_id: Some(format!("memory-ctl-demo-filter-{run_id}")),
                target_agent_id: None,
            },
            &recall_query,
            ContextBudget {
                max_tokens: 1_024,
                max_bytes: 4 * 1_024,
                max_items: 8,
            },
        )
        .map_err(protocol_failure)?;
    let candidate_evidence = view
        .items
        .iter()
        .filter(|item| {
            item.kind == agent_memory::protocol::ContextItemKind::Evidence
                && item.authority == MemoryAuthority::Candidate
        })
        .count();
    let recalled_demo = view.items.iter().any(|item| {
        item.kind == agent_memory::protocol::ContextItemKind::Evidence
            && item.authority == MemoryAuthority::Candidate
            && item.source_ref.ends_with(&event_id)
    });
    if !recalled_demo {
        return Err(CliFailure {
            code: "demo_recall_empty",
            message: "synthetic evidence was captured but not recalled".to_string(),
            action: "Run agent-memory-ctl doctor, then retry the demo.",
        });
    }
    let admitted_item_ids = view
        .items
        .iter()
        .map(|item| item.item_id.clone())
        .collect::<Vec<_>>();
    backend
        .report_recall_outcome(
            &recall_context,
            &format!("memory-ctl-outcome-{run_id}"),
            &view.context_view_id,
            &admitted_item_ids,
            &[],
            FeedbackOutcome::Useful,
        )
        .map_err(protocol_failure)?;
    backend
        .close_session(
            &recall_context,
            &format!("memory-ctl-close-recall-{run_id}"),
            SessionOutcome::Completed,
        )
        .map_err(protocol_failure)?;

    let report = DemoOutput {
        status: "ok",
        captured_events: 1,
        recalled_items: u64::try_from(view.items.len()).unwrap_or(u64::MAX),
        recalled_candidate_evidence: u64::try_from(candidate_evidence).unwrap_or(u64::MAX),
        context_view_id: view.context_view_id,
        cold_reopen_ms,
        outcome: "useful",
        message: "Synthetic evidence was recalled in a new local session.",
    };
    if json {
        write_json(&report)?;
    } else {
        println!("Agent Memory demo succeeded.");
        println!("Captured: {} synthetic event", report.captured_events);
        println!(
            "Recalled: {} items, including {} candidate evidence item",
            report.recalled_items, report.recalled_candidate_evidence
        );
        println!("Outcome: {}", report.outcome);
        println!("Cold backend reopen: {} ms", report.cold_reopen_ms);
        println!("Context view: {}", report.context_view_id);
        println!(
            "Explain it: agent-memory-ctl why {}",
            report.context_view_id
        );
        println!("Run `agent-memory-ctl status` to inspect the updated counters.");
    }
    Ok(true)
}

fn why(context_view_id: &str, json: bool) -> Result<bool, CliFailure> {
    let backend = open_backend()?;
    let management = local_management_context()?;
    let trace = backend
        .explain_owned_view(&management, context_view_id)
        .map_err(protocol_failure)?;
    let report = why_output(trace);
    if json {
        write_json(&report)?;
    } else {
        println!("Agent Memory explanation: {}", report.context_view_id);
        println!("Backend: {}", report.backend);
        println!(
            "Candidates: {}, admitted: {}",
            report.candidates, report.admitted
        );
        println!("Outcome: {}", report.outcome);
        if report.degraded {
            println!(
                "Degraded: {}",
                report.degradation_reason.as_deref().unwrap_or("unknown")
            );
        }
        for decision in &report.decisions {
            println!(
                "- rank {} [{}] {}: {}",
                decision.rank,
                if decision.admitted { "used" } else { "dropped" },
                decision.item_id,
                decision.reason
            );
        }
    }
    Ok(true)
}

fn forget(kind: ObjectKind, memory_id: &str, yes: bool, json: bool) -> Result<bool, CliFailure> {
    if !yes {
        return Err(CliFailure {
            code: "confirmation_required",
            message: "forget requires explicit confirmation".to_string(),
            action: "Review the object identifier, then repeat the command with --yes.",
        });
    }
    let backend = open_backend()?;
    let management = local_management_context()?;
    let deleted = backend
        .forget_owned(&management, kind.protocol_kind(), memory_id)
        .map_err(protocol_failure)?;
    let report = ForgetOutput {
        status: if deleted { "forgotten" } else { "not_found" },
        kind: kind.label(),
        memory_id,
        deleted,
    };
    if json {
        write_json(&report)?;
    } else if deleted {
        println!(
            "Forgot {} {} in the current workspace.",
            kind.label(),
            memory_id
        );
    } else {
        println!(
            "No matching {} is visible in the current workspace.",
            kind.label()
        );
    }
    Ok(deleted)
}

fn why_output(trace: RecallTrace) -> WhyOutput {
    let candidates = u64::try_from(trace.decisions.len()).unwrap_or(u64::MAX);
    let admitted = u64::try_from(
        trace
            .decisions
            .iter()
            .filter(|decision| decision.admitted)
            .count(),
    )
    .unwrap_or(u64::MAX);
    let outcome = trace
        .outcome_report
        .as_ref()
        .map(|report| match report.outcome {
            FeedbackOutcome::Useful => "useful",
            FeedbackOutcome::Irrelevant => "irrelevant",
            FeedbackOutcome::Harmful => "harmful",
            FeedbackOutcome::Unknown => "unknown",
        })
        .unwrap_or("unreported");
    WhyOutput {
        status: "explained",
        context_view_id: trace.context_view_id,
        backend: trace.backend_id,
        degraded: trace.degraded,
        degradation_reason: trace.degradation_reason,
        candidates,
        admitted,
        decisions: trace
            .decisions
            .into_iter()
            .map(|decision| WhyDecision {
                item_id: decision.item_id,
                admitted: decision.admitted,
                rank: decision.rank,
                reason: decision.reason,
            })
            .collect(),
        outcome,
    }
}

fn open_backend() -> Result<LocalMemoryBackend, CliFailure> {
    let path = default_local_memory_path().map_err(protocol_failure)?;
    LocalMemoryBackend::open(path).map_err(protocol_failure)
}

fn local_management_context() -> Result<LocalManagementContext, CliFailure> {
    LocalManagementContext::from_identity(&local_identity()?).map_err(protocol_failure)
}

fn mant_doctor_check() -> DoctorCheck {
    let configured = env::var_os("ANOLISA_MANT_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let Some(executable) = configured.or_else(|| command_path("mant")) else {
        return DoctorCheck {
            name: "mant",
            status: "not_found",
            required: false,
            detail: "optional ManT command was not found; local memory remains usable".to_string(),
            action: None,
        };
    };
    let mut config = MantCliConfig::new(executable);
    config.timeout = Duration::from_millis(500);
    let health = MantCliProvider::new(config).health();
    match (health.descriptor, health.error) {
        (Some(descriptor), None) => DoctorCheck {
            name: "mant",
            status: "ok",
            required: false,
            detail: format!(
                "optional ManT protocol is compatible ({})",
                descriptor.protocol.as_deref().unwrap_or("unknown")
            ),
            action: None,
        },
        (_, Some(error)) => DoctorCheck {
            name: "mant",
            status: "degraded",
            required: false,
            detail: format!("optional ManT provider is unavailable ({:?})", error.code),
            action: Some(
                "Run `mant --doctor --format json --compact`; local memory remains usable.",
            ),
        },
        _ => DoctorCheck {
            name: "mant",
            status: "degraded",
            required: false,
            detail: "optional ManT provider returned incomplete health data".to_string(),
            action: Some(
                "Run `mant --doctor --format json --compact`; local memory remains usable.",
            ),
        },
    }
}

fn local_identity() -> Result<IdentityContext, CliFailure> {
    let cwd = env::current_dir().map_err(|_| CliFailure {
        code: "workspace_unavailable",
        message: "the current workspace is unavailable".to_string(),
        action: "Change to a readable workspace directory and retry.",
    })?;
    let workspace_id = local_workspace_id(cwd).map_err(protocol_failure)?;
    Ok(IdentityContext {
        tenant_id: None,
        team_id: None,
        user_id: format!("unix:{}", Uid::effective().as_raw()),
        agent_id: "cosh-ng".to_string(),
        session_id: String::new(),
        workspace_id,
    })
}

fn request_context(
    identity: &IdentityContext,
    session_id: String,
    trace_id: String,
    run_id: &str,
) -> BackendRequestContext {
    let mut identity = identity.clone();
    identity.session_id = session_id;
    BackendRequestContext {
        request_id: Ulid::new().to_string(),
        trace_id,
        run_id: Some(run_id.to_string()),
        task_id: None,
        turn_id: None,
        deadline_at_ms: None,
        identity,
    }
}

fn runtime_context() -> RuntimeContext {
    RuntimeContext {
        runtime: "agent-memory-ctl".to_string(),
        runtime_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        model: None,
        platform: Some("linux".to_string()),
    }
}

fn command_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| executable(candidate))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn protocol_failure(error: ProtocolError) -> CliFailure {
    let (code, action) = match error.code {
        ProtocolErrorCode::Unauthorized => ("local_store_permissions", LOCAL_STORE_ACTION),
        ProtocolErrorCode::VersionUnsupported => (
            "schema_version_unsupported",
            "Upgrade agent-memory before opening data written by a newer release.",
        ),
        ProtocolErrorCode::IntegrityFailed => (
            "local_store_integrity_failed",
            "Restore a known-good local store, then run agent-memory-ctl doctor.",
        ),
        ProtocolErrorCode::ResourceExhausted => (
            "local_store_capacity_exhausted",
            "Run agent-memory-ctl doctor and free local storage before retrying.",
        ),
        _ => (
            "memory_operation_failed",
            "Run agent-memory-ctl doctor for the next actionable check.",
        ),
    };
    CliFailure {
        code,
        message: error.safe_message,
        action,
    }
}

fn write_json<T: Serialize>(value: &T) -> Result<(), CliFailure> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value).map_err(|_| output_failure())?;
    writeln!(output).map_err(|_| output_failure())
}

fn render_failure(error: &CliFailure, json: bool) {
    let stderr = io::stderr();
    let mut output = stderr.lock();
    if json {
        let payload = ErrorOutput {
            status: "error",
            code: error.code,
            message: &error.message,
            action: error.action,
        };
        let _ = serde_json::to_writer(&mut output, &payload);
        let _ = writeln!(output);
    } else {
        let _ = writeln!(output, "agent-memory-ctl: {}", error.message);
        let _ = writeln!(output, "Action: {}", error.action);
    }
}

fn output_failure() -> CliFailure {
    CliFailure {
        code: "output_failed",
        message: "command output could not be written".to_string(),
        action: "Retry with a writable stdout destination.",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
