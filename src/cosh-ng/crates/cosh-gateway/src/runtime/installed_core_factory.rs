//! Trusted admission and pinned launch policy for closed cosh-core profiles.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use cosh_gateway_contracts::{
    common::{BoundedName, Digest, TargetRef},
    error::{ContractError, ErrorCategory},
    ids::{AgentSessionId, InstallationId, RuntimeBindingId, RuntimeInstanceId},
    profile::GatewayCapabilityProfile,
};
use sha2::{Digest as _, Sha256};

use crate::daemon::ScheduledRun;

use super::{
    AgentRuntimePortFactory, CoshCoreBridge, CoshCoreBridgeConfig, CoshCoreBridgeIdentity,
    CoshCoreBrokeredContext, LocalOsActorResolver, PinnedExecutable, RuntimeLaunchSpec,
    ScheduledRuntimePort, TrustedWorkspaceResolver,
};

const MAX_CORE_FRAME_BYTES: usize = 1024 * 1024;
/// Exact Runtime selector profile for the installed task-only Core boundary.
pub const GATEWAY_BROKERED_CORE_RUNTIME_PROFILE: &str = "gateway-brokered-v1";
const ALLOWED_ENVIRONMENT: &[&str] = &[
    "HOME",
    "PATH",
    "TMPDIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_STATE_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_ORGANIZATION",
    "OPENAI_PROJECT",
    "DASHSCOPE_API_KEY",
];

/// Canonical Core launch inputs pinned before daemon socket admission.
pub struct ResolvedBrokeredCoreRuntimeProfile {
    executable: PinnedExecutable,
    environment: BTreeMap<OsString, OsString>,
}

impl fmt::Debug for ResolvedBrokeredCoreRuntimeProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedBrokeredCoreRuntimeProfile")
            .field("executable", &self.executable)
            .field("environment_names", &self.environment.keys())
            .finish()
    }
}

/// Production factory for one admitted Gateway-brokered Core profile.
///
/// The capability profile and its target are fixed by trusted daemon
/// configuration before any ScheduledRun can reach this factory.
pub struct InstalledBrokeredCoreRuntimePortFactory {
    installation_id: InstallationId,
    actors: LocalOsActorResolver,
    workspaces: TrustedWorkspaceResolver,
    capability_profile: GatewayCapabilityProfile,
    target: TargetRef,
    executable: PinnedExecutable,
    environment: BTreeMap<OsString, OsString>,
    #[cfg(test)]
    test_script: Option<PathBuf>,
}

impl fmt::Debug for InstalledBrokeredCoreRuntimePortFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstalledBrokeredCoreRuntimePortFactory")
            .field("installation_id", &self.installation_id)
            .field("actors", &self.actors)
            .field("workspaces", &self.workspaces)
            .field("capability_profile", &self.capability_profile.identity())
            .field("target", &self.target)
            .field("executable", &self.executable)
            .field("environment_names", &self.environment.keys())
            .finish()
    }
}

impl InstalledBrokeredCoreRuntimePortFactory {
    /// Resolves and pins the installed executable without binding daemon state.
    ///
    /// # Errors
    ///
    /// Rejects relative or unavailable executables, a non-`cosh-core`
    /// configured entry, or a non-executable canonical target.
    pub fn resolve(
        executable: impl AsRef<Path>,
        environment: BTreeMap<OsString, OsString>,
    ) -> Result<ResolvedBrokeredCoreRuntimeProfile, ContractError> {
        let configured = executable.as_ref();
        if !configured.is_absolute()
            || configured.file_name().and_then(OsStr::to_str) != Some("cosh-core")
        {
            return Err(profile_error());
        }
        let executable = PinnedExecutable::pin_native(configured).map_err(|_| profile_error())?;
        let environment = environment
            .into_iter()
            .filter(|(name, _)| {
                name.to_str()
                    .is_some_and(|name| ALLOWED_ENVIRONMENT.contains(&name))
            })
            .collect();
        Ok(ResolvedBrokeredCoreRuntimeProfile {
            executable,
            environment,
        })
    }

    /// Binds a pre-resolved launch profile to this daemon installation.
    ///
    /// # Errors
    ///
    /// Rejects an actor resolver issued by another installation or a target
    /// that is not the exact target sealed by the admitted profile.
    pub fn from_resolved(
        installation_id: InstallationId,
        actors: LocalOsActorResolver,
        workspaces: TrustedWorkspaceResolver,
        capability_profile: GatewayCapabilityProfile,
        target: TargetRef,
        profile: ResolvedBrokeredCoreRuntimeProfile,
    ) -> Result<Self, ContractError> {
        if actors.installation_id() != &installation_id
            || target != capability_profile.governed_target()
            || workspaces.resolve(&target).is_err()
        {
            return Err(profile_error());
        }
        Ok(Self {
            installation_id,
            actors,
            workspaces,
            capability_profile,
            target,
            executable: profile.executable,
            environment: profile.environment,
            #[cfg(test)]
            test_script: None,
        })
    }

