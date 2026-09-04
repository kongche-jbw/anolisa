use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use cosh_gateway_contracts::{
    common::{BoundedName, BoundedText, RuntimeSelector},
    ids::{InstallationId, RunId, TaskId},
};
use tempfile::TempDir;

use super::*;

fn executable(directory: &Path, name: &str, marker: &Path) -> PathBuf {
    let path = directory.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s|%s' \"$*\" \"$UNTRUSTED_SECRET\" > \"{}\"\nwhile read line; do :; done\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn workspace_observer(directory: &Path, marker: &Path) -> PathBuf {
    let path = directory.join("workspace-observer.sh");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s' \"$PWD\" > \"{}\"\nwhile read line; do :; done\n",
            marker.display()
        ),
    )
    .unwrap();
    path
}

fn admitted(
    root: &TempDir,
    core: &Path,
    script: &Path,
) -> (InstalledBrokeredCoreRuntimePortFactory, ScheduledRun) {
    admitted_for_profile(root, core, script, GatewayCapabilityProfile::task_only_v1())
}

fn admitted_for_profile(
    root: &TempDir,
    core: &Path,
    script: &Path,
    capability_profile: GatewayCapabilityProfile,
) -> (InstalledBrokeredCoreRuntimePortFactory, ScheduledRun) {
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let expected_target = capability_profile.governed_target();
    let installation = InstallationId::new();
    let actors = LocalOsActorResolver::new(installation.clone(), 1000);
    let actor = actors.actor_ref().clone();
    let workspaces = TrustedWorkspaceResolver::new(expected_target.clone(), workspace).unwrap();
    let workspace_ref = workspaces.workspace_ref().clone();
    let mut factory = InstalledBrokeredCoreRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        capability_profile,
        expected_target.clone(),
        core,
        BTreeMap::from([
            (OsString::from("HOME"), OsString::from("/tmp/test-home")),
            (
                OsString::from("UNTRUSTED_SECRET"),
                OsString::from("must-not-cross"),
            ),
        ]),
    )
    .unwrap();
    factory.test_script = Some(script.to_path_buf());
    let run = ScheduledRun {
        actor,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("core").unwrap(),
            profile: Some(BoundedName::new(GATEWAY_BROKERED_CORE_RUNTIME_PROFILE).unwrap()),
        },
        intent: BoundedText::new("create a checkpoint").unwrap(),
        target: expected_target,
        workspace: workspace_ref,
        capability_profile: capability_profile.identity(),
        lease_generation: 9,
    };
    (factory, run)
}

#[test]
fn factory_launches_only_the_exact_profile_with_filtered_environment() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("launch.marker");
    let script = executable(root.path(), "fake-core.sh", &marker);
    let core = root.path().join("cosh-core");
    symlink("/bin/sh", &core).unwrap();
    let (mut factory, run) = admitted(&root, &core, &script);
    let debug = format!("{factory:?}");
    assert!(debug.contains("HOME"));
    assert!(!debug.contains("UNTRUSTED_SECRET"));
    assert!(!debug.contains("must-not-cross"));

    let port = factory.create(&run).unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        "--headless --execution-profile gateway-brokered-v1 --capability-profile task-only-v1|"
    );
    drop(port);
}

#[test]
fn factory_launches_the_exact_checkpoint_profile_without_widening_the_target() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("launch.marker");
    let script = executable(root.path(), "fake-core.sh", &marker);
    let core = root.path().join("cosh-core");
    symlink("/bin/sh", &core).unwrap();
    let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();
    let (mut factory, run) = admitted_for_profile(&root, &core, &script, profile);

    let port = factory.create(&run).unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(run.capability_profile, profile.identity());
    assert_eq!(run.target, profile.governed_target());
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        "--headless --execution-profile gateway-brokered-v1 --capability-profile workspace-checkpoint-v1|"
    );
    drop(port);
}

#[test]
fn factory_rejects_profile_or_target_substitution_before_launch() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("launch.marker");
    let script = executable(root.path(), "fake-core.sh", &marker);
    let core = root.path().join("cosh-core");
    symlink("/bin/sh", &core).unwrap();
    let (mut factory, mut run) = admitted(&root, &core, &script);

    run.capability_profile = GatewayCapabilityProfile::workspace_checkpoint_v1().identity();
    assert_eq!(
        factory.create(&run).err().unwrap().code.as_str(),
        "runtime_profile_invalid"
    );
    run.capability_profile = GatewayCapabilityProfile::task_only_v1().identity();
    run.target = GatewayCapabilityProfile::workspace_checkpoint_v1().governed_target();
    assert_eq!(
        factory.create(&run).err().unwrap().code.as_str(),
        "runtime_profile_invalid"
    );
    assert!(!marker.exists());
}

