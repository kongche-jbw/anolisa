//! Private brokered-profile handshake values owned by the Core binary.
//!
//! Gateway remains authoritative for admission. Core mirrors only the closed
//! private wire values it must reject before accepting a brokered user turn.

use serde::{Deserialize, Serialize};

const TASK_ONLY_V1_PROFILE: &str = "task-only-v1";
const TASK_ONLY_V1_MANIFEST_DIGEST: &str =
    "2b95e0f3e28df8eb2b7930f2dec3650ffe399f971671c971865e4663c382c94a";
const TASK_ONLY_V1_RUNTIME_TOOLS: &[&str] = &["ask_user_question"];
const WORKSPACE_CHECKPOINT_V1_PROFILE: &str = "workspace-checkpoint-v1";
const WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST: &str =
    "6b3e7093e7b8656d4a7cf21faa85b9eed761ef415d002623cfc442f3ef3c8ae1";
const WORKSPACE_CHECKPOINT_V1_RUNTIME_TOOLS: &[&str] =
    &["ask_user_question", "workspace_checkpoint_create"];

/// Closed capability inventory selected before a brokered Core process starts.
#[derive(clap::ValueEnum, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum BrokeredCapabilityProfile {
    /// Side-effect-free question handling only.
    #[default]
    TaskOnlyV1,
    /// Adds the Gateway-hosted workspace checkpoint declaration.
    WorkspaceCheckpointV1,
}

impl BrokeredCapabilityProfile {
    /// Returns the exact private-wire identity for this launch selection.
    pub(crate) fn identity(self) -> BrokeredCapabilityProfileIdentity {
        match self {
            Self::TaskOnlyV1 => BrokeredCapabilityProfileIdentity::task_only_v1(),
            Self::WorkspaceCheckpointV1 => {
                BrokeredCapabilityProfileIdentity::workspace_checkpoint_v1()
            }
        }
    }

    /// Verifies the Gateway-requested identity against the launch selection.
    pub(crate) fn verify_identity(
        self,
        identity: &BrokeredCapabilityProfileIdentity,
    ) -> Result<(), &'static str> {
        match self {
            Self::TaskOnlyV1 => identity.verify_task_only_v1(),
            Self::WorkspaceCheckpointV1 => identity.verify_workspace_checkpoint_v1(),
        }
    }

    /// Verifies that Core constructed exactly the inventory sealed by the profile.
    pub(crate) fn verify_runtime_tools(self, actual: &[String]) -> Result<(), &'static str> {
        match self {
            Self::TaskOnlyV1 => verify_task_only_runtime_tools(actual),
            Self::WorkspaceCheckpointV1 => verify_workspace_checkpoint_runtime_tools(actual),
        }
    }

    /// Returns whether the closed inventory includes the hosted checkpoint tool.
    pub(crate) const fn hosts_checkpoint(self) -> bool {
        matches!(self, Self::WorkspaceCheckpointV1)
    }

    /// Returns the immutable runtime-generation label for diagnostics.
    pub(crate) const fn runtime_label(self) -> &'static str {
        match self {
            Self::TaskOnlyV1 => "gateway-brokered-v1/task-only-v1",
            Self::WorkspaceCheckpointV1 => "gateway-brokered-v1/workspace-checkpoint-v1",
        }
    }
}

/// Profile identity carried by the private brokered Core wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokeredCapabilityProfileIdentity {
    /// Exact versioned profile name selected by Gateway.
    pub profile_id: String,
    /// Exact canonical manifest digest selected by Gateway.
    pub manifest_digest: String,
}

impl BrokeredCapabilityProfileIdentity {
    /// Returns the only capability profile understood by private Core v3.
    pub fn task_only_v1() -> Self {
        Self {
            profile_id: TASK_ONLY_V1_PROFILE.to_owned(),
            manifest_digest: TASK_ONLY_V1_MANIFEST_DIGEST.to_owned(),
        }
    }