    /// Pins an installed `cosh-core` executable and filters its environment.
    ///
    /// # Errors
    ///
    /// Rejects installation mismatches, relative or unavailable executables,
    /// a non-`cosh-core` configured entry, or a non-executable canonical target.
    pub fn new(
        installation_id: InstallationId,
        actors: LocalOsActorResolver,
        workspaces: TrustedWorkspaceResolver,
        capability_profile: GatewayCapabilityProfile,
        target: TargetRef,
        executable: impl AsRef<Path>,
        environment: BTreeMap<OsString, OsString>,
    ) -> Result<Self, ContractError> {
        let profile = Self::resolve(executable, environment)?;
        Self::from_resolved(
            installation_id,
            actors,
            workspaces,
            capability_profile,
            target,
            profile,
        )
    }
}

impl AgentRuntimePortFactory for InstalledBrokeredCoreRuntimePortFactory {
    fn create(&mut self, run: &ScheduledRun) -> Result<ScheduledRuntimePort, ContractError> {
        if self
            .capability_profile
            .verify_identity(&run.capability_profile)
            .is_err()
            || run.target != self.target
            || run.runtime.runtime.as_str() != "core"
            || run.runtime.profile.as_ref().map(BoundedName::as_str)
                != Some(GATEWAY_BROKERED_CORE_RUNTIME_PROFILE)
        {
            return Err(profile_error());
        }
        let actor = self.actors.resolve(&run.actor)?;
        let workspace = self.workspaces.resolve(&run.target)?;
        if workspace.reference() != &run.workspace {
            return Err(contract_error(
                "runtime_workspace_mismatch",
                ErrorCategory::Unauthorized,
                false,
                "The admitted Runtime workspace no longer matches trusted configuration",
            ));
        }

        let mut launch = RuntimeLaunchSpec::from_pinned(
            self.executable.clone(),
            workspace.pinned_directory().clone(),
        );
        #[cfg(test)]
        if let Some(script) = &self.test_script {
            launch.arguments.push(script.as_os_str().to_owned());
        }
        launch.arguments.extend(
            [
                "--headless",
                "--execution-profile",
                GATEWAY_BROKERED_CORE_RUNTIME_PROFILE,
                "--capability-profile",
                self.capability_profile.id().as_str(),
            ]
            .into_iter()
            .map(OsString::from),
        );
        launch.environment.clone_from(&self.environment);
        launch.stdout_line_limit = MAX_CORE_FRAME_BYTES;

        let identity = CoshCoreBridgeIdentity {
            installation_id: self.installation_id.clone(),
            actor_id: Some(actor.actor_id.clone()),
            task_id: run.task_id.clone(),
            run_id: run.run_id.clone(),
            agent_session_id: AgentSessionId::new(),
            binding_id: RuntimeBindingId::new(),
            runtime_instance_id: RuntimeInstanceId::new(),
            runtime_generation: run.lease_generation,
            provider_authority: static_name("cosh-core"),
            provider_scope_digest: scope_digest(
                &self.installation_id,
                &self.executable,
                workspace.pinned_directory(),
                self.capability_profile,
                &self.target,
            ),
        };
        let config = CoshCoreBridgeConfig::new(launch, workspace.reference().clone(), identity)
            .gateway_brokered(CoshCoreBrokeredContext {
                actor,
                target: run.target.clone(),
                capability_profile: self.capability_profile,
            });
        let port = CoshCoreBridge::launch(config).map_err(|_| {
            contract_error(
                "runtime_launch_failed",
                ErrorCategory::RuntimeUnavailable,
                true,
                "The installed brokered Core Runtime could not be launched",
            )
        })?;
        Ok(ScheduledRuntimePort::new(
            Box::new(port),
            workspace.reference().clone(),
        ))
    }
}

fn scope_digest(
    installation: &InstallationId,
    executable: &PinnedExecutable,
    workspace: &super::PinnedDirectory,
    capability_profile: GatewayCapabilityProfile,
    target: &TargetRef,
) -> Digest {
    let mut digest = Sha256::new();
    let profile_manifest_digest = capability_profile.manifest_digest();
    for part in [
        b"cosh.gateway-brokered-core.scope.v2".as_slice(),
        installation.as_str().as_bytes(),
        path_bytes(executable.canonical_path()),
        path_bytes(workspace.canonical_path()),
        capability_profile.id().as_str().as_bytes(),
        profile_manifest_digest.as_str().as_bytes(),
        target.kind.as_str().as_bytes(),
        target.authority.as_str().as_bytes(),
        target.identifier.as_str().as_bytes(),
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    for identity in [executable.identity(), workspace.identity()] {
        digest.update(identity.device().to_be_bytes());
        digest.update(identity.inode().to_be_bytes());
    }
    Digest::parse(format!("{:x}", digest.finalize()))
        .unwrap_or_else(|_| unreachable!("SHA-256 output must remain canonical"))
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().to_str().unwrap_or_default().as_bytes()
}

fn static_name(value: &'static str) -> BoundedName {
    BoundedName::new(value).unwrap_or_else(|_| unreachable!("static Runtime name must be bounded"))
}

fn profile_error() -> ContractError {
    contract_error(
        "runtime_profile_invalid",
        ErrorCategory::InvalidRequest,
        false,
        "The selected installed Runtime profile is invalid",
    )
}

fn contract_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ContractError {
    ContractError::new(code, category, retryable, message)
        .unwrap_or_else(|_| unreachable!("static Runtime error must remain bounded"))
}

#[cfg(test)]
#[path = "installed_core_factory/tests.rs"]
mod tests;
