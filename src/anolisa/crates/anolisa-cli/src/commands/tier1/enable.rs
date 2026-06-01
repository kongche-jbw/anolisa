//! `anolisa enable` — capability activation.
//!
//! P1-F wiring: dry-run path still goes through [`anolisa_core::plan_enable`]
//! and renders the resulting [`EnablePlan`] (human or JSON). On the
//! non-dry-run path the handler now drives the real-execute orchestrator
//! [`anolisa_core::execute_enable`]: download artifact → install ANOLISA-owned
//! files → write `InstalledState` capability/component objects → append
//! `started` / `succeeded` records to `CentralLog` → release the
//! [`InstallLock`]. Any mid-flight failure self-cleans (unlinks files,
//! appends a `Failed` record, releases the lock).
//!
//! Scope limits enforced here (rather than inside the planner / executor)
//! so the underlying libraries stay general while the CLI surface honors
//! the launch-spec scope:
//!
//! * Exactly one capability per invocation.
//! * `--feature`, `--with-adapter`, `--from-source` are not supported on
//!   either path yet; we reject explicitly so users see a clear contract
//!   rather than silently-ignored flags.
//! * Both `--dry-run` and real-execute are currently scoped to
//!   `agent-observability` per the P1-E2/P1-F launch criteria — other
//!   capabilities surface `NOT_IMPLEMENTED` so the scope boundary is
//!   visible.

use clap::Parser;

use anolisa_core::{
    EnablePlan, ExecuteError, ExecuteOutcome, PlanError, PlanStatus, execute_enable, plan_enable,
};
use anolisa_env::EnvService;

use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "enable";
/// Capability the CLI scope-gate currently allows on both `--dry-run` and
/// real-execute paths. Other capabilities are surfaced as `NOT_IMPLEMENTED`
/// so the boundary is visible to users.
const SUPPORTED_CAPABILITY: &str = "agent-observability";

#[derive(Parser)]
pub struct EnableArgs {
    /// Capability name(s) to enable
    #[arg(required = true)]
    pub capabilities: Vec<String>,
    /// Only enable a specific sub-feature (capability must already be enabled)
    #[arg(long, value_name = "NAME")]
    pub feature: Option<String>,
    /// Adapter framework selection: explicit list ("cosh,openclaw"), `auto`, or omit for first-party only
    #[arg(long, value_name = "FRAMEWORKS|auto")]
    pub with_adapter: Option<String>,
    /// Build component(s) from source instead of installing prebuilt
    #[arg(long)]
    pub from_source: bool,
}

pub fn handle(args: EnableArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = format!("enable {}", args.capabilities.join(" "));

    if args.capabilities.len() != 1 {
        return Err(CliError::InvalidArgument {
            command,
            reason: "enable currently accepts exactly one capability".to_string(),
        });
    }
    let capability = args.capabilities[0].clone();

    // Scope guards apply uniformly to dry-run AND real-execute: the
    // launch-spec scope is the same for both surfaces today.
    if args.from_source {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--from-source is not supported yet",
        ));
    }
    if args.with_adapter.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--with-adapter is not supported yet",
        ));
    }
    if args.feature.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--feature is not supported yet",
        ));
    }

    if capability != SUPPORTED_CAPABILITY {
        return Err(CliError::not_implemented_with_hint(
            command,
            format!("enable is currently scoped to '{SUPPORTED_CAPABILITY}'; got '{capability}'"),
        ));
    }

    let catalog = common::load_bundled_catalog(ctx, COMMAND)?;
    // Missing index is not a CLI error: the planner reports it inside the
    // plan (overall warning + per-component blocked) so users still get a
    // structured dry-run output instead of an opaque error message.
    let dist_index = common::load_distribution_index(ctx, COMMAND)?
        .unwrap_or_else(common::empty_distribution_index);
    let env = EnvService::detect();
    let layout = common::resolve_layout(ctx);
    let install_mode = ctx.install_mode.as_str();

    let plan = plan_enable(
        &catalog,
        &dist_index,
        &env,
        install_mode,
        &layout,
        &capability,
    )
    .map_err(|err| match err {
        PlanError::UnknownCapability(name) => CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("capability '{name}' is not in the catalog"),
        },
    })?;

    if ctx.dry_run {
        if ctx.json {
            return render_json(COMMAND, &plan);
        }
        if !ctx.quiet {
            render_human(&plan, ctx.verbose);
        }
        return Ok(());
    }

    // Real-execute path: drive `execute_enable` and render its outcome.
    let actor = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "cli".to_string());
    let outcome = execute_enable(&plan, &layout, &actor).map_err(execute_err_to_cli)?;

    if ctx.json {
        let payload = ExecutePayload::from(&outcome);
        return render_json(COMMAND, &payload);
    }

    if !ctx.quiet {
        render_execute_human(&outcome, ctx.verbose);
    }
    Ok(())
}

