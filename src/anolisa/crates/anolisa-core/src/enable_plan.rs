//! `enable --dry-run` plan contract.
//!
//! [`plan_enable`] is a pure, side-effect-free function that takes a
//! catalog snapshot, distribution index, environment facts, install-mode
//! string, and a capability name and returns an [`EnablePlan`] describing
//! what `anolisa enable <cap>` *would* do. It does NOT touch the
//! filesystem, write state, download artifacts, or modify any service —
//! the plan is data only.
//!
//! Status vocabulary (`PlanStatus`):
//!
//! * `Ready`    — every precheck passed and an artifact resolved for every
//!                component.
//! * `Degraded` — at least one non-fatal warning (e.g. version drift, an
//!                optional probe is unknown). The plan can still execute.
//! * `Blocked`  — at least one blocker: env mismatch, missing component
//!                manifest, install-mode unsupported, no matching
//!                artifact. The plan cannot execute as-is.
//!
//! `PlanError` is reserved for inputs that prevent us from producing any
//! plan at all — currently only an unknown capability name. Everything
//! else surfaces inside the plan via per-check / per-component status.

use serde::Serialize;

use anolisa_env::EnvFacts;
use anolisa_platform::fs_layout::FsLayout;

use crate::catalog::Catalog;
use crate::distribution::{
    ArtifactType, DistributionEntry, DistributionIndex, ResolveError, ResolveQuery,
};
use crate::manifest::{DistributionSelector, EnvRequirements};

/// JSON schema version for the [`EnablePlan`] payload. Bump whenever the
/// wire shape changes in a way that consumers must observe.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// Errors that prevent generating any plan. Surface as INVALID_ARGUMENT.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("capability '{0}' is not in the catalog")]
    UnknownCapability(String),
}

/// Overall plan status, in machine-friendly snake_case for `--json`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Ready,
    Degraded,
    Blocked,
}

impl PlanStatus {
    /// Wire label, mirrors the serde discriminator so human renderers can
    /// reuse the same vocabulary without round-tripping through JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
        }
    }
}

/// Plan envelope: the structured preview of what `enable` would execute.
#[derive(Debug, Serialize)]
pub struct EnablePlan {
    pub schema_version: u32,
    pub capability: String,
    pub stability: String,
    pub install_mode: String,
    pub dry_run: bool,
    pub status: PlanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub components: Vec<ComponentPlan>,
    pub prechecks: Vec<PrecheckResult>,
    pub env_facts: EnvFactsSummary,
    pub layout: LayoutSummary,
    pub warnings: Vec<String>,
    pub advice: Vec<String>,
    pub next_actions: Vec<String>,
}

/// Per-component slice of the plan. Mirrors what the install runner
/// would do for one component without naming the runner explicitly.
#[derive(Debug, Serialize)]
pub struct ComponentPlan {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_version: Option<String>,
    pub status: PlanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactPlan>,
    pub services: Vec<String>,
    /// Destination patterns from `install.files` (unrendered, e.g.
    /// `"{bindir}/agentsight"`). Kept verbatim so users see manifest
    /// intent before any layout substitution happens.
    pub files: Vec<String>,
    /// `files` rendered against the resolved [`FsLayout`] (e.g.
    /// `"/usr/local/bin/agentsight"`). Same length and order as `files`.
    pub resolved_files: Vec<String>,
    pub requires_privilege: bool,
    pub env_requirements: EnvRequirements,
}

/// Concrete artifact the resolver picked for a component.
#[derive(Debug, Serialize)]
pub struct ArtifactPlan {
    pub artifact_type: String,
    pub backend: String,
    pub version: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
}

