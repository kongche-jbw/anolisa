//! Shell-owned pidfd observations; native Bash remains the only wait/reap owner.

use super::{evidence, notifications::Host, storage, Binding, Config, Error, Prepared, State};
use aw_contracts::events::{EventName, Notification};
use aw_core::{journal::FileJournal, ports::Cancellation, Core};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) fn owns(event: EventName) -> bool {
    matches!(
        event,
        EventName::RuntimeObserved | EventName::RuntimeExited | EventName::CoverageChanged
    )
}

pub(super) fn configured(config: &Config) -> bool {
    config.format == 2
        && config
            .notifications
            .as_ref()
            .is_some_and(|routes| routes.keys().any(|e| owns(*e)))
}

/// Shell-owned source for runtime registration, root exit and sampled coverage.
///
/// This Linux worker opens a pidfd before native exec and never calls waitpid.
/// Drop cancels handlers and joins the worker. It does not survive owner death,
/// infer exit codes, attest Agent readiness or reclaim native descendants.
pub struct RuntimeObserver {
    cancelled: Arc<AtomicBool>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RuntimeObserver {
    /// Starts a bounded owner worker only when an owner event route is configured.
    ///
    /// # Errors
    /// Rejects a different owner, unavailable pidfds or a failed worker spawn.
    pub fn start(root: PathBuf) -> Result<Option<Self>, Error> {
        let prepared: Prepared = storage::read(&root.join("prepared.json"))?;
        if !configured(&prepared.config) {
            return Ok(None);
        }
        if prepared.owner_pid != std::process::id()
            || crate::process_identity(prepared.owner_pid)
                .map_err(evidence)?
                .1
                != prepared.owner_ticks
        {
            return Err(Error::Profile(
                "runtime observer requires the preparing owner",
            ));
        }
        drop(pidfd(prepared.owner_pid)?);
        // Creation is exclusive: restarting the worker must not replay deliveries.
        storage::create(&root.join("observer.json"), &json!({"status":"active"}))?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = Stop(cancelled.clone());
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("aw-runtime".into())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(86_400);
                let mut seen = HashSet::new();
                let mut runs: Vec<Run> = Vec::new();
                let result = (|| {
                    for _ in 0..864_000 {
                        if cancellation.is_cancelled() || Instant::now() >= deadline {
                            break;
                        }
                        for entry in fs::read_dir(&root)?.take(134) {
                            let path = entry?.path();
                            if !path
                                .file_name()
                                .is_some_and(|n| n.to_string_lossy().starts_with("run-"))
                                || seen.contains(&path)
                                || !path.join("owner-request.json").try_exists()?
                            {
                                continue;
                            }
                            if seen.len() >= 128 {
                                return Err(Error::Profile("runtime observer capacity exceeded"));
                            }
                            seen.insert(path.clone());
                            match Run::register(&path, &prepared) {
                                Ok(run) => runs.push(run),
                                Err(_) => {
                                    storage::replace(
                                        &path.join("owner-registration.json"),
                                        &json!({"status":"failed"}),
                                    )?;
                                }
                            }
                        }
                        for run in &mut runs {
                            if !run.finished && run.tick(&cancellation).is_err() {
                                run.finished = true;
                                run.record["status"] = json!("observation_failed");
                                run.record["delivery_gap"] = json!(true);
                                let _ =
                                    storage::replace(&run.root.join("runtime.json"), &run.record);
                            }
                        }
                        runs.retain(|run| !run.finished);
                        match receiver.recv_timeout(Duration::from_millis(100)) {
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            _ => break,
                        }
                    }
                    Ok::<_, Error>(())
                })();
                let status = if result.is_err() {
                    "failed"
                } else if cancellation.is_cancelled() {
                    "stopped"
                } else {
                    "expired"
                };
                let _ = storage::replace(&root.join("observer.json"), &json!({"status":status}));
            })?;
        Ok(Some(Self {
            cancelled,
            stop,
            worker: Some(worker),
        }))
    }
}

