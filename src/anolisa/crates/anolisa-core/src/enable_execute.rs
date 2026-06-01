//! End-to-end orchestrator for `anolisa enable <capability>`.
//!
//! Given an already-built [`EnablePlan`] and a resolved [`FsLayout`],
//! [`execute_enable`] performs the minimal real install sequence:
//!
//!   1. acquire the advisory install lock;
//!   2. append a `started` audit record to the central log;
//!   3. for each component: download the artifact to the cache, then
//!      install it under the ANOLISA-owned layout;
//!   4. persist the operation outcome to `installed.toml`;
//!   5. append a `succeeded` audit record and release the lock.
//!
//! Any failure in steps 4 onwards (and any failure during a per-component
//! download/install) triggers cleanup: ANOLISA-owned files installed by
//! this operation are unlinked, a `failed` audit record is appended best
//! effort, and the lock is released. The CLI wrapper (Sub-D) renders the
//! returned [`ExecuteOutcome`] or [`ExecuteError`] to the user.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};

use anolisa_platform::fs_layout::FsLayout;

use crate::central_log::{CentralLog, CentralLogError, LogKind, LogRecord, LogStatus, Severity};
use crate::download::{DownloadCache, DownloadError};
use crate::enable_plan::{EnablePlan, PlanStatus};
use crate::install_runner::{InstallError, InstallRunner};
use crate::lock::{InstallLock, LockError};
use crate::state::{
    FileOwner, InstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    OperationRecord, OwnedFile, ServiceRef, StateError,
};

/// One file installed during this operation, tagged with its owning
/// component for later state-writing and cleanup.
#[derive(Debug, Clone)]
pub struct ExecuteInstalledFile {
    pub component: String,
    pub path: PathBuf,
    pub sha256: String,
}

/// What [`execute_enable`] actually did. Sub-D renders this to the user.
#[derive(Debug, Clone)]
pub struct ExecuteOutcome {
    pub operation_id: String,
    pub capability: String,
    pub install_mode: String,
    pub components: Vec<String>,
    pub installed_files: Vec<ExecuteInstalledFile>,
    /// Resolved on-disk paths the user can inspect after success.
    pub state_path: PathBuf,
    pub central_log_path: PathBuf,
    /// Non-fatal warnings: any plan warnings + cleanup notes if any.
    pub warnings: Vec<String>,
}

/// Failure surface for [`execute_enable`]. Every variant represents a
/// clean abort: any files this operation installed before the failure
/// are unlinked, a `failed` central-log record has been appended on a
/// best-effort basis, and the install lock has been released.
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("install lock at {path} is held by another process")]
    LockHeld { path: PathBuf },
    #[error("plan status is '{status}' — refuse to execute (reason: {reason})")]
    PlanNotExecutable { status: String, reason: String },
    #[error("component '{component}': no artifact resolved")]
    MissingArtifact { component: String },
    #[error(
        "component '{component}': resolved artifact has no sha256 — refusing to install without verification"
    )]
    MissingChecksum { component: String },
    #[error("download failed for component '{component}': {source}")]
    Download {
        component: String,
        #[source]
        source: DownloadError,
    },
    #[error("install failed for component '{component}': {source}")]
    Install {
        component: String,
        #[source]
        source: InstallError,
    },
    #[error("state write failed: {source}")]
    State {
        #[source]
        source: StateError,
    },
    #[error("central log write failed: {source}")]
    Log {
        #[source]
        source: CentralLogError,
    },
    #[error("lock io: {source}")]
    Lock {
        #[source]
        source: LockError,
    },
}