/// Single precheck outcome. `status` is one of `"ok" | "warn" | "fail"`.
#[derive(Debug, Serialize, Clone)]
pub struct PrecheckResult {
    pub name: String,
    pub status: String,
    pub expected: String,
    pub actual: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Subset of [`EnvFacts`] that is meaningful for capability enabling.
#[derive(Debug, Serialize)]
pub struct EnvFactsSummary {
    pub os: String,
    pub arch: String,
    pub libc: Option<String>,
    pub pkg_base: Option<String>,
    pub kernel: Option<String>,
    pub btf: Option<bool>,
    pub cap_bpf: Option<bool>,
}

/// Resolved filesystem layout paths the install would target.
#[derive(Debug, Serialize)]
pub struct LayoutSummary {
    pub bin_dir: String,
    pub etc_dir: String,
    pub state_dir: String,
    pub log_dir: String,
    pub manifests_overlay: String,
}

/// Build an [`EnablePlan`] for `capability_name`. Performs no IO.
///
/// On unknown capabilities returns `PlanError::UnknownCapability`. Every
/// other failure mode (env mismatch, missing component manifest, install
/// mode unsupported, no matching artifact) is reported in the plan's
/// `status` / `blocked_reason` / per-component `status` so the caller can
/// render a single coherent dry-run output.
pub fn plan_enable(
    catalog: &Catalog,
    dist_index: &DistributionIndex,
    env: &EnvFacts,
    install_mode: &str,
    layout: &FsLayout,
    capability_name: &str,
) -> Result<EnablePlan, PlanError> {
    let cap = catalog
        .capability(capability_name)
        .ok_or_else(|| PlanError::UnknownCapability(capability_name.to_string()))?;

    let mut warnings: Vec<String> = Vec::new();
    let advice: Vec<String> = Vec::new();

    let mut prechecks = capability_prechecks(&cap.env_requirements, env, install_mode);

    if dist_index.entries.is_empty() {
        warnings.push(
            "distribution index is empty or unavailable — no prebuilt artifacts will resolve"
                .to_string(),
        );
    }

    let mut components: Vec<ComponentPlan> = Vec::with_capacity(cap.components.len());
    for comp_name in &cap.components {
        components.push(plan_component(
            catalog,
            dist_index,
            env,
            install_mode,
            layout,
            comp_name,
            &mut prechecks,
            &mut warnings,
        ));
    }

    let any_fail = prechecks.iter().any(|p| p.status == "fail");
    let any_warn = prechecks.iter().any(|p| p.status == "warn");
    let any_comp_blocked = components.iter().any(|c| c.status == PlanStatus::Blocked);
    let any_comp_degraded = components.iter().any(|c| c.status == PlanStatus::Degraded);

    let status = if any_fail || any_comp_blocked {
        PlanStatus::Blocked
    } else if any_warn || any_comp_degraded || !warnings.is_empty() {
        PlanStatus::Degraded
    } else {
        PlanStatus::Ready
    };

    let blocked_reason = if status == PlanStatus::Blocked {
        Some(first_blocker(&prechecks, &components))
    } else {
        None
    };

    let next_actions = next_actions_for(status, capability_name);

    let env_facts = EnvFactsSummary {
        os: env.os.clone(),
        arch: env.arch.clone(),
        libc: env.libc.clone(),
        pkg_base: env.pkg_base.clone(),
        kernel: env.kernel.clone(),
        btf: env.btf,
        cap_bpf: env.cap_bpf,
    };

    let layout = LayoutSummary {
        bin_dir: layout.bin_dir.display().to_string(),
        etc_dir: layout.etc_dir.display().to_string(),
        state_dir: layout.state_dir.display().to_string(),
        log_dir: layout.log_dir.display().to_string(),
        manifests_overlay: layout.manifests_overlay.display().to_string(),
    };

    Ok(EnablePlan {
        schema_version: PLAN_SCHEMA_VERSION,
        capability: cap.capability.name.clone(),
        stability: cap.capability.stability.clone(),
        install_mode: install_mode.to_string(),
        dry_run: true,
        status,
        blocked_reason,
        components,
        prechecks,
        env_facts,
        layout,
        warnings,
        advice,
        next_actions,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn capability_prechecks(
    reqs: &EnvRequirements,
    env: &EnvFacts,
    install_mode: &str,
) -> Vec<PrecheckResult> {
    let mut out = eval_env_requirements(reqs, env, "");
    out.push(PrecheckResult {
        name: "install_mode".to_string(),
        status: "ok".to_string(),
        expected: install_mode.to_string(),
        actual: install_mode.to_string(),
        message: None,
    });
    out
}

/// Evaluate an [`EnvRequirements`] against an [`EnvFacts`] snapshot and
/// return one [`PrecheckResult`] per declared requirement. Used by both
/// capability-level checks and per-component checks; the latter passes a
/// `prefix` so the wire names disambiguate (`agentsight.btf` vs `btf`).
///
/// Three-valued result per check:
/// * `"ok"`   — requirement met.
/// * `"warn"` — probe unavailable on this host so we cannot evaluate; the
///   plan may still execute but should be reviewed.
/// * `"fail"` — probe returned a value that violates the requirement.
fn eval_env_requirements(
    reqs: &EnvRequirements,
    env: &EnvFacts,
    prefix: &str,
) -> Vec<PrecheckResult> {
    let mut out = Vec::new();
    let qualify = |n: &str| -> String {
        if prefix.is_empty() {
            n.to_string()
        } else {
            format!("{prefix}.{n}")
        }
    };

    if !reqs.os.is_empty() {
        let ok = reqs.os.iter().any(|o| o == &env.os);
        out.push(PrecheckResult {
            name: qualify("os"),
            status: if ok { "ok" } else { "fail" }.to_string(),
            expected: reqs.os.join("|"),
            actual: env.os.clone(),
            message: None,
        });
    }

    if !reqs.arch.is_empty() {
        let ok = reqs.arch.iter().any(|a| a == &env.arch);
        out.push(PrecheckResult {
            name: qualify("arch"),
            status: if ok { "ok" } else { "fail" }.to_string(),
            expected: reqs.arch.join("|"),
            actual: env.arch.clone(),
            message: None,
        });
    }

    if !reqs.libc.is_empty() {
        match env.libc.as_deref() {
            Some(l) => {
                let ok = reqs.libc.iter().any(|x| x == l);
                out.push(PrecheckResult {
                    name: qualify("libc"),
                    status: if ok { "ok" } else { "fail" }.to_string(),
                    expected: reqs.libc.join("|"),
                    actual: l.to_string(),
                    message: None,
                });
            }
            None => out.push(PrecheckResult {
                name: qualify("libc"),
                status: "warn".to_string(),
                expected: reqs.libc.join("|"),
                actual: "unknown".to_string(),
                message: Some("libc probe is unavailable on this host".to_string()),
            }),
        }
    }

    if !reqs.pkg_base.is_empty() {
        match env.pkg_base.as_deref() {
            Some(b) => {
                let ok = reqs.pkg_base.iter().any(|x| x == b);
                out.push(PrecheckResult {
                    name: qualify("pkg_base"),
                    status: if ok { "ok" } else { "fail" }.to_string(),
                    expected: reqs.pkg_base.join("|"),
                    actual: b.to_string(),
                    message: None,
                });
            }
            None => out.push(PrecheckResult {
                name: qualify("pkg_base"),
                status: "warn".to_string(),
                expected: reqs.pkg_base.join("|"),
                actual: "unknown".to_string(),
                message: Some("pkg_base probe is unavailable on this host".to_string()),
            }),
        }
    }

    if let Some(min) = reqs.kernel_min.as_deref() {
        // `kernel_min` is preserved verbatim from the manifest (e.g.
        // `">=5.8"` or `"5.8"`). Display whichever form the manifest
        // used, but normalize once for the numeric compare.
        let expected = if min.starts_with('>') {
            min.to_string()
        } else {
            format!(">={min}")
        };
        match env.kernel.as_deref() {
            Some(actual) => match compare_kernel(actual, min) {
                KernelCmp::Satisfies => out.push(PrecheckResult {
                    name: qualify("kernel"),
                    status: "ok".to_string(),
                    expected,
                    actual: actual.to_string(),
                    message: None,
                }),
                KernelCmp::Fails => out.push(PrecheckResult {
                    name: qualify("kernel"),
                    status: "fail".to_string(),
                    expected,
                    actual: actual.to_string(),
                    message: None,
                }),
                KernelCmp::Unparseable => out.push(PrecheckResult {
                    name: qualify("kernel"),
                    status: "warn".to_string(),
                    expected,
                    actual: actual.to_string(),
                    message: Some("could not parse kernel version string".to_string()),
                }),
            },
            None => out.push(PrecheckResult {
                name: qualify("kernel"),
                status: "warn".to_string(),
                expected,
                actual: "unknown".to_string(),
                message: Some("kernel probe is unavailable on this host".to_string()),
            }),
        }
    }

    if let Some(req) = reqs.btf {
        let expected = req.to_string();
        match env.btf {
            Some(actual) if actual == req => out.push(PrecheckResult {
                name: qualify("btf"),
                status: "ok".to_string(),
                expected,
                actual: actual.to_string(),
                message: None,
            }),
            Some(actual) => out.push(PrecheckResult {
                name: qualify("btf"),
                status: "fail".to_string(),
                expected,
                actual: actual.to_string(),
                message: Some("/sys/kernel/btf/vmlinux is not present".to_string()),
            }),
            None => out.push(PrecheckResult {
                name: qualify("btf"),
                status: "warn".to_string(),
                expected,
                actual: "unknown".to_string(),
                message: Some("BTF probe is unavailable on this host".to_string()),
            }),
        }
    }

    if let Some(req) = reqs.cap_bpf {
        let expected = req.to_string();
        match env.cap_bpf {
            Some(actual) if actual == req => out.push(PrecheckResult {
                name: qualify("cap_bpf"),
                status: "ok".to_string(),
                expected,
                actual: actual.to_string(),
                message: None,
            }),
            Some(actual) => out.push(PrecheckResult {
                name: qualify("cap_bpf"),
                status: "fail".to_string(),
                expected,
                actual: actual.to_string(),
                message: Some("process lacks CAP_BPF — run as root or grant CAP_BPF".to_string()),
            }),
            None => out.push(PrecheckResult {
                name: qualify("cap_bpf"),
                status: "warn".to_string(),
                expected,
                actual: "unknown".to_string(),
                message: Some("CAP_BPF probe is unavailable on this host".to_string()),
            }),
        }
    }

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KernelCmp {
    Satisfies,
    Fails,
    Unparseable,
}

/// Best-effort `actual >= requirement` comparison on dotted version strings.
/// Returns `Unparseable` when neither side is a clean `<int>(.int)*` prefix
/// so callers can downgrade to `warn` instead of false-failing.
fn compare_kernel(actual: &str, requirement: &str) -> KernelCmp {
    fn parse(s: &str) -> Option<Vec<u64>> {
        // Take everything up to the first non-numeric / non-dot character so
        // suffixes like "-anolis23.x86_64" don't break parsing.
        let head: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if head.is_empty() {
            return None;
        }
        let mut nums = Vec::new();
        for part in head.split('.') {
            if part.is_empty() {
                continue;
            }
            nums.push(part.parse::<u64>().ok()?);
        }
        if nums.is_empty() { None } else { Some(nums) }
    }
    let req = requirement
        .trim_start_matches(">=")
        .trim_start_matches('>')
        .trim();
    let (Some(a), Some(r)) = (parse(actual), parse(req)) else {
        return KernelCmp::Unparseable;
    };
    let len = a.len().max(r.len());
    for i in 0..len {
        let av = a.get(i).copied().unwrap_or(0);
        let rv = r.get(i).copied().unwrap_or(0);
        if av > rv {
            return KernelCmp::Satisfies;
        }
        if av < rv {
            return KernelCmp::Fails;
        }
    }
    KernelCmp::Satisfies
}

fn plan_component(
    catalog: &Catalog,
    dist_index: &DistributionIndex,
    env: &EnvFacts,
    install_mode: &str,
    layout: &FsLayout,
    name: &str,
    prechecks: &mut Vec<PrecheckResult>,
    warnings: &mut Vec<String>,
) -> ComponentPlan {
    let comp = match catalog.component(name) {
        Some(c) => c,
        None => {
            return ComponentPlan {
                name: name.to_string(),
                manifest_version: None,
                status: PlanStatus::Blocked,
                blocked_reason: Some(format!(
                    "component '{name}' is not in the catalog (catalog inconsistency)"
                )),
                artifact: None,
                services: Vec::new(),
                files: Vec::new(),
                resolved_files: Vec::new(),
                requires_privilege: false,
                env_requirements: EnvRequirements::default(),
            };
        }
    };

    let requires_privilege = install_mode == "system";
    let resolved_files = render_files(&comp.install.files, layout);

    // Evaluate the component's own env_requirements and append to the
    // top-level precheck list so users see a single ordered table. The
    // outcome also drives this component's status (warn → degraded,
    // fail → blocked) without needing a second pass.
    let comp_checks = eval_env_requirements(&comp.env_requirements, env, &comp.component.name);
    let comp_has_fail = comp_checks.iter().any(|p| p.status == "fail");
    let comp_has_warn = comp_checks.iter().any(|p| p.status == "warn");
    let first_comp_fail = comp_checks
        .iter()
        .find(|p| p.status == "fail")
        .map(|p| describe_precheck_blocker(p));
    prechecks.extend(comp_checks);

    // Install-mode gate from component manifest. The resolver also enforces
    // this, but checking it here gives a clearer blocked reason than
    // `UnsupportedMode` and avoids confusion when the component manifest
    // is more conservative than the index.
    if !comp.install.modes.is_empty() && !comp.install.modes.iter().any(|m| m == install_mode) {
        return ComponentPlan {
            name: comp.component.name.clone(),
            manifest_version: Some(comp.component.version.clone()),
            status: PlanStatus::Blocked,
            blocked_reason: Some(format!(
                "component '{}' does not support install_mode '{}' (allowed: {})",
                comp.component.name,
                install_mode,
                comp.install.modes.join(", "),
            )),
            artifact: None,
            services: comp.install.services.clone(),
            files: comp.install.files.clone(),
            resolved_files,
            requires_privilege,
            env_requirements: comp.env_requirements.clone(),
        };
    }

    if comp_has_fail {
        return ComponentPlan {
            name: comp.component.name.clone(),
            manifest_version: Some(comp.component.version.clone()),
            status: PlanStatus::Blocked,
            blocked_reason: first_comp_fail.or_else(|| {
                Some(format!(
                    "component '{}' env requirements not satisfied",
                    comp.component.name
                ))
            }),
            artifact: None,
            services: comp.install.services.clone(),
            files: comp.install.files.clone(),
            resolved_files,
            requires_privilege,
            env_requirements: comp.env_requirements.clone(),
        };
    }

    let preferred = select_preferred_types(&comp.distribution_selectors, env, install_mode);

    let query = ResolveQuery {
        component: &comp.component.name,
        version: None,
        channel: None,
        install_mode,
        os: &env.os,
        arch: &env.arch,
        libc: env.libc.as_deref(),
        pkg_base: env.pkg_base.as_deref(),
        preferred_types: &preferred,
    };

    match dist_index.resolve(&query) {
        Ok(entry) => {
            if entry.version != comp.component.version {
                warnings.push(format!(
                    "component '{}': artifact version {} differs from manifest version {}",
                    comp.component.name, entry.version, comp.component.version,
                ));
            }
            // Missing sha256 is a structural blocker, not a soft warning:
            // the executor's DownloadCache does not enforce a checksum when
            // given None, so a Degraded-but-executable plan would silently
            // install unverified bytes. Surface as Blocked so the user
            // fixes the distribution index instead.
            if entry.sha256.is_none() {
                return ComponentPlan {
                    name: comp.component.name.clone(),
                    manifest_version: Some(comp.component.version.clone()),
                    status: PlanStatus::Blocked,
                    blocked_reason: Some(format!(
                        "resolved artifact for component '{}' has no sha256 — refuse to install without verification",
                        comp.component.name,
                    )),
                    artifact: Some(artifact_plan_from(&entry)),
                    services: comp.install.services.clone(),
                    files: comp.install.files.clone(),
                    resolved_files,
                    requires_privilege,
                    env_requirements: comp.env_requirements.clone(),
                };
            }
            let status = if comp_has_warn {
                PlanStatus::Degraded
            } else {
                PlanStatus::Ready
            };
            ComponentPlan {
                name: comp.component.name.clone(),
                manifest_version: Some(comp.component.version.clone()),
                status,
                blocked_reason: None,
                artifact: Some(artifact_plan_from(&entry)),
                services: comp.install.services.clone(),
                files: comp.install.files.clone(),
                resolved_files,
                requires_privilege,
                env_requirements: comp.env_requirements.clone(),
            }
        }
        Err(err) => {
            let reason = describe_resolve_error(&comp.component.name, &err, env, install_mode);
            ComponentPlan {
                name: comp.component.name.clone(),
                manifest_version: Some(comp.component.version.clone()),
                status: PlanStatus::Blocked,
                blocked_reason: Some(reason),
                artifact: None,
                services: comp.install.services.clone(),
                files: comp.install.files.clone(),
                resolved_files,
                requires_privilege,
                env_requirements: comp.env_requirements.clone(),
            }
        }
    }
}

/// Substitute `{bindir}`, `{etcdir}`/`{etc_dir}`, `{statedir}`/`{state_dir}`,
/// `{logdir}`/`{log_dir}`, `{datadir}` in each manifest file pattern using
/// the resolved layout. Unknown placeholders are left as-is so users can
/// still see which patterns the install runner would need to handle.
fn render_files(files: &[String], layout: &FsLayout) -> Vec<String> {
    let bin = layout.bin_dir.display().to_string();
    let etc = layout.etc_dir.display().to_string();
    let state = layout.state_dir.display().to_string();
    let log = layout.log_dir.display().to_string();
    let data = layout.datadir.display().to_string();
    files
        .iter()
        .map(|f| {
            f.replace("{bindir}", &bin)
                .replace("{etcdir}", &etc)
                .replace("{etc_dir}", &etc)
                .replace("{statedir}", &state)
                .replace("{state_dir}", &state)
                .replace("{logdir}", &log)
                .replace("{log_dir}", &log)
                .replace("{datadir}", &data)
        })
        .collect()
}

fn describe_precheck_blocker(p: &PrecheckResult) -> String {
    let detail = p.message.as_deref().unwrap_or("");
    if detail.is_empty() {
        format!(
            "precheck '{}' failed: expected {}, actual {}",
            p.name, p.expected, p.actual,
        )
    } else {
        format!(
            "precheck '{}' failed: expected {}, actual {} ({})",
            p.name, p.expected, p.actual, detail,
        )
    }
}

fn artifact_plan_from(entry: &DistributionEntry) -> ArtifactPlan {
    ArtifactPlan {
        artifact_type: artifact_type_label(entry.artifact_type),
        backend: entry.backend.clone(),
        version: entry.version.clone(),
        url: entry.url.clone(),
        sha256: entry.sha256.clone(),
        signature: entry.signature.clone(),
        artifact_id: entry.artifact_id.clone(),
    }
}

fn artifact_type_label(t: ArtifactType) -> String {
    match t {
        ArtifactType::Rpm => "rpm",
        ArtifactType::Deb => "deb",
        ArtifactType::TarGz => "tar_gz",
        ArtifactType::Zip => "zip",
        ArtifactType::Oci => "oci",
        ArtifactType::File => "file",
        ArtifactType::Binary => "binary",
    }
    .to_string()
}

fn select_preferred_types(
    selectors: &[DistributionSelector],
    env: &EnvFacts,
    install_mode: &str,
) -> Vec<ArtifactType> {
    selectors
        .iter()
        .find(|sel| selector_matches(sel, env, install_mode))
        .map(|sel| sel.preferred_artifact_types.clone())
        .unwrap_or_default()
}

fn selector_matches(sel: &DistributionSelector, env: &EnvFacts, install_mode: &str) -> bool {
    if let Some(m) = sel.install_mode.as_deref() {
        if m != install_mode {
            return false;
        }
    }
    if !sel.os.is_empty() && !sel.os.iter().any(|o| o == &env.os) {
        return false;
    }
    if !sel.arch.is_empty() && !sel.arch.iter().any(|a| a == &env.arch) {
        return false;
    }
    if let Some(l) = sel.libc.as_deref() {
        match env.libc.as_deref() {
            Some(env_l) if env_l == l => {}
            _ => return false,
        }
    }
    if let Some(b) = sel.pkg_base.as_deref() {
        match env.pkg_base.as_deref() {
            Some(env_b) if env_b == b => {}
            _ => return false,
        }
    }
    true
}

fn describe_resolve_error(
    component: &str,
    err: &ResolveError,
    env: &EnvFacts,
    install_mode: &str,
) -> String {
    match err {
        ResolveError::NotFound => format!(
            "no prebuilt artifact for component '{}' on {}/{} (install_mode={})",
            component, env.os, env.arch, install_mode,
        ),
        ResolveError::UnsupportedMode => format!(
            "component '{}' has no artifact for install_mode '{}'",
            component, install_mode,
        ),
        ResolveError::Ambiguous(list) => format!(
            "multiple artifacts match component '{}' ({} candidates); declare preferred_artifact_types to disambiguate",
            component,
            list.len(),
        ),
        ResolveError::ChecksumMissing => format!(
            "matching artifact for component '{}' has no sha256",
            component,
        ),
    }
}

fn first_blocker(prechecks: &[PrecheckResult], components: &[ComponentPlan]) -> String {
    if let Some(p) = prechecks.iter().find(|p| p.status == "fail") {
        return describe_precheck_blocker(p);
    }
    if let Some(c) = components.iter().find(|c| c.status == PlanStatus::Blocked) {
        if let Some(r) = c.blocked_reason.as_deref() {
            return r.to_string();
        }
        return format!("component '{}' is blocked", c.name);
    }
    "plan is blocked".to_string()
}

fn next_actions_for(status: PlanStatus, capability: &str) -> Vec<String> {
    match status {
        PlanStatus::Ready => vec![format!("run `anolisa enable {capability}` to execute")],
        PlanStatus::Degraded => vec![
            "review warnings above".to_string(),
            format!("run `anolisa enable {capability}` to execute after reviewing warnings"),
        ],
        PlanStatus::Blocked => vec![format!(
            "resolve the blocker above before running `anolisa enable {capability}`"
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::distribution::{ArtifactType, DistributionEntry, DistributionIndex};
    use crate::manifest::{
        BuildSpec, CapabilityManifest, CapabilityMeta, ComponentManifest, ComponentMeta,
        DistributionSelector, FeatureSpec, HealthSpec, InstallSpec, SourceSpec,
    };
    use crate::{Catalog, CatalogLayers};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn make_env(os: &str, arch: &str) -> EnvFacts {
        EnvFacts {
            os: os.to_string(),
            arch: arch.to_string(),
            libc: if os == "linux" {
                Some("glibc".to_string())
            } else {
                None
            },
            kernel: None,
            pkg_base: if os == "linux" {
                Some("anolis23".to_string())
            } else {
                None
            },
            btf: if os == "linux" { Some(true) } else { None },
            cap_bpf: if os == "linux" { Some(true) } else { None },
            container: None,
            user: "tester".to_string(),
            uid: 1000,
            home: PathBuf::from("/tmp/home"),
        }
    }

    fn make_layout() -> FsLayout {
        FsLayout::user(PathBuf::from("/tmp/home"))
    }

    fn make_cap(
        name: &str,
        components: Vec<String>,
        os: Vec<String>,
        arch: Vec<String>,
    ) -> CapabilityManifest {
        CapabilityManifest {
            schema_version: 2,
            capability: CapabilityMeta {
                name: name.to_string(),
                description: format!("test capability {name}"),
                layer: "tier1-capability".to_string(),
                stability: "stable".to_string(),
            },
            components,
            default_features: Vec::new(),
            env_requirements: EnvRequirements {
                os,
                arch,
                libc: Vec::new(),
                kernel_min: None,
                btf: None,
                cap_bpf: None,
                pkg_base: Vec::new(),
            },
        }
    }

    fn make_component(
        name: &str,
        version: &str,
        install_modes: Vec<String>,
        selectors: Vec<DistributionSelector>,
    ) -> ComponentManifest {
        ComponentManifest {
            schema_version: 2,
            component: ComponentMeta {
                name: name.to_string(),
                version: version.to_string(),
                layer: "runtime".to_string(),
                domain: "test".to_string(),
            },
            source: SourceSpec::default(),
            distribution_selectors: selectors,
            build: BuildSpec::default(),
            install: InstallSpec {
                modes: install_modes,
                files: vec!["{bindir}/agentsight".to_string()],
                services: vec!["agentsight.service".to_string()],
                capabilities: Vec::new(),
            },
            env_requirements: EnvRequirements::default(),
            dependencies: Vec::new(),
            features: Vec::<FeatureSpec>::new(),
            adapters: Vec::new(),
            health: HealthSpec::default(),
        }
    }

    fn make_catalog(caps: Vec<CapabilityManifest>, comps: Vec<ComponentManifest>) -> Catalog {
        let mut capabilities = BTreeMap::new();
        for c in caps {
            capabilities.insert(c.capability.name.clone(), c);
        }
        let mut components = BTreeMap::new();
        for c in comps {
            components.insert(c.component.name.clone(), c);
        }
        Catalog {
            capabilities,
            components,
            layers: CatalogLayers::bundled_only(PathBuf::from("/dev/null")),
        }
    }

    fn agentsight_entry(
        version: &str,
        artifact_type: ArtifactType,
        backend: &str,
        install_modes: Vec<String>,
        os: &str,
        arch: &str,
        pkg_base: Option<String>,
    ) -> DistributionEntry {
        DistributionEntry {
            component: "agentsight".to_string(),
            version: version.to_string(),
            channel: "stable".to_string(),
            artifact_type,
            backend: backend.to_string(),
            url: format!("https://example.invalid/agentsight-{version}.{backend}"),
            os: os.to_string(),
            arch: arch.to_string(),
            libc: Some("glibc".to_string()),
            pkg_base,
            install_modes,
            sha256: Some("0".repeat(64)),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: Vec::new(),
        }
    }

    fn make_index(entries: Vec<DistributionEntry>) -> DistributionIndex {
        DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries,
        }
    }

    #[test]
    fn happy_path_linux_x86_64_yields_ready_plan() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Ready);
        assert!(plan.blocked_reason.is_none());
        assert_eq!(plan.components.len(), 1);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.name, "agentsight");
        assert_eq!(comp_plan.status, PlanStatus::Ready);
        let artifact = comp_plan.artifact.as_ref().expect("artifact selected");
        assert_eq!(artifact.artifact_type, "rpm");
        assert_eq!(artifact.version, "0.2.0");
        assert!(comp_plan.requires_privilege);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn version_drift_between_manifest_and_index_marks_degraded() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Index advertises only 0.1.0 — older than manifest.
        let index = make_index(vec![agentsight_entry(
            "0.1.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Degraded);
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("0.1.0") && w.contains("0.2.0"))
        );
    }

    #[test]
    fn unknown_capability_returns_plan_error() {
        let catalog = make_catalog(Vec::new(), Vec::new());
        let index = make_index(Vec::new());
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let err = plan_enable(&catalog, &index, &env, "system", &layout, "not-real")
            .expect_err("must error");
        assert_eq!(err, PlanError::UnknownCapability("not-real".to_string()));
    }

    #[test]
    fn missing_component_blocks_plan_with_clear_reason() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        // No component manifest registered — catalog inconsistency.
        let catalog = make_catalog(vec![cap], Vec::new());
        let index = make_index(Vec::new());
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let reason = plan.blocked_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("agentsight"),
            "reason mentions component: {reason}"
        );
        assert_eq!(plan.components[0].status, PlanStatus::Blocked);
        assert!(plan.components[0].artifact.is_none());
    }

    #[test]
    fn no_matching_artifact_on_macos_blocks_plan_without_panic() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("macos", "aarch64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        // OS precheck fails AND component artifact unresolved.
        assert!(
            plan.prechecks
                .iter()
                .any(|p| p.name == "os" && p.status == "fail")
        );
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        assert!(comp_plan.artifact.is_none());
        let reason = plan.blocked_reason.as_deref().unwrap_or("");
        assert!(!reason.is_empty(), "blocked_reason should be populated");
    }

    #[test]
    fn install_mode_user_blocks_when_component_is_system_only() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        let reason = comp_plan.blocked_reason.as_deref().unwrap_or("");
        assert!(reason.contains("install_mode"), "reason: {reason}");
        assert!(reason.contains("user"));
    }

    /// Resolved artifact without sha256 → Blocked, not Degraded: the
    /// executor cannot verify unsigned/unhashed bytes and must refuse.
    #[test]
    fn missing_sha256_marks_component_blocked() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let mut entry = agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        );
        entry.sha256 = None;
        let index = make_index(vec![entry]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        // Artifact stays populated so users see what was selected and why.
        assert!(comp_plan.artifact.is_some(), "artifact should be surfaced");
        let reason = comp_plan.blocked_reason.as_deref().unwrap_or("");
        assert!(reason.contains("sha256"), "reason names sha256: {reason}");
    }

    #[test]
    fn preferred_artifact_types_tiebreak_selects_tar_gz() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            vec![DistributionSelector {
                install_mode: Some("system".to_string()),
                os: vec!["linux".to_string()],
                arch: vec!["x86_64".to_string()],
                libc: None,
                pkg_base: None,
                preferred_artifact_types: vec![ArtifactType::TarGz, ArtifactType::Rpm],
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![
            agentsight_entry(
                "0.2.0",
                ArtifactType::Rpm,
                "rpm",
                vec!["system".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
            agentsight_entry(
                "0.2.0",
                ArtifactType::TarGz,
                "tar",
                vec!["system".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
        ]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Ready);
        let artifact = plan.components[0]
            .artifact
            .as_ref()
            .expect("tiebreak picked an artifact");
        assert_eq!(artifact.artifact_type, "tar_gz");
    }

    #[test]
    fn libc_unknown_on_non_linux_marks_warn_not_fail() {
        // Requirement says libc=glibc but env is macOS with libc=None.
        let mut cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["macos".to_string()],
            vec!["aarch64".to_string()],
        );
        cap.env_requirements.libc = vec!["glibc".to_string()];
        let comp = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![DistributionEntry {
            component: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            channel: "stable".to_string(),
            artifact_type: ArtifactType::TarGz,
            backend: "tar".to_string(),
            url: "https://example.invalid/agentsight-0.2.0-macos.tar.gz".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            libc: None,
            pkg_base: None,
            install_modes: vec!["user".to_string()],
            sha256: Some("0".repeat(64)),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: Vec::new(),
        }]);
        let env = make_env("macos", "aarch64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        // libc warn alone (no fail) — overall status: degraded, not blocked.
        assert_eq!(plan.status, PlanStatus::Degraded);
        let libc = plan
            .prechecks
            .iter()
            .find(|p| p.name == "libc")
            .expect("libc precheck");
        assert_eq!(libc.status, "warn");
        assert_eq!(libc.actual, "unknown");
    }

    /// Component declares btf=true / cap_bpf=true / kernel_min=">=5.8" and
    /// host env satisfies all of them — the plan still resolves to Ready
    /// and the component prechecks appear with `agentsight.*` namespace.
    #[test]
    fn component_env_requirements_satisfied_keeps_plan_ready() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        comp.env_requirements.kernel_min = Some("5.8".to_string());
        comp.env_requirements.btf = Some(true);
        comp.env_requirements.cap_bpf = Some(true);
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let mut env = make_env("linux", "x86_64");
        env.kernel = Some("6.6.0-anolis23.x86_64".to_string());
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Ready);
        // Component checks should be present and namespaced.
        let names: Vec<&str> = plan.prechecks.iter().map(|p| p.name.as_str()).collect();
        assert!(names.iter().any(|n| *n == "agentsight.kernel"));
        assert!(names.iter().any(|n| *n == "agentsight.btf"));
        assert!(names.iter().any(|n| *n == "agentsight.cap_bpf"));
        for p in &plan.prechecks {
            assert_eq!(p.status, "ok", "{}: {p:?}", p.name);
        }
    }

    /// Component requires btf=true but host reports btf=Some(false) — the
    /// component must be blocked and the namespaced precheck must fail.
    #[test]
    fn component_btf_required_but_disabled_blocks_component() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        comp.env_requirements.btf = Some(true);
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let mut env = make_env("linux", "x86_64");
        env.btf = Some(false);
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        let btf = plan
            .prechecks
            .iter()
            .find(|p| p.name == "agentsight.btf")
            .expect("namespaced btf precheck");
        assert_eq!(btf.status, "fail");
    }

    /// Component requires btf=true but the probe is unknown (e.g. macOS
    /// host that still happens to satisfy os/arch). Should warn, not fail,
    /// and the component should land in `degraded`, not `blocked`.
    #[test]
    fn component_btf_unknown_marks_component_degraded() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        comp.env_requirements.btf = Some(true);
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let mut env = make_env("linux", "x86_64");
        env.btf = None;
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Degraded);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Degraded);
        let btf = plan
            .prechecks
            .iter()
            .find(|p| p.name == "agentsight.btf")
            .expect("namespaced btf precheck");
        assert_eq!(btf.status, "warn");
    }

    /// kernel_min comparator parses `>=X.Y` and dotted version with suffix.
    /// `5.4` < `5.8` must fail; `5.15.0-anolis23.x86_64` >= `5.8` must
    /// satisfy; an unparseable kernel string must downgrade to warn, not
    /// false-fail.
    #[test]
    fn kernel_min_compare_handles_typical_inputs() {
        assert_eq!(
            compare_kernel("5.15.0-anolis23.x86_64", "5.8"),
            KernelCmp::Satisfies
        );
        assert_eq!(compare_kernel("5.4.123", "5.8"), KernelCmp::Fails);
        assert_eq!(compare_kernel("5.8", "5.8"), KernelCmp::Satisfies);
        assert_eq!(compare_kernel("6.6", ">=5.8"), KernelCmp::Satisfies);
        assert_eq!(
            compare_kernel("not-a-version", "5.8"),
            KernelCmp::Unparseable
        );
    }

    /// Empty index → planner still returns a plan, with a top-level warning
    /// and components blocked because no artifact resolved.
    #[test]
    fn empty_distribution_index_yields_blocked_plan_with_warning() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(Vec::new());
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("distribution index")),
            "expected index warning, got {:?}",
            plan.warnings,
        );
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        assert!(comp_plan.artifact.is_none());
    }

    /// `resolved_files` must substitute layout vars. We use a user-mode
    /// layout under a tmp home so the assertion is deterministic.
    #[test]
    fn resolved_files_substitute_layout_placeholders() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar",
            vec!["user".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = FsLayout::user(PathBuf::from("/tmp/home"));
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.files, vec!["{bindir}/agentsight".to_string()]);
        let resolved = &comp_plan.resolved_files[0];
        assert!(
            !resolved.contains('{') && resolved.ends_with("/agentsight"),
            "resolved file should be substituted: {resolved}",
        );
        assert!(
            resolved.starts_with("/tmp/home/.local/bin"),
            "resolved should be under XDG bin_dir: {resolved}",
        );
    }
}