impl Drop for RuntimeObserver {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Stop(Arc<AtomicBool>);
impl Cancellation for Stop {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub(super) fn await_registration(root: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cancelled() && Instant::now() < deadline {
        let path = root.join("owner-registration.json");
        if path.try_exists()? {
            let value: Value = storage::read(&path)?;
            return if value["status"] == "registered" {
                Ok(())
            } else {
                Err(Error::Profile("runtime listener registration failed"))
            };
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(Error::Profile(
        "runtime listener registration cancelled or timed out",
    ))
}

#[cfg(target_os = "linux")]
fn pidfd(pid: u32) -> Result<OwnedFd, Error> {
    use std::os::fd::FromRawFd;
    // pidfd_open has no borrowed memory and returns a new CLOEXEC descriptor.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // The successful syscall transfers sole descriptor ownership here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

#[cfg(not(target_os = "linux"))]
fn pidfd(_: u32) -> Result<OwnedFd, Error> {
    Err(Error::Profile("runtime observation requires Linux pidfds"))
}

struct Run {
    root: PathBuf,
    binding: Binding,
    fd: OwnedFd,
    record: Value,
    coverage: Option<Value>,
    sequence: u64,
    observed: bool,
    finished: bool,
}

impl Run {
    fn register(root: &Path, prepared: &Prepared) -> Result<Self, Error> {
        let binding: Binding = storage::read(&root.join("binding.json"))?;
        if binding.prepared.owner_pid != prepared.owner_pid
            || binding.prepared.owner_ticks != prepared.owner_ticks
            || binding.prepared.revision != prepared.revision
            || serde_json::to_value(&binding.prepared.config).map_err(evidence)?
                != serde_json::to_value(&prepared.config).map_err(evidence)?
        {
            return Err(Error::Profile(
                "runtime binding differs from owner configuration",
            ));
        }
        let (shell, ticks) = crate::process_identity(binding.agent_pid).map_err(evidence)?;
        if ticks != binding.agent_ticks
            || crate::process_identity(shell).map_err(evidence)?.0 != prepared.owner_pid
        {
            return Err(Error::Profile("runtime is not a direct native shell child"));
        }
        let fd = pidfd(binding.agent_pid)?;
        if crate::process_identity(binding.agent_pid)
            .map_err(evidence)?
            .1
            != ticks
        {
            return Err(Error::Profile(
                "runtime identity changed while opening pidfd",
            ));
        }
        let record = json!({"status":"registered","root_exited":false,"exit_status":null,
            "agent_pid":binding.agent_pid,"agent_start_ticks":ticks,
            "delivery_gap":false,"coverage_sequence":0});
        storage::create(&root.join("runtime.json"), &record)?;
        storage::replace(
            &root.join("owner-registration.json"),
            &json!({"status":"registered"}),
        )?;
        Ok(Self {
            root: root.into(),
            binding,
            fd,
            record,
            coverage: None,
            sequence: 0,
            observed: false,
            finished: false,
        })
    }

    fn tick(&mut self, cancellation: &dyn Cancellation) -> Result<(), Error> {
        if !self.observed {
            self.observed = true;
            self.emit(EventName::RuntimeObserved, "registered".into(),
                json!({"registration":"admitted_before_exec","agent_ready":false,
                    "agent_pid":self.binding.agent_pid,"agent_start_ticks":self.binding.agent_ticks}),
                None, cancellation)?;
        }
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // poll borrows one initialized descriptor for a nonblocking exit observation.
        if unsafe { libc::poll(&mut poll, 1, 0) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(Error::Evidence);
        }
        let exited = poll.revents & (libc::POLLIN | libc::POLLHUP) != 0;
        let state: State = {
            let _lock = storage::lock(&self.root)?;
            storage::read(&self.root.join("state.json"))?
        };
        if exited {
            self.record["root_exited"] = json!(true);
            self.record["status"] = json!("root_exited");
            self.emit(
                EventName::RuntimeExited,
                "exited".into(),
                json!({"root_exited":true,"source":"pidfd","exit_status":null,
                    "descendants_reaped":false,"task_success":null}),
                Some(&state),
                cancellation,
            )?;
            self.finished = true;
        }
        let coverage = json!({
            "runtime":if exited {"exited"} else {"registered"},
            "native_callbacks":if exited {"ended"} else if state.gap {"gap"} else if state.attached {"attached"} else if state.session_id.is_some() {"session_ended"} else {"awaiting_session"},
            "session_id":state.session_id,"session_epoch":state.attachment,
            "session_start_observed":state.session_start_observed,
            "notification_delivery":if self.record["delivery_gap"] == true {"gap"} else {"no_failure_observed"},
            "os_coverage":"not_attached","required_safety":"unsupported",
            "tool_guard":if self.binding.prepared.config.tool_guard.is_some() {"configured_not_certified"} else {"not_configured"}
        });
        if self.coverage.as_ref() != Some(&coverage) {
            if self.sequence < 1024 {
                self.sequence += 1;
                let payload =
                    json!({"previous":self.coverage,"current":coverage,"sampling":"owner_poll"});
                self.coverage = Some(coverage);
                self.emit(
                    EventName::CoverageChanged,
                    format!("coverage-{}", self.sequence),
                    payload,
                    Some(&state),
                    cancellation,
                )?;
            } else {
                self.record["delivery_gap"] = json!(true);
                self.record["coverage_capacity_exceeded"] = json!(true);
            }
        }
        self.record["coverage_sequence"] = json!(self.sequence);
        self.record["coverage"] = json!(self.coverage);
        storage::replace(&self.root.join("runtime.json"), &self.record)
    }

    fn emit(
        &mut self,
        name: EventName,
        occurrence: String,
        payload: Value,
        state: Option<&State>,
        cancellation: &dyn Cancellation,
    ) -> Result<(), Error> {
        let event = Notification {
            format: 1,
            event: name,
            source: "runtime_owner".into(),
            native_event: None,
            runtime_id: self.binding.runtime_id.clone(),
            runtime_generation: 1,
            session_id: state.and_then(|s| s.session_id.clone()),
            session_epoch: state
                .filter(|s| s.session_id.is_some())
                .map(|s| s.attachment),
            turn_id: None,
            turn_unknown_reason: Some("process_observation_does_not_prove_task_acceptance".into()),
            tool_call_id: None,
            subagent_id: None,
            occurrence_id: occurrence,
            config_revision: self.binding.prepared.revision.clone(),
            payload,
        };
        let routes = self
            .binding
            .prepared
            .config
            .notifications
            .as_ref()
            .ok_or(Error::Evidence)?;
        let Some(commands) = routes.get(&name) else {
            return Ok(());
        };
        let result = (|| {
            for command in commands {
                command.check_pins().map_err(evidence)?;
            }
            Core::new()
                .map_err(evidence)?
                .notify(
                    &event,
                    &commands
                        .iter()
                        .map(|c| c.provider_id.clone())
                        .collect::<Vec<_>>(),
                    &mut Host {
                        commands,
                        deadline: Instant::now() + Duration::from_secs(2),
                    },
                    &mut FileJournal::new(self.root.join("owner-journal")).map_err(evidence)?,
                    cancellation,
                )
                .map_err(evidence)
        })();
        if result
            .as_ref()
            .map_or(true, |report| report["gap"] != false)
        {
            self.record["delivery_gap"] = json!(true);
        }
        storage::replace(&self.root.join("runtime.json"), &self.record)
    }
}
