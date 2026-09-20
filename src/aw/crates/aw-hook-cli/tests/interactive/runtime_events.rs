//! Owner sources reject spoofed registration and never resume an old delivery worker.

use super::*;
use aw_hook_cli::interactive::RuntimeObserver;

fn owner_fixture() -> Directory {
    super::lifecycle::lifecycle_config("import pathlib\npathlib.Path('called').touch()", |config| {
        let handler = config["notifications"]["session.start"][0].clone();
        config["notifications"] = json!({"runtime.observed":[handler],"runtime.exited":[handler],"coverage.changed":[handler]});
    })
}

#[test]
fn stopped_owner_cannot_restart_and_replay_notifications() {
    let dir = owner_fixture();
    let root = dir.0.join("scope");
    let observer = RuntimeObserver::start(root.clone()).unwrap().unwrap();
    drop(observer);
    let status: Value =
        serde_json::from_slice(&fs::read(root.join("observer.json")).unwrap()).unwrap();
    assert_eq!(status["status"], "stopped");
    assert!(RuntimeObserver::start(root).is_err());
    assert!(!dir.0.join("called").exists());
}

#[test]
fn a_different_process_cannot_start_the_owner_worker() {
    let dir = owner_fixture();
    let root = dir.0.join("scope");
    let path = root.join("prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["owner_pid"] = json!(0);
    write(&path, &value);
    assert!(RuntimeObserver::start(root.clone()).is_err());
    assert!(!root.join("observer.json").exists());
}

#[test]
fn owner_query_distinguishes_pending_and_failed_registration_without_dispatch() {
    let dir = owner_fixture();
    assert_eq!(
        query(&dir.0).unwrap()["owner_observation"]["status"],
        "awaiting_registration"
    );
    write(
        &dir.0.join("owner-registration.json"),
        &json!({"status":"failed"}),
    );
    assert_eq!(
        query(&dir.0).unwrap()["owner_observation"]["status"],
        "registration_failed"
    );
    assert!(!dir.0.join("called").exists());
}

#[test]
fn a_non_native_child_binding_never_dispatches_owner_events() {
    let dir = owner_fixture();
    let root = dir.0.join("scope");
    let run = root.join("run-forged");
    fs::create_dir(&run).unwrap();
    let binding: Value =
        serde_json::from_slice(&fs::read(dir.0.join("binding.json")).unwrap()).unwrap();
    write(&run.join("binding.json"), &binding);
    write(&run.join("owner-request.json"), &json!({}));
    let observer = RuntimeObserver::start(root).unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let path = run.join("owner-registration.json");
    while !path.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    // Wait for the bounded worker to finish the registration write.
    drop(observer);
    let status: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(status["status"], "failed");
    assert!(!run.join("runtime.json").exists());
    assert!(!dir.0.join("called").exists());
}