/// Execute `plan` against `layout`. `actor` is recorded in every audit
/// record (typically `$USER`, falling back to `"cli"`).
///
/// On success returns an [`ExecuteOutcome`] describing every file that
/// was written plus the audit-log / state paths the user can inspect.
///
/// On any failure inside the execution body the function:
///   1. unlinks every ANOLISA-owned file already installed in this op,
///   2. appends a [`LogStatus::Failed`] central-log record (best effort),
///   3. releases the install lock,
///   4. returns the underlying error unchanged.
pub fn execute_enable(
    plan: &EnablePlan,
    layout: &FsLayout,
    actor: &str,
) -> Result<ExecuteOutcome, ExecuteError> {
    // Step 1 — gate on plan status BEFORE we touch the lock or audit log.
    // A blocked plan must leave the filesystem untouched.
    if plan.status == PlanStatus::Blocked {
        return Err(ExecuteError::PlanNotExecutable {
            status: "blocked".to_string(),
            reason: plan.blocked_reason.clone().unwrap_or_default(),
        });
    }

    // Step 2 — acquire the install lock. Held by another process → clean
    // abort with no log entry; any other lock IO surfaces as ExecuteError::Lock.
    let lock = match InstallLock::acquire(&layout.lock_file) {
        Ok(l) => l,
        Err(LockError::Held { path }) => return Err(ExecuteError::LockHeld { path }),
        Err(other) => return Err(ExecuteError::Lock { source: other }),
    };

    // Compute the operation id and started_at AFTER the lock is held so
    // concurrent invocations don't accidentally share timestamps.
    let started_at_utc = Utc::now();
    let started_at = started_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);
    let operation_id = build_operation_id(&started_at_utc);

    // Pre-compute "objects" list once — both started and succeeded/failed
    // records must agree on the touched-objects set.
    let mut objects: Vec<String> = Vec::with_capacity(1 + plan.components.len());
    objects.push(plan.capability.clone());
    for c in &plan.components {
        objects.push(c.name.clone());
    }

    let central = CentralLog::open(layout.central_log.clone());

    // Step 3 — append the "started" record. Failure here means we have
    // nothing to clean up yet; just drop the lock and report.
    if let Err(err) = central.append(&started_record(
        &operation_id,
        plan,
        actor,
        &started_at,
        objects.clone(),
    )) {
        drop(lock);
        return Err(ExecuteError::Log { source: err });
    }

    let mut installed: Vec<ExecuteInstalledFile> = Vec::new();

    // Step 4 — per-component download + install.
    for c in &plan.components {
        let Some(artifact) = c.artifact.as_ref() else {
            let err = ExecuteError::MissingArtifact {
                component: c.name.clone(),
            };
            return cleanup_and_fail(
                err,
                &installed,
                &central,
                &operation_id,
                plan,
                actor,
                &started_at,
                objects.clone(),
                None,
                lock,
            );
        };

        // Hard guard: the planner now marks missing-sha256 plans Blocked,
        // but `execute_enable` is a public API — a hand-built plan could
        // still arrive with `artifact.sha256: None`. Refuse it here so the
        // download is never attempted without verification, regardless of
        // caller. This is defense-in-depth against bypassing the planner.
        let Some(expected_sha) = artifact.sha256.as_deref() else {
            let err = ExecuteError::MissingChecksum {
                component: c.name.clone(),
            };
            return cleanup_and_fail(
                err,
                &installed,
                &central,
                &operation_id,
                plan,
                actor,
                &started_at,
                objects.clone(),
                None,
                lock,
            );
        };

        let cache = DownloadCache::new(layout.cache_dir.clone());
        let cached = match cache.fetch(&artifact.url, Some(expected_sha)) {
            Ok(d) => d,
            Err(src) => {
                let err = ExecuteError::Download {
                    component: c.name.clone(),
                    source: src,
                };
                return cleanup_and_fail(
                    err,
                    &installed,
                    &central,
                    &operation_id,
                    plan,
                    actor,
                    &started_at,
                    objects.clone(),
                    None,
                    lock,
                );
            }
        };

        let runner = InstallRunner::new(layout);
        let resolved: Vec<PathBuf> = c.resolved_files.iter().map(PathBuf::from).collect();
        let outcome = match runner.install(&artifact.artifact_type, &cached.cached_path, &resolved)
        {
            Ok(o) => o,
            Err(src) => {
                let err = ExecuteError::Install {
                    component: c.name.clone(),
                    source: src,
                };
                return cleanup_and_fail(
                    err,
                    &installed,
                    &central,
                    &operation_id,
                    plan,
                    actor,
                    &started_at,
                    objects.clone(),
                    None,
                    lock,
                );
            }
        };

        for f in outcome.files {
            installed.push(ExecuteInstalledFile {
                component: c.name.clone(),
                path: f.path,
                sha256: f.sha256,
            });
        }
    }

    // Step 5 — persist state. `installed.toml` lives under state_dir.
    let state_path = layout.state_dir.join("installed.toml");
    let finished_at_utc = Utc::now();
    let finished_at = finished_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);

    // Snapshot the prior on-disk state so any failure from state.save()
    // onwards can restore the machine to its pre-op state. Without this
    // snapshot a successful state.save() followed by a failed succeeded-log
    // append would leave `installed.toml` claiming components are installed
    // while cleanup unlinks their files — the worst possible inconsistency
    // for a package manager. `None` means there was no prior file; cleanup
    // will remove anything this op wrote instead of restoring bytes.
    let prior_state_bytes: Option<Vec<u8>> = fs::read(&state_path).ok();

    let mut state = match InstalledState::load(&state_path) {
        Ok(s) => s,
        Err(src) => {
            return cleanup_and_fail(
                ExecuteError::State { source: src },
                &installed,
                &central,
                &operation_id,
                plan,
                actor,
                &started_at,
                objects.clone(),
                None,
                lock,
            );
        }
    };

    state.install_mode = match plan.install_mode.as_str() {
        "system" => InstallMode::System,
        _ => InstallMode::User,
    };
    state.prefix = layout.prefix.clone();

    let service_manager = if plan.install_mode == "system" {
        "systemd".to_string()
    } else {
        "systemd-user".to_string()
    };

    for c in &plan.components {
        let comp_files: Vec<OwnedFile> = installed
            .iter()
            .filter(|f| f.component == c.name)
            .map(|f| OwnedFile {
                path: f.path.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some(f.sha256.clone()),
            })
            .collect();

        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: c.name.clone(),
            version: c.manifest_version.clone().unwrap_or_default(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: c.artifact.as_ref().map(|a| a.url.clone()),
            installed_at: finished_at.clone(),
            last_operation_id: Some(operation_id.clone()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: comp_files,
            external_modified_files: Vec::new(),
            services: c
                .services
                .iter()
                .map(|svc| ServiceRef {
                    name: svc.clone(),
                    manager: service_manager.clone(),
                    restartable: true,
                    // Service enablement is out of scope for this milestone.
                    enabled: false,
                })
                .collect(),
            health: Vec::new(),
        });
    }

    state.upsert_object(InstalledObject {
        kind: ObjectKind::Capability,
        name: plan.capability.clone(),
        // Capability has no version field on the plan; use the stability
        // label so the on-disk record's version stays non-empty.
        version: plan.stability.clone(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        installed_at: finished_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: plan.components.iter().map(|c| c.name.clone()).collect(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
    });

    state.append_operation(OperationRecord {
        id: operation_id.clone(),
        command: format!("enable {}", plan.capability),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(finished_at.clone()),
    });

    if let Err(src) = state.save(&state_path) {
        // state.save uses tmp+rename so on failure the on-disk file is
        // usually the prior bytes already. Pass the snapshot anyway as
        // defense in depth — restoring known-good bytes is idempotent.
        return cleanup_and_fail(
            ExecuteError::State { source: src },
            &installed,
            &central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes.clone())),
            lock,
        );
    }

    // Step 6 — append the succeeded record. Even if state.save above
    // succeeded, a failure here is fatal: without the audit record the
    // user has no way to find this operation in `anolisa logs`. The
    // snapshot restore is load-bearing here — state.save just succeeded,
    // so the on-disk file currently claims this op completed; we must
    // roll it back before unlinking files.
    if let Err(src) = central.append(&succeeded_record(
        &operation_id,
        plan,
        actor,
        &started_at,
        &finished_at,
        objects.clone(),
    )) {
        return cleanup_and_fail(
            ExecuteError::Log { source: src },
            &installed,
            &central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes)),
            lock,
        );
    }

    let outcome = ExecuteOutcome {
        operation_id,
        capability: plan.capability.clone(),
        install_mode: plan.install_mode.clone(),
        components: plan.components.iter().map(|c| c.name.clone()).collect(),
        installed_files: installed,
        state_path,
        central_log_path: layout.central_log.clone(),
        warnings: plan.warnings.clone(),
    };
    drop(lock);
    Ok(outcome)
}

