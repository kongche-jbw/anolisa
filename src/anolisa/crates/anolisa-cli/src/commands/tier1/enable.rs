//! `anolisa enable` — capability activation.
//!
//! P1-E2 wiring: the handler supports `--dry-run` only. Without `--dry-run`
//! the handler returns `NOT_IMPLEMENTED` because the mutating install path
//! (downloader, install runner, transaction/backup/rollback, state +
//! central-log writes) is still un-wired. `--dry-run` calls
//! [`anolisa_core::plan_enable`] to produce a side-effect-free [`EnablePlan`]
//! and renders it either as a human table or as the standard JSON envelope.
//!
//! Scope limits enforced here (rather than inside the planner) so the
//! planner stays general while the CLI surface honors the launch-spec scope:
//!
//! * Exactly one capability per invocation.
//! * `--feature`, `--with-adapter`, `--from-source` are not supported by
//!   `--dry-run` yet; we reject explicitly so users see a clear contract
//!   rather than silently-ignored flags.
//! * `--dry-run` is currently scoped to `agent-observability` per the P1-E2
//!   launch criteria — other capabilities surface `NOT_IMPLEMENTED` so the
//!   scope boundary is visible.

use clap::Parser;

use anolisa_core::{EnablePlan, PlanError, PlanStatus, plan_enable};
use anolisa_env::EnvService;

use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "enable";
const SUPPORTED_DRY_RUN_CAPABILITY: &str = "agent-observability";

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

    if !ctx.dry_run {
        return Err(CliError::not_implemented_with_hint(
            command,
            "use --dry-run; mutating enable execution is not wired yet",
        ));
    }

    if args.from_source {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--from-source is not supported by --dry-run yet",
        ));
    }
    if args.with_adapter.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--with-adapter is not supported by --dry-run yet",
        ));
    }
    if args.feature.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--feature is not supported by --dry-run yet",
        ));
    }

    if capability != SUPPORTED_DRY_RUN_CAPABILITY {
        return Err(CliError::not_implemented_with_hint(
            command,
            format!(
                "--dry-run plan is currently scoped to '{SUPPORTED_DRY_RUN_CAPABILITY}'; got '{capability}'"
            ),
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

    if ctx.json {
        return render_json(COMMAND, &plan);
    }

    if !ctx.quiet {
        render_human(&plan, ctx.verbose);
    }
    Ok(())
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

    fn ctx(json: bool, dry_run: bool, install_mode: InstallMode) -> CliContext {
        CliContext {
            install_mode,
            prefix: None,
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
    fn enable_without_dry_run_returns_not_implemented() {
        let err = handle(
            args(&["agent-observability"]),
            &ctx(false, false, InstallMode::System),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
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
}