/// Translate an [`ExecuteError`] into the CLI's existing error surface.
///
/// TODO(P1-G): introduce a dedicated `CliError::Runtime` variant with code
/// `EXECUTION_FAILED` (exit code 1) so runtime failures are
/// distinguishable from argument errors on the wire. For this milestone
/// every variant collapses onto `INVALID_ARGUMENT` (exit 2) to keep the
/// CLI surface unchanged.
fn execute_err_to_cli(err: ExecuteError) -> CliError {
    let reason = match &err {
        ExecuteError::LockHeld { path } => format!(
            "install lock at {} is held by another process — run again after the other invocation finishes",
            path.display(),
        ),
        ExecuteError::PlanNotExecutable { status, reason } => format!(
            "plan is {status}: {reason} — run `anolisa enable agent-observability --dry-run` for details and resolve blockers before retrying",
        ),
        ExecuteError::MissingArtifact { component } => format!(
            "component '{component}' has no resolved artifact (catalog vs distribution-index mismatch — check `anolisa enable agent-observability --dry-run`)",
        ),
        ExecuteError::Download { component, source } => {
            format!("download for component '{component}' failed: {source}")
        }
        ExecuteError::Install { component, source } => {
            format!("install for component '{component}' failed: {source}")
        }
        ExecuteError::State { source } => format!("installed state write failed: {source}"),
        ExecuteError::Log { source } => format!("central log write failed: {source}"),
        ExecuteError::Lock { source } => format!("install lock io: {source}"),
    };
    CliError::InvalidArgument {
        command: COMMAND.to_string(),
        reason,
    }
}

/// Wire shape mirrored from [`ExecuteOutcome`]. Defined at the CLI
/// boundary so `anolisa-core` does not need to derive `Serialize` on its
/// internal outcome struct.
#[derive(serde::Serialize)]
struct ExecutePayload {
    operation_id: String,
    capability: String,
    install_mode: String,
    components: Vec<String>,
    installed_files: Vec<InstalledFilePayload>,
    state_path: String,
    central_log_path: String,
    warnings: Vec<String>,
}

#[derive(serde::Serialize)]
struct InstalledFilePayload {
    component: String,
    path: String,
    sha256: String,
}

impl From<&ExecuteOutcome> for ExecutePayload {
    fn from(o: &ExecuteOutcome) -> Self {
        Self {
            operation_id: o.operation_id.clone(),
            capability: o.capability.clone(),
            install_mode: o.install_mode.clone(),
            components: o.components.clone(),
            installed_files: o
                .installed_files
                .iter()
                .map(|f| InstalledFilePayload {
                    component: f.component.clone(),
                    path: f.path.display().to_string(),
                    sha256: f.sha256.clone(),
                })
                .collect(),
            state_path: o.state_path.display().to_string(),
            central_log_path: o.central_log_path.display().to_string(),
            warnings: o.warnings.clone(),
        }
    }
}