    /// Returns the closed checkpoint capability identity understood by private Core v3.
    pub fn workspace_checkpoint_v1() -> Self {
        Self {
            profile_id: WORKSPACE_CHECKPOINT_V1_PROFILE.to_owned(),
            manifest_digest: WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST.to_owned(),
        }
    }

    /// Verifies that the requested identity is the closed private v3 profile.
    pub fn verify_task_only_v1(&self) -> Result<(), &'static str> {
        if self.profile_id != TASK_ONLY_V1_PROFILE {
            return Err("capability profile identity does not match the brokered profile");
        }
        if self.manifest_digest != TASK_ONLY_V1_MANIFEST_DIGEST {
            return Err("capability profile manifest digest does not match the brokered profile");
        }
        Ok(())
    }

    /// Verifies that the requested identity is the closed checkpoint profile.
    pub fn verify_workspace_checkpoint_v1(&self) -> Result<(), &'static str> {
        if self.profile_id != WORKSPACE_CHECKPOINT_V1_PROFILE {
            return Err("capability profile identity does not match the brokered profile");
        }
        if self.manifest_digest != WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST {
            return Err("capability profile manifest digest does not match the brokered profile");
        }
        Ok(())
    }
}

/// Verifies the actual Core inventory against the private v3 task-only profile.
pub fn verify_task_only_runtime_tools(actual: &[String]) -> Result<(), &'static str> {
    if actual
        .iter()
        .map(String::as_str)
        .eq(TASK_ONLY_V1_RUNTIME_TOOLS.iter().copied())
    {
        Ok(())
    } else {
        Err("Runtime tool inventory does not match the brokered capability profile")
    }
}

/// Verifies the actual Core inventory against the checkpoint capability profile.
pub fn verify_workspace_checkpoint_runtime_tools(actual: &[String]) -> Result<(), &'static str> {
    if actual
        .iter()
        .map(String::as_str)
        .eq(WORKSPACE_CHECKPOINT_V1_RUNTIME_TOOLS.iter().copied())
    {
        Ok(())
    } else {
        Err("Runtime tool inventory does not match the brokered capability profile")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_only_identity_and_inventory_are_exact() {
        let identity = BrokeredCapabilityProfileIdentity::task_only_v1();
        assert_eq!(identity.profile_id, "task-only-v1");
        assert_eq!(identity.manifest_digest, TASK_ONLY_V1_MANIFEST_DIGEST);
        assert_eq!(identity.verify_task_only_v1(), Ok(()));
        assert_eq!(
            verify_task_only_runtime_tools(&["ask_user_question".to_owned()]),
            Ok(())
        );
    }

    #[test]
    fn task_only_identity_and_inventory_reject_drift() {
        let mut identity = BrokeredCapabilityProfileIdentity::task_only_v1();
        identity.manifest_digest = "0".repeat(64);
        assert!(identity.verify_task_only_v1().is_err());
        assert!(verify_task_only_runtime_tools(&[]).is_err());
        assert!(verify_task_only_runtime_tools(&[
            "ask_user_question".to_owned(),
            "shell".to_owned(),
        ])
        .is_err());
    }

    #[test]
    fn checkpoint_identity_inventory_and_launch_selection_are_exact() {
        let profile = BrokeredCapabilityProfile::WorkspaceCheckpointV1;
        let identity = BrokeredCapabilityProfileIdentity::workspace_checkpoint_v1();
        let tools = vec![
            "ask_user_question".to_owned(),
            "workspace_checkpoint_create".to_owned(),
        ];

        assert_eq!(profile.identity(), identity);
        assert!(profile.hosts_checkpoint());
        assert_eq!(profile.verify_identity(&identity), Ok(()));
        assert_eq!(profile.verify_runtime_tools(&tools), Ok(()));
        assert!(profile
            .verify_identity(&BrokeredCapabilityProfileIdentity::task_only_v1())
            .is_err());
        assert!(profile
            .verify_runtime_tools(&[tools[1].clone(), tools[0].clone()])
            .is_err());
    }
}