/// Cleanup helper invoked when any post-lock step fails. Unlinks every
/// file already installed in this operation, optionally rolls
/// `installed.toml` back to its pre-op bytes (only required when the
/// failure happened after `state.save()`), appends a `failed` audit
/// record (errors here are swallowed so the original failure surfaces),
/// drops the lock, and returns the original error.
///
/// `state_restore`:
///   * `None` — failure happened before `state.save()`; the state file
///     was never written by this op and must not be touched.
///   * `Some((path, Some(bytes)))` — restore `path` to `bytes` (the
///     pre-op snapshot).
///   * `Some((path, None))` — no prior state existed; remove `path`
///     entirely so the cleanup is a true rollback.
#[allow(clippy::too_many_arguments)]
fn cleanup_and_fail(
    err: ExecuteError,
    installed: &[ExecuteInstalledFile],
    central: &CentralLog,
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    objects: Vec<String>,
    state_restore: Option<(PathBuf, Option<Vec<u8>>)>,
    lock: InstallLock,
) -> Result<ExecuteOutcome, ExecuteError> {
    for f in installed {
        let _ = fs::remove_file(&f.path);
    }
    if let Some((path, prior)) = state_restore {
        match prior {
            Some(bytes) => {
                // Best-effort restore: if the rewrite fails the failed
                // audit record will still be appended below and the user
                // sees the original error, which is the right signal.
                let _ = fs::write(&path, &bytes);
            }
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let _ = central.append(&failed_record(
        operation_id,
        plan,
        actor,
        started_at,
        &finished_at,
        objects,
        &err,
    ));
    drop(lock);
    Err(err)
}

fn started_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    objects: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: format!("enable {} started", plan.capability),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: None,
        status: None,
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

fn succeeded_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: format!("enable {} succeeded", plan.capability),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Ok),
        objects,
        backup_ids: Vec::new(),
        warnings: plan.warnings.clone(),
        details: serde_json::Value::Null,
    }
}