fn render_execute_human(outcome: &ExecuteOutcome, verbose: bool) {
    println!("enable {} succeeded", outcome.capability);
    println!("operation_id: {}", outcome.operation_id);
    println!("install_mode: {}", outcome.install_mode);
    println!("components:");
    for c in &outcome.components {
        println!("  - {c}");
    }
    println!("installed_files ({}):", outcome.installed_files.len());
    for f in &outcome.installed_files {
        let sha_render = if verbose {
            f.sha256.clone()
        } else {
            // Short-form sha256 keeps the human line readable; full hash
            // is one --verbose away.
            f.sha256.get(..8).unwrap_or(&f.sha256).to_string()
        };
        println!(
            "  - {}  {}  sha256={}",
            f.component,
            f.path.display(),
            sha_render,
        );
    }
    println!("state: {}", outcome.state_path.display());
    println!("log:   {}", outcome.central_log_path.display());
    if !outcome.warnings.is_empty() {
        println!("warnings:");
        for w in &outcome.warnings {
            println!("  - {w}");
        }
    }
}

fn render_human(plan: &EnablePlan, verbose: bool) {
    println!(
        "capability: {} (stability: {}, install_mode: {}, dry_run: true)",
        plan.capability, plan.stability, plan.install_mode,
    );
    println!("status: {}", plan.status.as_str());
    if let Some(reason) = plan.blocked_reason.as_deref() {
        println!("blocked: {reason}");
    }

    println!("env:");
    println!(
        "  os={} arch={} libc={} pkg_base={}",
        plan.env_facts.os,
        plan.env_facts.arch,
        plan.env_facts.libc.as_deref().unwrap_or("-"),
        plan.env_facts.pkg_base.as_deref().unwrap_or("-"),
    );

    if !plan.prechecks.is_empty() {
        println!("prechecks:");
        for p in &plan.prechecks {
            let detail = p.message.as_deref().unwrap_or("");
            println!(
                "  - {:<14} {:<5} expected={} actual={} {}",
                p.name, p.status, p.expected, p.actual, detail,
            );
        }
    }

    println!("components:");
    for c in &plan.components {
        let version = c.manifest_version.as_deref().unwrap_or("-");
        println!("  - {} v{} status={}", c.name, version, c.status.as_str(),);
        if let Some(reason) = c.blocked_reason.as_deref() {
            println!("      blocked: {reason}");
        }
        if let Some(a) = &c.artifact {
            println!(
                "      artifact: {} ({}) v{} url={}",
                a.artifact_type, a.backend, a.version, a.url,
            );
            if verbose {
                if let Some(sha) = a.sha256.as_deref() {
                    println!("      sha256: {sha}");
                }
            }
        }
        if verbose {
            if !c.services.is_empty() {
                println!("      services: {}", c.services.join(", "));
            }
            if !c.files.is_empty() {
                println!("      files: {}", c.files.join(", "));
            }
            if !c.resolved_files.is_empty() {
                println!("      resolved_files: {}", c.resolved_files.join(", "));
            }
            println!("      requires_privilege: {}", c.requires_privilege);
        }
    }

    println!("layout:");
    println!("  bin_dir:           {}", plan.layout.bin_dir);
    println!("  etc_dir:           {}", plan.layout.etc_dir);
    println!("  state_dir:         {}", plan.layout.state_dir);
    println!("  log_dir:           {}", plan.layout.log_dir);
    println!("  manifests_overlay: {}", plan.layout.manifests_overlay);

    if !plan.warnings.is_empty() {
        println!("warnings:");
        for w in &plan.warnings {
            println!("  - {w}");
        }
    }

    if !plan.next_actions.is_empty() {
        println!("next:");
        for n in &plan.next_actions {
            println!("  - {n}");
        }
    }

    let _ = PlanStatus::Ready; // silence "unused import" if future refactor drops branches
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx(json: bool, dry_run: bool, install_mode: InstallMode) -> CliContext {
        ctx_with_prefix(json, dry_run, install_mode, None)
    }

    fn ctx_with_prefix(
        json: bool,
        dry_run: bool,
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
    ) -> CliContext {
        CliContext {
            install_mode,
            prefix,
            json,
            dry_run,
            verbose: false,
            quiet: true, // suppress stdout during tests
            no_color: true,
        }
    }

    fn args(caps: &[&str]) -> EnableArgs {
        EnableArgs {
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            feature: None,
            with_adapter: None,
            from_source: false,
        }
    }

    #[test]
    fn enable_with_zero_capabilities_is_rejected_by_clap() {
        // clap enforces `required = true` upstream, so this path is owned
        // by argument parsing — confirmed by integration coverage. Here we
        // verify the multi-capability guard inside the handler instead.
        let err = handle(
            args(&["agent-observability", "tokenless"]),
            &ctx(false, true, InstallMode::System),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }

    #[test]
    fn enable_dry_run_other_capability_returns_not_implemented_with_scope_hint() {
        let err = handle(args(&["tokenless"]), &ctx(false, true, InstallMode::User))
            .expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        let hint = err.hint().unwrap_or("");
        assert!(
            hint.contains("agent-observability"),
            "hint must name the supported capability: {hint}"
        );
    }

    #[test]
    fn enable_dry_run_from_source_is_explicit_not_implemented() {
        let mut a = args(&["agent-observability"]);
        a.from_source = true;
        let err = handle(a, &ctx(false, true, InstallMode::System)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.hint().unwrap_or("").contains("--from-source"));
    }

    #[test]
    fn enable_dry_run_with_adapter_is_explicit_not_implemented() {
        let mut a = args(&["agent-observability"]);
        a.with_adapter = Some("auto".to_string());
        let err = handle(a, &ctx(false, true, InstallMode::System)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.hint().unwrap_or("").contains("--with-adapter"));
    }

    /// Smoke: with the bundled dev-tree manifests + index, the planner runs
    /// end-to-end and produces a plan. We assert the call succeeds and that
    /// the plan capability/install_mode match the request — we do not assert
    /// a specific status because the host env (linux vs macOS) drives it.
    #[test]
    fn enable_dry_run_happy_path_returns_plan_on_host() {
        // We need an env-agnostic ctx, but ctx.install_mode=System with
        // ctx.prefix=None defaults to / on system mode; that's fine for a
        // bundled-catalog dry-run since nothing is written.
        let _ = PathBuf::from("/tmp");
        let result = handle(
            args(&["agent-observability"]),
            &ctx(true, true, InstallMode::System),
        );
        result.expect("dry-run plan should not error on bundled fixtures");
    }

    /// On macOS the OS precheck for `agent-observability` (requires linux)
    /// turns the plan `Blocked`. The real-execute path must refuse a
    /// `Blocked` plan with an `INVALID_ARGUMENT` whose reason names both
    /// the block status and the suggested `--dry-run` next step — and it
    /// must do so without touching the real `/var/lib/anolisa/lock`. We
    /// rebase the layout under a tempdir via `ctx.prefix` so any lock /
    /// state IO that does happen lands in tmp.
    #[test]
    fn enable_execute_without_dry_run_blocked_plan_returns_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let result = handle(
            args(&["agent-observability"]),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        );

        // The macOS precheck fails the os check → plan.status = blocked
        // → execute_enable returns PlanNotExecutable. If a future change
        // ever makes this path succeed on the dev host we want to know,
        // so we assert specifically on the blocked reason.
        let err = result.expect_err("blocked plan must surface as a CLI error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        let reason = err.reason();
        assert!(
            reason.contains("blocked"),
            "reason must mention 'blocked': {reason}"
        );
        assert!(
            reason.contains("dry-run"),
            "reason must point at --dry-run: {reason}"
        );
    }

    /// Real-execute path must keep the scope guard: capabilities other
    /// than `agent-observability` are surfaced as `NOT_IMPLEMENTED` with
    /// the supported-capability name in the hint, just like dry-run.
    #[test]
    fn enable_execute_without_dry_run_other_capability_still_not_implemented() {
        let err = handle(args(&["tokenless"]), &ctx(false, false, InstallMode::User))
            .expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        let hint = err.hint().unwrap_or("");
        assert!(
            hint.contains("agent-observability"),
            "hint must name the supported capability: {hint}",
        );
    }

    /// Real-execute path must keep the flag-scope guards: `--from-source`
    /// is still `NOT_IMPLEMENTED` with the flag named in the hint.
    #[test]
    fn enable_execute_without_dry_run_from_source_still_not_implemented() {
        let mut a = args(&["agent-observability"]);
        a.from_source = true;
        let err = handle(a, &ctx(false, false, InstallMode::System)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.hint().unwrap_or("").contains("--from-source"));
    }
}