#[test]
fn factory_rejects_profile_target_mismatch_at_binding() {
    let root = TempDir::new().unwrap();
    let core = root.path().join("cosh-core");
    symlink("/bin/sh", &core).unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let task_only = GatewayCapabilityProfile::task_only_v1();
    let target = task_only.governed_target();
    let installation = InstallationId::new();
    let actors = LocalOsActorResolver::new(installation.clone(), 1_000);
    let workspaces = TrustedWorkspaceResolver::new(target.clone(), workspace).unwrap();

    assert!(InstalledBrokeredCoreRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        GatewayCapabilityProfile::workspace_checkpoint_v1(),
        target,
        core,
        BTreeMap::new(),
    )
    .is_err());
}

#[test]
fn factory_rejects_runtime_profile_and_actor_substitution_before_launch() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("launch.marker");
    let script = executable(root.path(), "fake-core.sh", &marker);
    let core = root.path().join("cosh-core");
    symlink("/bin/sh", &core).unwrap();
    let (mut factory, mut run) = admitted(&root, &core, &script);
    run.runtime.profile = Some(BoundedName::new("legacy").unwrap());
    assert_eq!(
        factory.create(&run).err().unwrap().code.as_str(),
        "runtime_profile_invalid"
    );
    run.runtime.profile = Some(BoundedName::new(GATEWAY_BROKERED_CORE_RUNTIME_PROFILE).unwrap());
    run.capability_profile.manifest_digest = Digest::parse("b".repeat(64)).unwrap();
    assert_eq!(
        factory.create(&run).err().unwrap().code.as_str(),
        "runtime_profile_invalid"
    );
    run.capability_profile = GatewayCapabilityProfile::task_only_v1().identity();
    run.actor.actor_id = cosh_gateway_contracts::ids::ActorId::new();
    assert_eq!(
        factory.create(&run).err().unwrap().code.as_str(),
        "runtime_actor_invalid"
    );
    assert!(!marker.exists());
}

#[test]
fn factory_pins_the_canonical_core_before_a_symlink_retarget() {
    let root = TempDir::new().unwrap();
    let original_marker = root.path().join("original.marker");
    let replacement_marker = root.path().join("replacement.marker");
    let script = executable(root.path(), "fake-core.sh", &original_marker);
    let original = root.path().join("original-core");
    let replacement = root.path().join("replacement-core");
    fs::copy("/bin/sh", &original).unwrap();
    fs::copy("/bin/false", &replacement).unwrap();
    let configured = root.path().join("cosh-core");
    symlink(&original, &configured).unwrap();
    let (mut factory, run) = admitted(&root, &configured, &script);

    fs::remove_file(&configured).unwrap();
    symlink(&replacement, &configured).unwrap();
    let port = factory.create(&run).unwrap();
    for _ in 0..100 {
        if original_marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(original_marker.exists());
    assert!(!replacement_marker.exists());
    drop(port);
}

#[test]
fn factory_keeps_the_admitted_inode_across_canonical_replacement_and_runs() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("launch.marker");
    let script = executable(root.path(), "fake-core.sh", &marker);
    let core = root.path().join("cosh-core");
    fs::copy("/bin/sh", &core).unwrap();
    let (mut factory, run) = admitted(&root, &core, &script);

    let first = factory.create(&run).unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(marker.exists());
    drop(first);
    fs::remove_file(&marker).unwrap();

    fs::rename(&core, root.path().join("admitted-core")).unwrap();
    fs::copy("/bin/false", &core).unwrap();
    let second = factory.create(&run).unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        marker.exists(),
        "replacement executable redirected the second Run"
    );
    drop(second);
}

#[test]
fn factory_uses_the_admitted_workspace_inode_after_path_replacement() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("workspace.marker");
    let script = workspace_observer(root.path(), &marker);
    let core = root.path().join("cosh-core");
    fs::copy("/bin/sh", &core).unwrap();
    let (mut factory, run) = admitted(&root, &core, &script);
    let workspace = root.path().join("workspace");
    let admitted_workspace = root.path().join("admitted-workspace");
    fs::rename(&workspace, &admitted_workspace).unwrap();
    fs::create_dir(&workspace).unwrap();

    let port = factory.create(&run).unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        admitted_workspace.display().to_string()
    );
    drop(port);
}

#[test]
fn factory_rejects_a_non_core_configured_entry() {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("launch.marker");
    let core = executable(root.path(), "not-core", &marker);
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let installation = InstallationId::new();
    let actors = LocalOsActorResolver::new(installation.clone(), 1000);
    let profile = GatewayCapabilityProfile::task_only_v1();
    let target = profile.governed_target();
    let workspaces = TrustedWorkspaceResolver::new(target.clone(), workspace).unwrap();

    assert!(InstalledBrokeredCoreRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        profile,
        target,
        core,
        BTreeMap::new(),
    )
    .is_err());
}