fn failed_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
    err: &ExecuteError,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Error,
        message: format!("enable {} failed: {err}", plan.capability),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Failed),
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

/// `op-YYYYMMDDHHMMSS-<6-hex>` — sortable, unique per call, no new
/// crate deps. The 24-bit suffix is the low bits of the timestamp nanos
/// run through `DefaultHasher` so two calls inside the same second still
/// disambiguate.
fn build_operation_id(now: &chrono::DateTime<Utc>) -> String {
    let ts = now.format("%Y%m%d%H%M%S").to_string();
    let nanos = now.timestamp_nanos_opt().unwrap_or_else(|| now.timestamp());
    let mut hasher = DefaultHasher::new();
    nanos.hash(&mut hasher);
    let suffix = hasher.finish() & 0xff_ffff;
    format!("op-{ts}-{suffix:06x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::enable_plan::{
        ArtifactPlan, ComponentPlan, EnablePlan, EnvFactsSummary, LayoutSummary,
        PLAN_SCHEMA_VERSION,
    };
    use crate::manifest::EnvRequirements;
    use sha2::{Digest, Sha256};
    use std::fs as std_fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn sha256_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        let out = h.finalize();
        let mut s = String::with_capacity(64);
        for b in out {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    fn write_payload_artifact(dir: &Path, name: &str, bytes: &[u8]) -> (String, String) {
        let p = dir.join(name);
        std_fs::write(&p, bytes).expect("write payload");
        let url = format!("file://{}", p.to_str().expect("utf8 path"));
        (url, sha256_of(bytes))
    }

    fn fixture_layout(prefix: &Path) -> FsLayout {
        FsLayout::system(Some(prefix.to_path_buf()))
    }

    fn fixture_component(
        name: &str,
        artifact: Option<ArtifactPlan>,
        resolved_files: Vec<String>,
        status: PlanStatus,
    ) -> ComponentPlan {
        ComponentPlan {
            name: name.to_string(),
            manifest_version: Some("0.2.0".to_string()),
            status,
            blocked_reason: None,
            artifact,
            services: vec!["agentsight.service".to_string()],
            files: vec!["{bindir}/agentsight".to_string()],
            resolved_files,
            requires_privilege: true,
            env_requirements: EnvRequirements::default(),
        }
    }

    fn fixture_plan(
        capability: &str,
        components: Vec<ComponentPlan>,
        status: PlanStatus,
        install_mode: &str,
        layout: &FsLayout,
    ) -> EnablePlan {
        EnablePlan {
            schema_version: PLAN_SCHEMA_VERSION,
            capability: capability.to_string(),
            stability: "stable".to_string(),
            install_mode: install_mode.to_string(),
            dry_run: false,
            status,
            blocked_reason: if status == PlanStatus::Blocked {
                Some("test blocker".to_string())
            } else {
                None
            },
            components,
            prechecks: Vec::new(),
            env_facts: EnvFactsSummary {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                libc: Some("glibc".to_string()),
                pkg_base: Some("anolis23".to_string()),
                kernel: Some("6.6.0".to_string()),
                btf: Some(true),
                cap_bpf: Some(true),
            },
            layout: LayoutSummary {
                bin_dir: layout.bin_dir.display().to_string(),
                etc_dir: layout.etc_dir.display().to_string(),
                state_dir: layout.state_dir.display().to_string(),
                log_dir: layout.log_dir.display().to_string(),
                manifests_overlay: layout.manifests_overlay.display().to_string(),
            },
            warnings: Vec::new(),
            advice: Vec::new(),
            next_actions: Vec::new(),
        }
    }

    fn artifact_plan(url: &str, sha256: &str) -> ArtifactPlan {
        ArtifactPlan {
            artifact_type: "binary".to_string(),
            backend: "binary".to_string(),
            version: "0.2.0".to_string(),
            url: url.to_string(),
            sha256: Some(sha256.to_string()),
            signature: None,
            artifact_id: None,
        }
    }

    fn read_log_lines(path: &Path) -> Vec<serde_json::Value> {
        let content = std_fs::read_to_string(path).expect("read log");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("parse log line"))
            .collect()
    }

    #[test]
    fn happy_path_single_binary_installs_writes_state_and_two_logs() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"fake-agentsight-binary-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let outcome = execute_enable(&plan, &layout, "tester").expect("execute ok");
        assert_eq!(outcome.installed_files.len(), 1);
        assert_eq!(outcome.installed_files[0].path, dest);
        assert_eq!(outcome.installed_files[0].sha256, sha);
        assert!(dest.exists(), "destination binary must exist");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std_fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }

        // installed.toml — capability + component objects, one ok operation.
        let state_path = layout.state_dir.join("installed.toml");
        assert!(state_path.exists());
        let state = InstalledState::load(&state_path).expect("load state");
        let cap = state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("capability object present");
        assert_eq!(cap.status, ObjectStatus::Installed);
        let agentsight = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object present");
        assert_eq!(agentsight.status, ObjectStatus::Installed);
        assert_eq!(agentsight.files.len(), 1);
        assert_eq!(agentsight.files[0].path, dest);
        assert_eq!(state.operations.len(), 1);
        assert_eq!(state.operations[0].status, "ok");
        assert_eq!(state.operations[0].id, outcome.operation_id);

        // central log — exactly 2 lines, both for this op, second is "ok".
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2, "expected started + succeeded entries");
        for line in &lines {
            assert_eq!(
                line.get("operation_id").and_then(|v| v.as_str()),
                Some(outcome.operation_id.as_str()),
            );
        }
        assert!(lines[0].get("status").map(|v| v.is_null()).unwrap_or(true));
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"),);
    }

    #[test]
    fn blocked_plan_is_rejected_with_no_side_effects() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let comp = fixture_component(
            "agentsight",
            None,
            vec![layout.bin_dir.join("agentsight").display().to_string()],
            PlanStatus::Blocked,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Blocked,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        assert!(
            matches!(err, ExecuteError::PlanNotExecutable { ref status, .. } if status == "blocked"),
            "unexpected error: {err:?}",
        );

        // No log file, no state file, no install touched.
        assert!(!layout.central_log.exists());
        assert!(!layout.state_dir.join("installed.toml").exists());
    }

    #[test]
    fn lock_contention_returns_lock_held() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        // Hold the lock from the outside for the duration of execute_enable.
        let _held = InstallLock::acquire(&layout.lock_file).expect("hold lock");

        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan("file:///does/not/matter", &"0".repeat(64))),
            vec![layout.bin_dir.join("agentsight").display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::LockHeld { path } => assert_eq!(path, layout.lock_file),
            other => panic!("expected LockHeld, got {other:?}"),
        }
        // No log, no state.
        assert!(!layout.central_log.exists());
        assert!(!layout.state_dir.join("installed.toml").exists());
    }

    #[test]
    fn checksum_mismatch_cleans_up_partial_install_and_writes_failed_log() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload_a = b"good-component-a";
        let (url_a, sha_a) = write_payload_artifact(payloads.path(), "comp-a", payload_a);

        // Component B's artifact has correct content but wrong expected sha.
        let payload_b = b"good-component-b";
        let (url_b, _) = write_payload_artifact(payloads.path(), "comp-b", payload_b);
        let wrong_sha_b = "0".repeat(64);

        let dest_a = layout.bin_dir.join("comp-a");
        let dest_b = layout.bin_dir.join("comp-b");
        let comp_a = fixture_component(
            "comp-a",
            Some(artifact_plan(&url_a, &sha_a)),
            vec![dest_a.display().to_string()],
            PlanStatus::Ready,
        );
        let comp_b = fixture_component(
            "comp-b",
            Some(artifact_plan(&url_b, &wrong_sha_b)),
            vec![dest_b.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp_a, comp_b],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::Download {
                ref component,
                source: DownloadError::ChecksumMismatch { .. },
            } => assert_eq!(component, "comp-b"),
            other => panic!("expected Download/ChecksumMismatch on comp-b, got {other:?}"),
        }

        // Component A's file must have been unlinked by cleanup.
        assert!(!dest_a.exists(), "comp-a file must be cleaned up");
        assert!(!dest_b.exists(), "comp-b file was never installed");

        // No state file (failure before save).
        assert!(!layout.state_dir.join("installed.toml").exists());

        // Two log lines: started (info) + failed (status=failed).
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        let op_id = lines[0]
            .get("operation_id")
            .and_then(|v| v.as_str())
            .expect("op id on started");
        assert_eq!(
            lines[1].get("operation_id").and_then(|v| v.as_str()),
            Some(op_id),
        );
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
        assert_eq!(
            lines[1].get("severity").and_then(|v| v.as_str()),
            Some("error"),
        );
    }

    #[test]
    fn missing_artifact_returns_missing_artifact_error_with_no_install() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let dest = layout.bin_dir.join("agentsight");
        // Degraded is still executable per spec; we just have no artifact.
        let comp = fixture_component(
            "agentsight",
            None,
            vec![dest.display().to_string()],
            PlanStatus::Degraded,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Degraded,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::MissingArtifact { ref component } => assert_eq!(component, "agentsight"),
            other => panic!("expected MissingArtifact, got {other:?}"),
        }

        assert!(!dest.exists());
        assert!(!layout.state_dir.join("installed.toml").exists());

        // Started + failed log entries (started written before the per-component loop).
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// Executor-level checksum hard guard: even though the planner now
    /// marks missing-sha256 plans Blocked, `execute_enable` is a public
    /// API — a hand-built plan could still arrive with `artifact.sha256:
    /// None`. The executor must refuse without touching the disk: no
    /// download, no install, no state file. The started/failed audit
    /// records are still expected (the lock was acquired and started was
    /// already written before we hit the per-component loop).
    #[test]
    fn missing_checksum_in_artifact_returns_missing_checksum_error_with_no_install() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        // Construct a plan that bypasses the planner's missing-sha guard:
        // status=Ready, artifact present, but sha256=None.
        let dest = layout.bin_dir.join("agentsight");
        let artifact_no_sha = ArtifactPlan {
            artifact_type: "binary".to_string(),
            backend: "binary".to_string(),
            version: "0.2.0".to_string(),
            url: "file:///does/not/matter".to_string(),
            sha256: None,
            signature: None,
            artifact_id: None,
        };
        let comp = fixture_component(
            "agentsight",
            Some(artifact_no_sha),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::MissingChecksum { ref component } => assert_eq!(component, "agentsight"),
            other => panic!("expected MissingChecksum, got {other:?}"),
        }

        // No file installed, no state file written.
        assert!(!dest.exists(), "no file may be installed without sha256");
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "no state file may be created when the executor refuses to install",
        );

        // Started + failed audit records.
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// Regression for the snapshot+restore path: when `state.save()` fails
    /// AND there is a pre-op `installed.toml`, the prior file must remain
    /// intact (cleanup must not delete it). We force the save failure by
    /// pre-creating `installed.toml`'s `.tmp` sibling as a *directory* so
    /// `fs::write(&tmp, ...)` inside `InstalledState::save` errors out
    /// before the rename.
    ///
    /// The prior state is built with `InstalledState::default().save(...)`
    /// so the file is a *real* serialized state (not just a TOML comment).
    /// That way `InstalledState::load` actually parses a populated-shape
    /// document and the test exercises the real cleanup path — losing
    /// the prior state of an existing install is the worst-case failure
    /// for a package manager and is what this regression locks down.
    #[test]
    fn state_save_failure_restores_prior_installed_toml() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"new-agentsight-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        // Build a valid prior installed.toml using the real serializer
        // and snapshot its bytes; that snapshot is what cleanup must
        // restore byte-for-byte after the failed save.
        let state_path = layout.state_dir.join("installed.toml");
        std_fs::create_dir_all(&layout.state_dir).unwrap();
        InstalledState::default()
            .save(&state_path)
            .expect("prior state save");
        let prior_bytes = std_fs::read(&state_path).expect("read prior bytes");

        // Trip InstalledState::save by squatting on its tmp sibling path.
        let tmp_squat = layout.state_dir.join(".installed.toml.tmp");
        std_fs::create_dir_all(&tmp_squat).unwrap();

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must fail at state.save");
        assert!(
            matches!(err, ExecuteError::State { .. }),
            "unexpected error: {err:?}",
        );

        // Prior installed.toml content is unchanged byte-for-byte.
        let after = std_fs::read(&state_path).expect("installed.toml still readable");
        assert_eq!(
            after, prior_bytes,
            "cleanup must restore the prior installed.toml byte-for-byte",
        );
        // Belt-and-suspenders: the restored bytes still parse as a valid
        // InstalledState — proof we did not leave a truncated/garbled file.
        let _: InstalledState = InstalledState::load(&state_path).expect("prior state reparses");

        // Installed binary was unlinked.
        assert!(!dest.exists(), "cleanup must unlink installed files");
        // A failed audit record was appended.
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// Same trip, but with NO pre-existing `installed.toml`. Cleanup must
    /// remove any state file this op wrote (here state.save fails before
    /// the rename so nothing is on disk anyway — the assertion is "still
    /// nothing", confirming the `None` snapshot branch is a no-op rather
    /// than accidentally writing an empty file).
    #[test]
    fn state_save_failure_no_prior_state_leaves_no_installed_toml() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"new-agentsight-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        std_fs::create_dir_all(&layout.state_dir).unwrap();
        let tmp_squat = layout.state_dir.join(".installed.toml.tmp");
        std_fs::create_dir_all(&tmp_squat).unwrap();

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must fail at state.save");
        assert!(
            matches!(err, ExecuteError::State { .. }),
            "unexpected error: {err:?}",
        );
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "no installed.toml may leak from a failed first-time enable",
        );
        assert!(!dest.exists());
    }
}
