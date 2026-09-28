//! One bounded process per native callback; host scheduling remains outside AW.
use crate::{
    ipc, mapping,
    model::{Configuration, Request, Response},
    process, provider, Error,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}
pub(crate) fn private_dir(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    // getuid has no pointer arguments or side effects.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_dir() || metadata.uid() != uid {
        return Err("state directory must be owned by the current user and not a symlink".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
struct SocketGuard<'a>(&'a Path);
impl Drop for SocketGuard<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}

#[derive(Default)]
struct Leases(BTreeMap<u32, u64>);

impl Leases {
    fn prune(&mut self) {
        self.0
            .retain(|pid, start| process_starttime(*pid).ok().flatten() == Some(*start));
    }

    fn acquire(&mut self, pid: u32) -> Result<(), Error> {
        self.prune();
        let start = process_starttime(pid)?.ok_or("lease owner is not alive")?;
        if !self.0.contains_key(&pid) && self.0.len() >= 128 {
            return Err("daemon session lease limit exceeded".into());
        }
        self.0.insert(pid, start);
        Ok(())
    }
}

fn process_starttime(pid: u32) -> Result<Option<u64>, Error> {
    if pid == 0 {
        return Err("lease owner PID must be nonzero".into());
    }
    let path = std::path::PathBuf::from(format!("/proc/{pid}/stat"));
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // The socket and its leases belong to the same local user.
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err("lease owner must belong to the current user".into());
    }
    let stat = match fs::read(path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // comm may contain spaces or parentheses. Fields after its final ')' are ASCII.
    let end = stat
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or("invalid process stat")?;
    let mut fields = std::str::from_utf8(&stat[end + 1..])?.split_whitespace();
    if matches!(fields.next(), Some("Z" | "X") | None) {
        return Ok(None);
    }
    Ok(Some(
        fields.nth(18).ok_or("missing process starttime")?.parse()?,
    ))
}

pub(crate) fn serve(config: Configuration, socket: &Path, idle_seconds: u64) -> Result<(), Error> {
    if !(1..=86400).contains(&idle_seconds) {
        return Err("idle timeout must be 1..86400 seconds".into());
    }
    let parent = socket.parent().ok_or("socket has no parent")?;
    private_dir(parent)?;
    // Do not unlink an existing listener or stale path owned by another run.
    let listener = UnixListener::bind(socket)?;
    let _guard = SocketGuard(socket);
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    // Handlers only set an atomic flag. No allocation, locking or I/O in a signal handler.
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as usize);
        libc::signal(libc::SIGINT, stop as *const () as usize);
    }
    let mut audit_options = OpenOptions::new();
    audit_options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let audit = Arc::new(Mutex::new(audit_options.open(parent.join("audit.jsonl"))?));
    let active = Arc::new(AtomicUsize::new(0));
    let serial = Arc::new(AtomicU64::new(1));
    let leases = Arc::new(Mutex::new(Leases::default()));
    let config = Arc::new(config);
    let mut workers = Vec::new();
    let mut last = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        workers.retain(|worker: &thread::JoinHandle<()>| !worker.is_finished());
        let live_leases = {
            let mut leases = leases.lock().map_err(|_| "session lease lock poisoned")?;
            leases.prune();
            leases.0.len()
        };
        if active.load(Ordering::Relaxed) == 0
            && live_leases == 0
            && last.elapsed() >= Duration::from_secs(idle_seconds)
        {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                last = Instant::now();
                if active.load(Ordering::Relaxed) >= 64 {
                    drop(stream);
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let active = active.clone();
                let config = config.clone();
                let audit = audit.clone();
                let serial = serial.clone();
                let leases = leases.clone();
                workers.push(thread::spawn(move||{
                    let started=Instant::now();
                    let result=(||->Result<(),Error>{
                        let request:Request=ipc::read(&mut stream,Instant::now()+Duration::from_secs(10))?;
                        let id=serial.fetch_add(1,Ordering::Relaxed);
                        let mut response=Response {code:0,stdout:vec![],stderr:vec![],error:None,revision:config.revision.clone()};
                        match request.op.as_str() {
                            "status"=>{
                                let mut leases=leases.lock().map_err(|_|"session lease lock poisoned")?;
                                leases.prune();
                                response.stdout=serde_json::to_vec(&json!({"pid":std::process::id(),"revision":config.revision,"active":active.load(Ordering::Relaxed).saturating_sub(1),"leases":leases.0.len()}))?;
                            }
                            "lease"=>{
                                let result=leases.lock().map_err(|_|"session lease lock poisoned")?.acquire(request.pid);
                                if let Err(error)=result {response.code=125;response.error=Some(error.to_string());}
                            }
                            "release"=>{leases.lock().map_err(|_|"session lease lock poisoned")?.0.remove(&request.pid);}
                            "stop"=>STOP.store(true,Ordering::Relaxed),
                            "invoke"=>{
                                let structured = config.value["spec"]["providers"][&request.provider]["protocol"] == "aw-provider/v1alpha1";
                                let mut disposition = "native";
                                let result = if structured {
                                    provider::invoke(&config,&request,&STOP).map(|outcome|{disposition=outcome.disposition;outcome.output})
                                } else {
                                    config.process(&request).and_then(|(process_config,timeout)|process::run(&process_config,&[],&request.input,timeout,&STOP))
                                };
                                match result {
                                    Ok(output)=>{response.code=output.exit_code;response.stdout=output.stdout;response.stderr=output.stderr;}
                                    Err(error)=>{
                                        response.code=125;
                                        response.error=Some(if structured {"structured_provider_failed".into()} else {error.to_string()});
                                        if structured {
                                            disposition="error";
                                            let fallback=(||->Result<_,Error>{
                                                let step=config.step(&request)?;
                                                let adapter=config.agent(&request.agent)?["adapter"].as_str().ok_or("missing adapter")?;
                                                mapping::failure(adapter,&request.event,step["on_error"].as_str().ok_or("missing error policy")?)
                                            })();
                                            if let Ok((code,stdout,stderr))=fallback {response.code=code;response.stdout=stdout;response.stderr=stderr;}
                                        }
                                    }
                                }
                                let entry=json!({"version":1,"request_id":id,"at_ms":SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),"config_revision":config.revision,
                                    "agent":request.agent,"event":request.event,"provider":request.provider,"input_bytes":request.input.len(),"stdout_bytes":response.stdout.len(),"stderr_bytes":response.stderr.len(),
                                    "exit_code":response.code,"duration_ms":started.elapsed().as_millis(),"error":response.error,"protocol":if structured {"aw-provider/v1alpha1"}else{"native-hook/v1alpha1"},"disposition":disposition});
                                let mut audit=audit.lock().map_err(|_|"audit lock poisoned")?;
                                serde_json::to_writer(&mut *audit,&entry)?;audit.write_all(b"\n")?;audit.flush()?;
                            }
                            _=>{response.code=125;response.error=Some("unknown daemon operation".into());}
                        }
                        ipc::write(&mut stream,&response,Instant::now()+Duration::from_secs(10))?;Ok(())
                    })();
                    if result.is_err(){ eprintln!("AW callback transport or audit failed"); }
                    active.fetch_sub(1,Ordering::Relaxed);
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(error.into()),
        }
    }
    STOP.store(true, Ordering::Relaxed);
    for worker in workers {
        worker.join().map_err(|_| "callback worker panicked")?;
    }
    Ok(())
}
